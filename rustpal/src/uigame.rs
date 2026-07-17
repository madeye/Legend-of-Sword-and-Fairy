//! In-game and opening menus (port of SDLPAL uigame.c). BRING-UP STUB — the
//! real port replaces this file; the signatures below are the stable
//! contract other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

impl Engine {
    /// PAL_OpeningMenu: returns the chosen save slot (0 = new game).
    pub fn opening_menu(&mut self) -> i32 {
        // STUB: real implementation ports PAL_OpeningMenu.
        0
    }
}
