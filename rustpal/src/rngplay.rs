//! RNG animated cutscene playback (port of SDLPAL rngplay.c). BRING-UP STUB
//! — the real port replaces this file; the signatures below are the stable
//! contract other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

impl Engine {
    /// PAL_RNGPlay: play frames [start_frame, end_frame] of RNG animation
    /// `rng_num` at the given speed.
    pub fn rng_play(&mut self, _rng_num: u16, _start_frame: i32, _end_frame: i32, _speed: i32) {
        // STUB
    }
}
