//! Game update loop (port of SDLPAL play.c). BRING-UP STUB — the real port
//! replaces this file; the signatures below are the stable contract other
//! modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

/// Module-private state for the play loop.
#[derive(Default)]
pub struct PlayState {}

impl Engine {
    /// PAL_GameUpdate: run auto scripts / update party & event objects.
    /// `trigger` mirrors the fTrigger parameter.
    pub fn game_update(&mut self, _trigger: bool) {
        // STUB: real implementation ports PAL_GameUpdate.
    }

    /// PAL_StartFrame: process one frame in the main game.
    pub fn start_frame(&mut self) {
        // STUB: real implementation ports PAL_StartFrame.
    }
}
