#!/usr/bin/env python3
"""Offline TTS pipeline for PAL NPC dialog voice-over (Qwen3-TTS).

Two-stage pipeline for consistent per-character voices:
  1. `--design-refs`: for every character in voice_map.json without a
     reference clip, synthesize one with Qwen3-TTS VoiceDesign (1.7B) from
     its Chinese voice description -> refs/<id>.wav. Curate: listen, delete
     any you dislike, re-run (a fresh design is sampled each time).
  2. default run: every faced dialog line from voice_manifest.json (produced
     by `cargo run --example voicedump`) is synthesized with Qwen3-TTS Base
     (0.6B) by cloning the character's reference clip, then encoded to
     low-bitrate mono Ogg Vorbis and packed into per-scene banks
     ../../pal/voice/NNN.vbk.

Usage (run from tools/voice with uv):
  uv run gen_voices.py --list-faces           # face -> character coverage
  uv run gen_voices.py --design-refs          # stage 1 (casting)
  uv run gen_voices.py --scenes 1 2           # stage 2 for some scenes
  uv run gen_voices.py                        # stage 2, everything
  uv run gen_voices.py --stats                # cache/bank size summary
"""

import argparse
import hashlib
import json
import struct
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).parent
MANIFEST = HERE / "voice_manifest.json"
VOICE_MAP = HERE / "voice_map.json"
CACHE = HERE / "cache"
REFS = HERE / "refs"
BANK_DIR = HERE / "../../pal/voice"

SAMPLE_RATE = 16000  # bank sample rate (Qwen3-TTS outputs 24000)
VORBIS_BITRATE = "20k"

DESIGN_MODEL = "mlx-community/Qwen3-TTS-12Hz-1.7B-VoiceDesign-bf16"
CLONE_MODEL = "mlx-community/Qwen3-TTS-12Hz-1.7B-Base-bf16"
REFS_LONG = HERE / "refs_long"


# ---------------------------------------------------------------------------
# Text preparation (mirrors src/ui.rs display_text control-code parsing)
# ---------------------------------------------------------------------------

def big5_is_lead(b: int) -> bool:
    return 0x81 <= b <= 0xFE


def strip_control_codes(raw: bytes) -> bytes:
    """Keep only printable text bytes, dropping inline control codes."""
    out = bytearray()
    i = 0
    while i < len(raw):
        b = raw[i]
        if big5_is_lead(b) and i + 1 < len(raw):
            out += raw[i : i + 2]
            i += 2
        elif b in b"-'@\"()":  # color toggles + waiting-icon selectors
            i += 1
        elif b == ord("$"):  # $NN: set delay time
            i += 3
        elif b == ord("~"):  # ~NN: delay and end line (rest is dropped)
            break
        elif b == ord("\\"):  # escape: next char is literal
            i += 1
            if i < len(raw):
                if big5_is_lead(raw[i]) and i + 1 < len(raw):
                    out += raw[i : i + 2]
                    i += 2
                else:
                    out.append(raw[i])
                    i += 1
        else:
            out.append(b)
            i += 1
    return bytes(out)


def prepare_text(big5_hex: str, t2s) -> str:
    import re

    raw = strip_control_codes(bytes.fromhex(big5_hex))
    text = raw.decode("big5", errors="ignore")
    # Layout whitespace (full-width U+3000 pads, stray spaces/newlines) reads
    # as unnatural pauses in TTS — drop it all; the text is pure Chinese.
    for ws in ("　", " ", "\t", "\r", "\n"):
        text = text.replace(ws, "")
    # "∼" marks a drawn-out tone in the game script; as punctuation it reads
    # best as a short pause. Runs of full-width dots are ellipses.
    text = text.replace("∼", ",").replace("~", ",")
    text = re.sub("．+", lambda m: "……" if len(m.group()) > 1 else "。", text)
    # Collapse punctuation stacks the replacements may have created.
    text = re.sub(r",+", ",", text).strip(",")
    # Lines with no speakable content (pure ellipsis/punctuation) are silence.
    if not re.search(r"[一-鿿0-9A-Za-z]", text):
        return ""
    # The game text is traditional Chinese; the TTS is happier in simplified.
    text = t2s.convert(text).strip()
    return text


# ---------------------------------------------------------------------------
# Manifest / voice map
# ---------------------------------------------------------------------------

def load_manifest():
    m = json.loads(MANIFEST.read_text())
    return m["messages"]


def load_characters():
    vm = json.loads(VOICE_MAP.read_text())
    chars = vm["characters"]
    face_to_char = {}
    for cid, c in chars.items():
        for f in c["faces"]:
            face_to_char[f] = cid
    return chars, face_to_char


def ref_path(cid: str) -> Path:
    return REFS / f"{cid}.wav"


# ---------------------------------------------------------------------------
# Synthesis
# ---------------------------------------------------------------------------

def qwen3_tag(cid: str) -> str:
    """Cache tag tied to the clone model and the reference clip's content, so
    re-designing a voice (or upgrading the model) invalidates only the
    affected cached lines."""
    h = hashlib.sha1(ref_path(cid).read_bytes()).hexdigest()[:8]
    return f"q3b|{CLONE_MODEL}|{cid}-{h}"


def cache_key(tag: str, text: str) -> Path:
    h = hashlib.sha1(f"{tag}|{text}".encode()).hexdigest()
    return CACHE / f"{h}.wav"


def postprocess(audio, rate: int):
    """Trim silence, normalize, resample to the bank rate. None if silent."""
    import numpy as np
    from scipy.signal import resample_poly

    amp = np.abs(audio)
    gate = max(0.005, float(amp.max()) * 0.01)
    idx = np.nonzero(amp > gate)[0]
    if len(idx) == 0:
        return None
    pad = rate // 20  # 50 ms
    audio = audio[max(0, idx[0] - pad) : min(len(audio), idx[-1] + pad)]
    peak = float(np.abs(audio).max())
    if peak > 0:
        audio = audio * (0.891 / peak)  # -1 dBFS
    if rate != SAMPLE_RATE:
        from math import gcd

        g = gcd(SAMPLE_RATE, rate)
        audio = resample_poly(audio, SAMPLE_RATE // g, rate // g)
    return audio


class CloneSynth:
    """Lazy-loaded Qwen3-TTS Base model: clone a character reference."""

    def __init__(self, chars):
        self.model = None
        self.chars = chars

    def generate(self, text: str, cid: str):
        import numpy as np

        if self.model is None:
            from mlx_audio.tts.utils import load_model

            self.model = load_model(CLONE_MODEL)
        results = list(
            self.model.generate(
                text=text,
                ref_audio=str(ref_path(cid)),
                ref_text=self.chars[cid]["ref_text"],
            )
        )
        chunks = [np.array(r.audio) for r in results]
        if not chunks:
            return None
        return np.concatenate(chunks)


# ---------------------------------------------------------------------------
# ffmpeg / Ogg encoding
# ---------------------------------------------------------------------------

_FFMPEG = None


def find_ffmpeg() -> str:
    """An ffmpeg with libvorbis (homebrew's plain ffmpeg may lack it)."""
    global _FFMPEG
    if _FFMPEG is None:
        import os
        import subprocess

        candidates = [
            os.environ.get("FFMPEG"),
            "ffmpeg",
            "/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg",
        ]
        for c in candidates:
            if not c:
                continue
            try:
                enc = subprocess.run([c, "-hide_banner", "-encoders"],
                                     capture_output=True).stdout
            except FileNotFoundError:
                continue
            if b"libvorbis" in enc:
                _FFMPEG = c
                break
        if _FFMPEG is None:
            sys.exit("no ffmpeg with libvorbis found (set $FFMPEG)")
    return _FFMPEG


def encode_ogg_pcm(pcm) -> bytes:
    """Encode mono i16 PCM to one low-bitrate Vorbis stream (ffmpeg)."""
    import subprocess

    p = subprocess.run(
        [find_ffmpeg(), "-v", "error",
         "-f", "s16le", "-ar", str(SAMPLE_RATE), "-ac", "1", "-i", "-",
         "-c:a", "libvorbis", "-b:a", VORBIS_BITRATE, "-f", "ogg", "-"],
        input=pcm.astype("<i2").tobytes(),
        capture_output=True,
    )
    if p.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {p.stderr.decode()}")
    return p.stdout


# ---------------------------------------------------------------------------
# Bank packing. All clips of a scene are concatenated into ONE Vorbis stream
# (per-clip Ogg files would each carry ~3 KB of codebook headers); the index
# addresses clips as sample ranges into the decoded stream:
#   "PVB1" | u32 count | count*(u32 msg,u32 start_sample,u32 n_samples) | ogg
# ---------------------------------------------------------------------------

def pack_bank(path: Path, entries: dict):
    """entries: msg_id -> mono i16 PCM (numpy array) at SAMPLE_RATE"""
    import numpy as np

    index = []
    chunks = []
    pos = 0
    for msg in sorted(entries):
        pcm = entries[msg]
        index.append((msg, pos, len(pcm)))
        chunks.append(pcm)
        pos += len(pcm)
    ogg = encode_ogg_pcm(np.concatenate(chunks))
    out = bytearray(b"PVB1")
    out += struct.pack("<I", len(index))
    for msg, start, n in index:
        out += struct.pack("<III", msg, start, n)
    out += ogg
    path.write_bytes(out)
    return len(out)


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

def char_lines(messages, t2s, face_to_char):
    """cid -> [(msg_id, text)] of its dialog lines (manifest order)."""
    out = defaultdict(list)
    for m in messages:
        if m["kind"] != "dialog":
            continue
        cid = face_to_char.get(m["face"])
        if cid is None:
            continue
        text = prepare_text(m["big5_hex"], t2s)
        if text:
            out[cid].append((m["msg"], text))
    return out


def cmd_extend_refs(messages, only=None):
    """Build refs_long/<cid>.wav (>= ~15 s): the approved reference plus the
    same timbre (Qwen3 1.7B clone) speaking the character's own longest game
    lines. MiniMax voice cloning needs >= 10 s of source audio."""
    import numpy as np
    import opencc
    import soundfile as sf

    t2s = opencc.OpenCC("t2s")
    chars, face_to_char = load_characters()
    lines = char_lines(messages, t2s, face_to_char)
    REFS_LONG.mkdir(exist_ok=True)
    synth = CloneSynth(chars)

    for cid in chars:
        if only and cid not in only:
            continue
        if (REFS_LONG / f"{cid}.wav").exists():
            continue
        if not ref_path(cid).exists():
            print(f"  {cid}: no base ref, skipping", file=sys.stderr)
            continue
        ref, rate = sf.read(ref_path(cid), dtype="float32")
        parts = [ref]
        total = len(ref) / rate
        # Longest lines first: fewer synth calls to cross the bar.
        for _, text in sorted(lines.get(cid, []), key=lambda x: -len(x[1]))[:8]:
            if total >= 18.0:
                break
            audio = synth.generate(text, cid)
            if audio is None:
                continue
            audio = np.asarray(audio)
            parts += [np.zeros(int(0.35 * rate), dtype=np.float32), audio]
            total += len(audio) / rate + 0.35
        if total < 10.5:
            print(f"  {cid}: only {total:.1f}s, below the 10 s clone minimum",
                  file=sys.stderr)
            continue
        sf.write(REFS_LONG / f"{cid}.wav", np.concatenate(parts), rate)
        print(f"  refs_long/{cid}.wav  {total:.1f}s")


def cmd_mm_clone(only=None):
    """Register MiniMax cloned voices for characters with refs_long."""
    import minimax

    chars, _ = load_characters()
    voices = minimax.load_voices()
    for cid in chars:
        if only and cid not in only:
            continue
        if cid in voices:
            continue
        wav = REFS_LONG / f"{cid}.wav"
        if not wav.exists():
            continue
        try:
            voices[cid] = minimax.clone_voice(cid, wav)
            print(f"  {cid} -> {voices[cid]['voice_id']}")
        except Exception as e:
            print(f"  {cid}: clone failed: {e}", file=sys.stderr)
        minimax.VOICES_FILE.write_text(json.dumps(voices, indent=2) + "\n")
    print(f"{len(voices)} cloned voices in {minimax.VOICES_FILE.name}")


def build_blocks(messages, t2s, face_to_char, chars, scene_filter=None):
    """Conversation blocks per scene: consecutive msg-id runs, keeping name
    lines as speaker markers. Yields [(msg_id, cid, text, is_dialog)]."""
    per_scene = defaultdict(list)
    for m in messages:
        for s in m["scenes"]:
            if scene_filter and s not in scene_filter:
                continue
            per_scene[s].append(m)
    blocks = []
    for s in sorted(per_scene):
        msgs = sorted({m["msg"]: m for m in per_scene[s]}.values(),
                      key=lambda m: m["msg"])
        block = []
        prev = None
        for m in msgs:
            if prev is not None and m["msg"] - prev > 3 and block:
                blocks.append(block)
                block = []
            text = prepare_text(m["big5_hex"], t2s)
            cid = face_to_char.get(m["face"])
            block.append((m["msg"], cid, text, m["kind"] == "dialog"))
            prev = m["msg"]
        if block:
            blocks.append(block)
    return blocks


def cmd_tag_emotions(messages, scene_filter=None):
    """Context-aware per-line emotion tags via a MiniMax text model."""
    import opencc

    import minimax

    t2s = opencc.OpenCC("t2s")
    chars, face_to_char = load_characters()
    existing = {}
    if minimax.EMOTIONS_FILE.exists():
        existing = {int(k): v for k, v in
                    json.loads(minimax.EMOTIONS_FILE.read_text()).items()}
    blocks = build_blocks(messages, t2s, face_to_char, chars, scene_filter)
    todo = [b for b in blocks
            if any(d and m not in existing for m, _, _, d in b)]
    print(f"{len(todo)} blocks to tag ({len(blocks)} total in scope)")
    personas = {cid: f"{c['who']}({c['desc']})" for cid, c in chars.items()}
    tags = minimax.tag_emotions(todo, personas)
    existing.update(tags)
    minimax.EMOTIONS_FILE.write_text(json.dumps(
        {str(k): v for k, v in sorted(existing.items())},
        ensure_ascii=False, indent=0) + "\n")
    print(f"{len(tags)} lines tagged, {len(existing)} total")


def cmd_design_refs(only=None):
    """Stage 1: create missing character reference clips with VoiceDesign."""
    import numpy as np
    import soundfile as sf

    chars, _ = load_characters()
    REFS.mkdir(exist_ok=True)
    todo = [cid for cid in chars
            if (not only or cid in only) and not ref_path(cid).exists()]
    if not todo:
        print("all reference clips present; delete refs/<id>.wav to re-roll")
        return

    from mlx_audio.tts.utils import load_model

    model = load_model(DESIGN_MODEL)
    for cid in todo:
        c = chars[cid]
        results = list(model.generate_voice_design(
            text=c["ref_text"], language="Chinese", instruct=c["desc"]))
        chunks = [np.array(r.audio) for r in results]
        if not chunks:
            print(f"  {cid}: design FAILED", file=sys.stderr)
            continue
        audio = np.concatenate(chunks)
        sf.write(ref_path(cid), audio, 24000)
        print(f"  refs/{cid}.wav  {len(audio) / 24000:.1f}s  ({c['who']})")
    print("listen to refs/, delete any bad ones, and re-run to re-roll them")


def cmd_list_faces(messages):
    import opencc

    t2s = opencc.OpenCC("t2s")
    chars, face_to_char = load_characters()
    by_face = defaultdict(lambda: {"dialogs": 0, "sample": None})
    for m in messages:
        if m["kind"] != "dialog":
            continue
        f = by_face[m["face"]]
        f["dialogs"] += 1
        if f["sample"] is None:
            f["sample"] = prepare_text(m["big5_hex"], t2s)
    for face in sorted(by_face):
        cid = face_to_char.get(face)
        who = chars[cid]["who"] if cid else "UNMAPPED"
        ref = "ref✓" if cid and ref_path(cid).exists() else "ref✗"
        print(f"face {face:3d}  lines {by_face[face]['dialogs']:4d}  "
              f"{who:12s} {ref}  e.g. {by_face[face]['sample'] or ''}")


def cmd_generate(messages, scene_filter, limit, dry_run, engine):
    import opencc
    import soundfile as sf

    t2s = opencc.OpenCC("t2s")
    chars, face_to_char = load_characters()
    CACHE.mkdir(exist_ok=True)

    emotions = {}
    if engine == "minimax":
        import minimax

        voices = minimax.load_voices()
        synth = minimax.Synth(voices)
        ineligible = {cid for cid in chars if cid not in voices}
        if minimax.EMOTIONS_FILE.exists():
            emotions = {int(k): v for k, v in
                        json.loads(minimax.EMOTIONS_FILE.read_text()).items()}

        def tag_of(cid, msg):
            emo = emotions.get(msg, "")
            return f"mmx|{minimax.TTS_MODEL}|{voices[cid]['voice_id']}|{emo}"

        def synthesize(text, cid, msg):
            return synth.generate(text, cid, emotions.get(msg))
    else:
        synth = CloneSynth(chars)
        ineligible = {cid for cid in chars if not ref_path(cid).exists()}

        def tag_of(cid, msg):
            return qwen3_tag(cid)

        def synthesize(text, cid, msg):
            return synth.generate(text, cid)

    banks = defaultdict(dict)  # scene -> msg_id -> cache path
    skipped = defaultdict(int)
    todo = []
    for m in messages:
        if m["kind"] != "dialog":
            continue
        scenes = m["scenes"]
        if scene_filter and not any(s in scene_filter for s in scenes):
            continue
        cid = face_to_char.get(m["face"])
        if cid is None or cid in ineligible:
            skipped[m["face"]] += 1
            continue
        text = prepare_text(m["big5_hex"], t2s)
        if not text:
            continue
        todo.append((m["msg"], scenes, cid, text))

    # Group by (character, emotion): the MiniMax connection's voice settings
    # are fixed at task_start, so this maximizes connection reuse.
    todo.sort(key=lambda t: (t[2], emotions.get(t[0], "")))

    n_synth = sum(1 for msg, _, c, t in todo
                  if not cache_key(tag_of(c, msg), t).exists())
    print(f"[{engine}] {len(todo)} lines, {n_synth} to synthesize, "
          f"{len(todo) - n_synth} cached; skipped faces: "
          f"{dict(sorted(skipped.items())) or 'none'}"
          + (f"; characters not usable with this engine: {sorted(ineligible)}"
             if ineligible else ""))
    if dry_run:
        return

    done = 0
    for msg, scenes, cid, text in todo:
        if limit and done >= limit:
            break
        path = cache_key(tag_of(cid, msg), text)
        if not path.exists():
            audio = synthesize(text, cid, msg)
            if audio is not None:
                audio = postprocess(audio, 24000)
            if audio is None:
                print(f"  msg {msg}: synthesis failed, skipping", file=sys.stderr)
                continue
            sf.write(path, audio, SAMPLE_RATE, subtype="PCM_16")
            done += 1
            if done % 25 == 0:
                print(f"  synthesized {done}/{n_synth}")
        for s in scenes:
            if not scene_filter or s in scene_filter:
                banks[s][msg] = path

    BANK_DIR.mkdir(parents=True, exist_ok=True)
    pcms = {}  # wav path -> i16 PCM, shared across scenes
    total = 0
    for scene in sorted(banks):
        entries = {}
        for m, p in banks[scene].items():
            if not p.exists():
                continue
            if p not in pcms:
                data, rate = sf.read(p, dtype="int16")
                pcms[p] = data if rate == SAMPLE_RATE else None
            if pcms[p] is not None and len(pcms[p]):
                entries[m] = pcms[p]
        if not entries:
            continue
        size = pack_bank(BANK_DIR / f"{scene:03d}.vbk", entries)
        total += size
        print(f"  {scene:03d}.vbk  {len(entries):4d} lines  {size / 1024:7.1f} KB")
    print(f"total {total / 1024 / 1024:.1f} MB in {len(banks)} banks")


def cmd_stats():
    wavs = list(CACHE.glob("*.wav"))
    if not wavs:
        print("cache empty")
        return
    import soundfile as sf

    secs = []
    for p in wavs:
        try:
            info = sf.info(str(p))
            secs.append(info.frames / info.samplerate)
        except Exception:
            pass
    print(f"{len(wavs)} clips cached, {sum(secs) / 60:.1f} min audio, "
          f"avg {sum(secs) / max(len(secs), 1):.1f}s")
    banks = sorted(BANK_DIR.glob("*.vbk"))
    if banks:
        total = sum(p.stat().st_size for p in banks)
        print(f"{len(banks)} banks, {total / 1024 / 1024:.1f} MB total")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--design-refs", action="store_true")
    ap.add_argument("--only", nargs="*", default=None,
                    help="with --design-refs: limit to these character ids")
    ap.add_argument("--list-faces", action="store_true")
    ap.add_argument("--stats", action="store_true")
    ap.add_argument("--extend-refs", action="store_true",
                    help="build >=10s refs_long/ clips for MiniMax cloning")
    ap.add_argument("--mm-clone", action="store_true",
                    help="register MiniMax cloned voices from refs_long/")
    ap.add_argument("--tag-emotions", action="store_true",
                    help="tag per-line emotions from dialog context")
    ap.add_argument("--scenes", type=int, nargs="*", default=None)
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--engine", choices=["qwen3", "minimax"], default="qwen3",
                    help="qwen3: offline MLX clone (default); minimax: cloud "
                         "cloned voices on speech-2.8-hd")
    args = ap.parse_args()

    only = set(args.only) if args.only else None
    if args.design_refs:
        cmd_design_refs(only)
        return
    if args.mm_clone:
        cmd_mm_clone(only)
        return
    messages = load_manifest()
    scenes = set(args.scenes or []) or None
    if args.extend_refs:
        cmd_extend_refs(messages, only)
    elif args.tag_emotions:
        cmd_tag_emotions(messages, scenes)
    elif args.list_faces:
        cmd_list_faces(messages)
    elif args.stats:
        cmd_stats()
    else:
        cmd_generate(messages, scenes, args.limit, args.dry_run, args.engine)


if __name__ == "__main__":
    main()
