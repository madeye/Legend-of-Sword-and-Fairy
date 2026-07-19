"""MiniMax cloud backend for the voice pipeline.

Three pieces:
  - voice cloning: upload an extended (>=10 s) per-character reference wav
    (built from the approved Qwen3-designed timbre) and register a cloned
    `voice_id` usable in T2A;
  - context-aware emotion tagging: a MiniMax text model reads each dialog
    block (speakers + surrounding lines) and assigns one of the T2A emotion
    enums to every line;
  - synthesis: T2A v2 synchronous WebSocket with the cloned voice. The
    emotion lives in `task_start`, so lines are grouped by (character,
    emotion) to reuse connections.
"""

import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
VOICES_FILE = HERE / "minimax_voices.json"
EMOTIONS_FILE = HERE / "emotions.json"

API = "https://api.minimaxi.com"
WSS = "wss://api.minimaxi.com/ws/v1/t2a_v2"
TTS_MODEL = "speech-2.8-hd"
CHAT_MODEL = "MiniMax-M2.7-highspeed"

# voice_setting.emotion enum supported by speech-2.8 (no "whisper").
EMOTIONS = ["happy", "sad", "angry", "fearful", "disgusted", "surprised",
            "calm", "fluent"]


def api_key() -> str:
    import os

    key = os.environ.get("MINIMAX_API_KEY")
    if not key and (HERE / ".env").exists():
        for line in (HERE / ".env").read_text().splitlines():
            if line.startswith("MINIMAX_API_KEY="):
                key = line.split("=", 1)[1].strip()
    if not key:
        sys.exit("MINIMAX_API_KEY not set (env or tools/voice/.env)")
    return key


def _auth():
    return {"Authorization": f"Bearer {api_key()}"}


# ---------------------------------------------------------------------------
# Voice cloning
# ---------------------------------------------------------------------------

def load_voices() -> dict:
    if VOICES_FILE.exists():
        return json.loads(VOICES_FILE.read_text())
    return {}


def clone_voice(cid: str, wav: Path) -> dict:
    """Upload `wav` and register a cloned voice for character `cid`."""
    import urllib.request
    import uuid

    boundary = uuid.uuid4().hex
    body = bytearray()
    body += (f"--{boundary}\r\n"
             "Content-Disposition: form-data; name=\"purpose\"\r\n\r\n"
             "voice_clone\r\n").encode()
    body += (f"--{boundary}\r\n"
             f"Content-Disposition: form-data; name=\"file\"; "
             f"filename=\"{cid}.wav\"\r\n"
             "Content-Type: audio/wav\r\n\r\n").encode()
    body += wav.read_bytes()
    body += f"\r\n--{boundary}--\r\n".encode()
    req = urllib.request.Request(
        f"{API}/v1/files/upload", data=bytes(body), method="POST",
        headers={**_auth(),
                 "Content-Type": f"multipart/form-data; boundary={boundary}"})
    with urllib.request.urlopen(req, timeout=120) as r:
        up = json.load(r)
    file_id = (up.get("file") or {}).get("file_id") or up.get("file_id")
    if not file_id:
        raise RuntimeError(f"upload failed: {up}")

    voice_id = f"pal_{cid}_{int(time.time()) % 100000:05d}"
    payload = {"file_id": file_id, "voice_id": voice_id}
    req = urllib.request.Request(
        f"{API}/v1/voice_clone", data=json.dumps(payload).encode(),
        method="POST", headers={**_auth(), "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        res = json.load(r)
    base = res.get("base_resp") or {}
    if base.get("status_code") not in (0, None):
        raise RuntimeError(f"clone failed: {base}")
    return {"voice_id": voice_id, "file_id": file_id, "at": int(time.time())}


# ---------------------------------------------------------------------------
# Context-aware emotion tagging
# ---------------------------------------------------------------------------

def tag_emotions(blocks, personas) -> dict:
    """blocks: list of [(msg_id, speaker, text, is_dialog)] conversation
    runs. personas: cid -> short description. Returns {msg_id: emotion}."""
    import urllib.request

    out = {}
    system = (
        "你是一部中文武侠 RPG(仙剑奇侠传)的配音导演。给定一段对话(含说话人),"
        "为每一句台词选择最贴合上下文的情绪标签,只能从这个列表里选:"
        f"{EMOTIONS}。calm 表示平静中性,fluent 表示生动自然(默认可用)。"
        "只输出 JSON 对象:{\"<行号>\": \"<情绪>\"},不要输出其他内容。"
    )
    for bi, block in enumerate(blocks):
        dialog_ids = [m for m, _, _, d in block if d]
        if not dialog_ids:
            continue
        lines = []
        for msg, speaker, text, is_dialog in block:
            who = personas.get(speaker, speaker or "旁白")
            mark = f"[{msg}]" if is_dialog else "(说话人)"
            lines.append(f"{mark} {who}: {text}")
        user = "对话:\n" + "\n".join(lines) + \
            f"\n\n为编号 {dialog_ids} 的每一句选择情绪,输出 JSON。"
        payload = {
            "model": CHAT_MODEL,
            "messages": [{"role": "system", "content": system},
                         {"role": "user", "content": user}],
            "temperature": 0.2,
        }
        try:
            req = urllib.request.Request(
                f"{API}/v1/chat/completions", data=json.dumps(payload).encode(),
                method="POST",
                headers={**_auth(), "Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=120) as r:
                res = json.load(r)
            content = res["choices"][0]["message"]["content"]
            # Reasoning models prepend a <think> block; parse after it.
            if "</think>" in content:
                content = content.split("</think>", 1)[1]
            start, end = content.find("{"), content.rfind("}")
            tags = json.loads(content[start:end + 1])
            for k, v in tags.items():
                if v in EMOTIONS:
                    out[int(k)] = v
        except Exception as e:
            print(f"  block {bi}: emotion tagging failed ({e}); "
                  "lines fall back to auto", file=sys.stderr)
        if (bi + 1) % 20 == 0:
            print(f"  tagged {bi + 1}/{len(blocks)} blocks")
    return out


# ---------------------------------------------------------------------------
# T2A synthesis (synchronous WebSocket)
# ---------------------------------------------------------------------------

class Synth:
    """One live connection per (voice_id, emotion); callers should group
    lines accordingly (the pipeline sorts by that key)."""

    def __init__(self, voices: dict):
        self.voices = voices
        self.ws = None
        self.cur = None  # (cid, emotion)

    def _connect(self, cid: str, emotion):
        from websockets.sync.client import connect

        self.ws = connect(WSS, additional_headers=_auth(), max_size=None,
                          open_timeout=15)
        ev = json.loads(self.ws.recv(timeout=15))
        if ev.get("event") != "connected_success":
            raise RuntimeError(f"connect failed: {ev.get('base_resp')}")
        vs = {"voice_id": self.voices[cid]["voice_id"], "vol": 1.0}
        if emotion:
            vs["emotion"] = emotion
        start = {
            "event": "task_start",
            "model": TTS_MODEL,
            "language_boost": "Chinese",
            "voice_setting": vs,
            "audio_setting": {"sample_rate": 24000, "format": "pcm",
                              "channel": 1},
        }
        self.ws.send(json.dumps(start))
        ev = json.loads(self.ws.recv(timeout=30))
        if ev.get("event") != "task_started":
            raise RuntimeError(f"task_start failed: {ev.get('base_resp')}")

    def close(self):
        if self.ws is not None:
            try:
                self.ws.close()
            except Exception:
                pass
        self.ws = None
        self.cur = None

    def generate(self, text: str, cid: str, emotion=None):
        import numpy as np

        for attempt in range(4):
            try:
                if self.ws is None or self.cur != (cid, emotion):
                    self.close()
                    self._connect(cid, emotion)
                    self.cur = (cid, emotion)
                self.ws.send(json.dumps({"event": "task_continue",
                                         "text": text}))
                audio = bytearray()
                while True:
                    ev = json.loads(self.ws.recv(timeout=60))
                    if ev.get("event") == "task_failed":
                        raise RuntimeError(f"task_failed: {ev.get('base_resp')}")
                    data = ev.get("data") or {}
                    if data.get("audio"):
                        audio += bytes.fromhex(data["audio"])
                    if ev.get("is_final"):
                        break
                if not audio:
                    return None
                return (np.frombuffer(bytes(audio), "<i2").astype(np.float32)
                        / 32768.0)
            except Exception as e:
                print(f"  minimax retry {attempt + 1} ({cid}): {e}",
                      file=sys.stderr)
                self.close()
                time.sleep(2.0 * (attempt + 1))
        return None
