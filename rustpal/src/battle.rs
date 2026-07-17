//! Battle system core (port of SDLPAL battle.c). BRING-UP STUB — the real
//! port replaces this file; the signatures below are the stable contract
//! other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

/// BATTLERESULT.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BattleResult {
    Won,
    Lost,
    Fleed,
    Terminated,
    OnGoing,
    PreBattle,
    Pause,
}

impl Engine {
    /// PAL_StartBattle.
    pub fn start_battle(&mut self, _enemy_team: u16, _is_boss: bool) -> BattleResult {
        // STUB: real implementation ports battle.c.
        BattleResult::Won
    }
}
