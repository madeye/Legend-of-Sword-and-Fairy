//! Scene engine (port of SDLPAL scene.c). BRING-UP STUB — the real port
//! replaces this file; the signatures below are the stable contract other
//! modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

/// Module-private state for the scene engine.
#[derive(Default)]
pub struct SceneState {}

impl Engine {
    /// PAL_MakeScene: draw the entire scene (map + sprites) to self.screen.
    pub fn make_scene(&mut self) {
        // STUB: real implementation ports PAL_MakeScene.
    }

    /// PAL_UpdateParty: walk the party according to input.
    pub fn update_party(&mut self) {
        // STUB: real implementation ports PAL_UpdateParty.
    }

    /// PAL_NPCWalkOneStep.
    pub fn npc_walk_one_step(&mut self, _event_object_id: u16, _speed: i32) {
        // STUB: real implementation ports PAL_NPCWalkOneStep.
    }

    /// PAL_CheckObstacle.
    pub fn check_obstacle(
        &self,
        _pos: (i32, i32),
        _check_event_objects: bool,
        _self_object: u16,
    ) -> bool {
        // STUB: real implementation ports PAL_CheckObstacle.
        false
    }
}
