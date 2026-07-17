// Engine worker: receives the game data + input SharedArrayBuffer from the
// main thread, installs them on the global scope (read by the Rust side),
// then runs the whole synchronous engine. web_run() blocks this worker for
// the lifetime of the game — frames go out via postMessage, input comes in
// via Atomics on PAL_INPUT.

importScripts("pkg/rustpal.js");

self.onmessage = async (e) => {
  const { files, input, audio, audioRate } = e.data;
  self.PAL_FILES = files;
  self.PAL_INPUT = input;
  if (audio) {
    self.PAL_AUDIO = audio;
    self.PAL_AUDIO_RATE = audioRate;
  }
  console.log(`worker: audio=${audio ? audio.byteLength : "none"} rate=${audioRate}`);
  try {
    await wasm_bindgen({ module_or_path: "pkg/rustpal_bg.wasm" });
    postMessage("engine loaded — booting…");
    wasm_bindgen.web_run();
  } catch (err) {
    postMessage(`engine failed: ${err}`);
    throw err;
  }
};
