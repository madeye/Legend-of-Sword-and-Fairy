//! Web (wasm32) backend: runs the whole synchronous engine inside a Web
//! Worker, where blocking the thread is allowed.
//!
//! The contract with the JS side (web/main.js + web/worker.js):
//! - `PAL_FILES` on the worker global scope: object mapping upper-case DOS
//!   file names to `Uint8Array`s (read by `data::DataDir`).
//! - `PAL_INPUT` on the worker global scope: a `SharedArrayBuffer` viewed as
//!   an `Int32Array` ring buffer of key events. Slot 0 is the number of
//!   events ever written; event `i` lives at `1 + i % capacity` encoded as
//!   `(key_index << 1) | pressed`, where `key_index` indexes `WEB_KEYS`.
//! - Each presented frame is posted to the main thread as an RGBA
//!   `Uint8Array` (320x200x4) for `putImageData`.

use std::io;

use js_sys::{Atomics, Int32Array, SharedArrayBuffer, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use winit::keyboard::KeyCode;

use crate::game_loop::PalColor;
use crate::surface::{Surface, SCREEN_H, SCREEN_W};

/// Keys forwarded from the browser. The order MUST match `KEY_CODES` in
/// web/main.js: the main thread sends indexes into this table.
pub const WEB_KEYS: &[KeyCode] = &[
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Escape,
    KeyCode::Insert,
    KeyCode::Enter,
    KeyCode::Space,
    KeyCode::ControlLeft,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::KeyR,
    KeyCode::KeyA,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyW,
    KeyCode::KeyQ,
    KeyCode::KeyF,
    KeyCode::KeyS,
];

fn worker_scope() -> web_sys::DedicatedWorkerGlobalScope {
    js_sys::global().unchecked_into()
}

/// The web replacement for game_loop's winit/pixels `Video`: same interface
/// (`new`, `pump`, `present`, `close_requested`), backed by the SAB input
/// ring and postMessage frame output.
pub struct Video {
    input: Int32Array,
    read_seq: u32,
    rgba: Vec<u8>,
}

impl Video {
    pub fn new() -> io::Result<Video> {
        let sab = js_sys::Reflect::get(&js_sys::global(), &"PAL_INPUT".into())
            .map_err(|_| io::Error::other("no global scope"))?;
        if sab.is_undefined() || sab.is_null() {
            return Err(io::Error::other(
                "PAL_INPUT not set on the worker global scope",
            ));
        }
        Ok(Video {
            input: Int32Array::new(&sab),
            read_seq: 0,
            rgba: vec![0; SCREEN_W * SCREEN_H * 4],
        })
    }

    /// Drain key events written by the main thread since the last pump.
    /// Ring layout: [0] = write counter (main thread), [1] = read counter
    /// (mirrored here for debugging), [2..] = event slots.
    pub fn pump(&mut self) -> Vec<(KeyCode, bool)> {
        let mut out = Vec::new();
        let capacity = self.input.length().saturating_sub(2);
        if capacity == 0 {
            return out;
        }
        let write = Atomics::load(&self.input, 0).unwrap_or(0) as u32;
        // If we fell behind by more than the ring size, skip lost events.
        if write.wrapping_sub(self.read_seq) > capacity {
            self.read_seq = write.wrapping_sub(capacity);
        }
        while self.read_seq != write {
            let slot = 2 + (self.read_seq % capacity);
            let v = Atomics::load(&self.input, slot).unwrap_or(0);
            let pressed = v & 1 != 0;
            if let Some(&code) = WEB_KEYS.get((v >> 1) as usize) {
                out.push((code, pressed));
            }
            self.read_seq = self.read_seq.wrapping_add(1);
        }
        let _ = Atomics::store(&self.input, 1, self.read_seq as i32);
        out
    }

    /// Present a frame: convert to RGBA and post it to the main thread.
    pub fn present(
        &mut self,
        surf: &Surface,
        palette: &[PalColor; 256],
        shake: Option<(u16, u16)>,
    ) {
        crate::game_loop::render_rgba(surf, palette, shake, &mut self.rgba);
        // Copies out of wasm memory; the receiving side takes ownership.
        let frame = Uint8Array::from(&self.rgba[..]);
        let _ = worker_scope().post_message(&frame);
    }

    /// The browser tab closing just kills the worker; never reported.
    pub fn close_requested(&self) -> bool {
        false
    }
}

/// Store a saved game: update the in-worker `PAL_FILES` map so loads in this
/// session see it, and post it to the main thread, which persists it in
/// localStorage (re-injected into PAL_FILES on the next boot).
pub fn store_save(slot: i32, data: &[u8]) {
    let arr = Uint8Array::from(data);
    if let Ok(files) = js_sys::Reflect::get(&js_sys::global(), &"PAL_FILES".into()) {
        let _ = js_sys::Reflect::set(&files, &format!("{slot}.RPG").into(), &arr);
    }
    let msg = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&msg, &"palSave".into(), &JsValue::from(slot));
    let _ = js_sys::Reflect::set(&msg, &"data".into(), &arr);
    let _ = worker_scope().post_message(&msg);
}

thread_local! {
    /// Dummy SAB used purely as an `Atomics.wait` sleep timer.
    static SLEEP_CELL: Int32Array = Int32Array::new(&SharedArrayBuffer::new(4));
}

/// Block the worker for `ms` milliseconds without burning CPU.
pub fn sleep_ms(ms: u64) {
    SLEEP_CELL.with(|cell| {
        // The cell always holds 0, so this waits until the timeout elapses.
        let _ = Atomics::wait_with_timeout(cell, 0, 0, ms as f64);
    });
}

/// Worker entry point: boot the engine and run the full game, blocking this
/// worker thread for the lifetime of the game (exactly like native main()).
#[wasm_bindgen]
pub fn web_run() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("rustpal panic: {info}").into());
    }));
    match crate::game_loop::Engine::new(false) {
        Ok(mut engine) => engine.run(),
        Err(e) => {
            web_sys::console::error_1(&format!("rustpal: failed to start: {e}").into());
        }
    }
}
