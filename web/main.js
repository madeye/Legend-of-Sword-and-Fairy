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

  // Fetch all game data up front (~13 MB).
  let loaded = 0;
  const files = {};
  await Promise.all(FILES.map(async (name) => {
    const resp = await fetch(`../pal/${name}`);
    if (!resp.ok) throw new Error(`fetch ${name}: HTTP ${resp.status}`);
    files[name] = new Uint8Array(await resp.arrayBuffer());
    status.textContent = `loading game data… ${++loaded}/${FILES.length}`;
  }));

  // Key-event ring buffer: [0] = write count, then RING_CAPACITY slots.
  const inputSab = new SharedArrayBuffer(4 * (1 + RING_CAPACITY));
  const input = new Int32Array(inputSab);
  window.__palInput = input; // debug handle

  const worker = new Worker("worker.js");
  worker.onmessage = (e) => {
    if (e.data && e.data.byteLength === 320 * 200 * 4) {
      status.textContent = "";
      ctx.putImageData(
        new ImageData(new Uint8ClampedArray(e.data.buffer), 320, 200), 0, 0);
    } else if (typeof e.data === "string") {
      status.textContent = e.data; // worker status/error text
    }
  };
  worker.onerror = (e) => { status.textContent = `worker error: ${e.message}`; };
  worker.postMessage({ files, input: inputSab },
    Object.values(files).map((u8) => u8.buffer));
  status.textContent = "starting engine…";

  const pushKey = (code, pressed) => {
    const id = KEY_CODES.indexOf(KEY_ALIASES[code] ?? code);
    if (id < 0) return false;
    const seq = Atomics.load(input, 0);
    Atomics.store(input, 1 + (seq % RING_CAPACITY), (id << 1) | (pressed ? 1 : 0));
    Atomics.store(input, 0, seq + 1);
    return true;
  };
  window.addEventListener("keydown", (e) => {
    if (pushKey(e.code, true)) e.preventDefault();
  });
  window.addEventListener("keyup", (e) => {
    if (pushKey(e.code, false)) e.preventDefault();
  });
}

boot().catch((e) => { status.textContent = String(e); });
