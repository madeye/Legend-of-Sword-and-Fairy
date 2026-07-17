//! Ending sequences and FBP full-screen picture helpers (port of SDLPAL
//! ending.c). BRING-UP STUB — the real port replaces this file; the
//! signatures below are the stable contract other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

impl Engine {
    /// PAL_EndingSetEffectSprite.
    pub fn ending_set_effect_sprite(&mut self, _sprite_num: u16) {
        // STUB
    }

    /// PAL_ShowFBP: draw FBP.MKF chunk `chunk_num` to the screen with the
    /// given fade (wave) effect.
    pub fn show_fbp(&mut self, _chunk_num: u16, _fade: u16) {
        // STUB
    }

    /// PAL_ScrollFBP.
    pub fn scroll_fbp(&mut self, _chunk_num: u16, _scroll_speed: u16, _scroll_down: bool) {
        // STUB
    }

    /// PAL_EndingAnimation.
    pub fn ending_animation(&mut self) {
        // STUB
    }

    /// PAL_EndingScreen.
    pub fn ending_screen(&mut self) {
        // STUB
    }
}
