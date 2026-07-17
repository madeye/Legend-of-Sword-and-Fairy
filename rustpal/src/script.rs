//! Script interpreter (port of SDLPAL script.c). BRING-UP STUB — the real
//! port replaces this file; the signatures below are the stable contract
//! other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

/// Module-private interpreter state (script.c statics).
#[derive(Default)]
pub struct ScriptState {
    /// g_fScriptSuccess.
    pub script_success: bool,
    /// g_iCurEquipPart.
    pub cur_equip_part: i32,
}

impl Engine {
    /// PAL_RunTriggerScript. Returns the script entry to save back.
    pub fn run_trigger_script(&mut self, script_entry: u16, _event_object_id: u16) -> u16 {
        // STUB: real implementation ports PAL_RunTriggerScript.
        script_entry
    }

    /// PAL_RunAutoScript. Returns the script entry to save back.
    pub fn run_auto_script(&mut self, script_entry: u16, _event_object_id: u16) -> u16 {
        // STUB: real implementation ports PAL_RunAutoScript.
        script_entry
    }

    /// PAL_UpdateEquipments (global.c, but needs the interpreter).
    pub fn update_equipments(&mut self) {
        // STUB: iterates equipped items running their equip scripts.
    }

    /// PAL_AddPoisonForPlayer (global.c, but needs the interpreter).
    pub fn add_poison_for_player(&mut self, _player_role: u16, _poison_id: u16) {
        // STUB: real implementation runs the poison script.
    }
}
