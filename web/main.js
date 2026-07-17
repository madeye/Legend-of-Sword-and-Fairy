// Main-thread side of the web build: fetches the game data, spawns the
// engine worker, forwards keyboard input through a SharedArrayBuffer ring,
// and blits the RGBA frames the worker posts back onto the canvas.

// Every data file the engine reads (upper-case DOS names, served from ../pal/).
const FILES = [
  "ABC.MKF", "BALL.MKF", "DATA.MKF", "F.MKF", "FBP.MKF", "FIRE.MKF",
  "GOP.MKF", "M.MSG", "MAP.MKF", "MGO.MKF", "MUS.MKF", "PAT.MKF",
  "RGM.MKF", "RNG.MKF", "SSS.MKF", "VOC.MKF",
  "WOR16.ASC", "WOR16.FON", "WORD.DAT",
];

// Order MUST match WEB_KEYS in src/web.rs: key events are sent as indexes.
const KEY_CODES = [
  "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
  "Escape", "Insert", "Enter", "Space", "ControlLeft",
  "PageUp", "PageDown", "Home", "End",
  "KeyR", "KeyA", "KeyD", "KeyE", "KeyW", "KeyQ", "KeyF", "KeyS",
];
// Aliases folded onto an equivalent entry above.
const KEY_ALIASES = { NumpadEnter: "Enter", AltLeft: "Escape", AltRight: "Escape" };

const RING_CAPACITY = 128;

const canvas = document.getElementById("screen");
const ctx = canvas.getContext("2d");
const status = document.getElementById("status");

async function boot() {
  if (typeof SharedArrayBuffer === "undefined") {
    status.textContent =
      "SharedArrayBuffer unavailable — serve with COOP/COEP headers (use web/serve.py).";
    return;
  }

  // Key-event ring buffer: [0] = write count, [1] = engine read count
  // (debug), then RING_CAPACITY slots. Created (and listeners attached)
  // before the data fetch so keys pressed while loading aren't lost.
  const inputSab = new SharedArrayBuffer(4 * (2 + RING_CAPACITY));
  const input = new Int32Array(inputSab);
  window.__palInput = input; // debug handle

  const pushKey = (code, pressed) => {
    const id = KEY_CODES.indexOf(KEY_ALIASES[code] ?? code);
    if (id < 0) return false;
    const seq = Atomics.load(input, 0);
    Atomics.store(input, 2 + (seq % RING_CAPACITY), (id << 1) | (pressed ? 1 : 0));
    Atomics.store(input, 0, seq + 1);
    return true;
  };
  window.addEventListener("keydown", (e) => {
    if (pushKey(e.code, true)) e.preventDefault();
  });
  window.addEventListener("keyup", (e) => {
    if (pushKey(e.code, false)) e.preventDefault();
  });

  // Virtual gamepad (touch devices): buttons push the same key ring entries.
  // Pointer capture keeps the release firing even if the finger slides off.
  for (const btn of document.querySelectorAll("#touch [data-key]")) {
    const code = btn.dataset.key;
    btn.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      pushKey(code, true);
      try { btn.setPointerCapture(e.pointerId); } catch (_) {}
    });
    for (const ev of ["pointerup", "pointercancel"]) {
      btn.addEventListener(ev, (e) => {
        e.preventDefault();
        pushKey(code, false);
      });
    }
    btn.addEventListener("contextmenu", (e) => e.preventDefault());
  }

  // Audio: an AudioWorklet drains a sample ring the engine worker fills.
  // 8-byte header (write/read counters) + 16384 stereo f32 frames.
  let audioSab = null;
  let audioRate = 44100;
  let audioCtx = null;
  try {
    audioCtx = new (window.AudioContext || window.webkitAudioContext)();
    audioRate = audioCtx.sampleRate;
    audioSab = new SharedArrayBuffer(8 + 16384 * 2 * 4);
    await audioCtx.audioWorklet.addModule("worklet.js");
    const node = new AudioWorkletNode(audioCtx, "pal-audio", {
      outputChannelCount: [2],
      processorOptions: { sab: audioSab },
    });
    node.connect(audioCtx.destination);
    window.__palAudio = audioSab; // debug handle
  } catch (e) {
    console.warn("audio unavailable:", e);
    audioSab = null;
  }
  // Browsers keep the context suspended until a user gesture. On touch
  // devices there is no keydown, and tap "click" events can be swallowed by
  // zoom heuristics — touchend/pointerup are the activation events that
  // reliably fire on a tap. A visible hint shows until the context runs.
  const soundHint = document.getElementById("sound");
  const updateSoundHint = () => {
    soundHint.hidden = !audioCtx || audioCtx.state === "running";
  };
  const resumeAudio = () => {
    if (audioCtx && audioCtx.state !== "running") audioCtx.resume();
  };
  if (audioCtx) {
    audioCtx.onstatechange = updateSoundHint;
    updateSoundHint();
  }
  for (const ev of ["keydown", "click", "pointerup", "touchend"]) {
    window.addEventListener(ev, resumeAudio);
  }

  // Fetch all game data up front (~13 MB).
  let loaded = 0;
  const files = {};
  await Promise.all(FILES.map(async (name) => {
    const resp = await fetch(`../pal/${name}`);
    if (!resp.ok) throw new Error(`fetch ${name}: HTTP ${resp.status}`);
    files[name] = new Uint8Array(await resp.arrayBuffer());
    status.textContent = `loading game data… ${++loaded}/${FILES.length}`;
  }));

  // Seed saved games (slots 1-5) from localStorage into the file map.
  for (let slot = 1; slot <= 5; slot++) {
    const b64 = localStorage.getItem(`pal-save-${slot}`);
    if (!b64) continue;
    const bin = atob(b64);
    const u8 = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
    files[`${slot}.RPG`] = u8;
  }

  const worker = new Worker("worker.js");
  worker.onmessage = (e) => {
    if (e.data && e.data.byteLength === 320 * 200 * 4) {
      status.textContent = "";
      ctx.putImageData(
        new ImageData(new Uint8ClampedArray(e.data.buffer), 320, 200), 0, 0);
    } else if (e.data && e.data.palSave !== undefined) {
      // Persist a saved game posted by the engine.
      const u8 = e.data.data;
      let bin = "";
      for (let i = 0; i < u8.length; i += 0x8000) {
        bin += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
      }
      localStorage.setItem(`pal-save-${e.data.palSave}`, btoa(bin));
    } else if (typeof e.data === "string") {
      status.textContent = e.data; // worker status/error text
    }
  };
  worker.onerror = (e) => { status.textContent = `worker error: ${e.message}`; };
  worker.postMessage({ files, input: inputSab, audio: audioSab, audioRate },
    Object.values(files).map((u8) => u8.buffer));
  status.textContent = "starting engine…";
}

boot().catch((e) => { status.textContent = String(e); });
