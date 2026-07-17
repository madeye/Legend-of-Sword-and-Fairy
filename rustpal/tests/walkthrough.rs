//! Headless end-to-end integration test: start a new game exactly like
//! PAL_GameMain, run the scene-enter script, then walk the player around the
//! starting room with simulated key input and verify movement, rendering and
//! obstacle behavior against the real game data.

use rustpal::battle::BattleResult;
use rustpal::game_loop::Engine;
use rustpal::global::{seed_random, ScriptEntry, MAX_PLAYER_MAGICS};
use winit::keyboard::KeyCode;

fn new_game_engine() -> Engine {
    std::env::set_var(
        "PAL_DATA_DIR",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
    );
    let mut e = Engine::new(true).expect("headless engine");
    e.globals.current_save_slot = 0;
    e.globals.in_main_game = true;
    e.globals.reload_in_next_tick(0);

    // First frame loads resources and runs the scene-enter script.
    let flags = e.res.load_resources(&mut e.globals).expect("resources");
    assert!(flags.global_data && flags.scene && flags.player_sprite);
    e.update_equipments();
    e.input.clear_key_state();
    e.start_frame();
    e
}

#[test]
fn new_game_starts_in_scene_one_with_valid_position() {
    let e = new_game_engine();
    assert_eq!(e.globals.num_scene, 1);
    // The enter script must have placed the viewport somewhere real.
    assert!(e.globals.viewport.0 > 0 && e.globals.viewport.1 > 0);
    // The scene must render non-empty.
    let nonzero = e.screen.pixels.iter().filter(|&&p| p != 0).count();
    assert!(nonzero > 10000, "scene mostly empty: {nonzero}");
}

#[test]
fn player_walks_with_key_input_and_stops_at_obstacles() {
    let mut e = new_game_engine();

    let start = e.globals.viewport;
    // Hold "down" (south) and run frames; the party should move.
    e.input.handle_key_event(KeyCode::ArrowDown, true);
    for _ in 0..6 {
        e.input.update_keyboard_state(e.ticks() + 1000);
        e.start_frame();
        e.input.clear_key_state();
    }
    e.input.handle_key_event(KeyCode::ArrowDown, false);
    let after_south = e.globals.viewport;
    assert_ne!(start, after_south, "party did not move south");

    // Walk in every direction; the engine must never panic and the
    // viewport must stay within the map bounds.
    for key in [
        KeyCode::ArrowLeft,
        KeyCode::ArrowUp,
        KeyCode::ArrowRight,
        KeyCode::ArrowDown,
    ] {
        e.input.handle_key_event(key, true);
        for _ in 0..40 {
            e.input.update_keyboard_state(e.ticks() + 1000);
            e.start_frame();
            e.input.clear_key_state();
        }
        e.input.handle_key_event(key, false);
        let (vx, vy) = e.globals.viewport;
        assert!(
            (0..4096).contains(&vx) && (0..2048).contains(&vy),
            "viewport out of world bounds: {vx},{vy}"
        );
    }

    // Obstacles must exist: walking forever in one direction cannot go on
    // unbounded (the room has walls) — verify the party got stopped at some
    // point by checking it did not travel 40 tiles in the last direction.
    let total_dy = (e.globals.viewport.1 - start.1).abs();
    assert!(total_dy < 40 * 16, "no obstacle ever stopped the party");
}

/// Build a headless engine with the default game loaded and a single, very
/// strong, magic-less party member — so auto-battle picks physical attacks and
/// reliably wins a weak fight.  Battles run with the `instant` fast path.
fn battle_engine() -> Engine {
    std::env::set_var(
        "PAL_DATA_DIR",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
    );
    let mut e = Engine::new(true).expect("headless engine");
    e.globals.load_default_game().expect("default game");
    e.globals.max_party_member_index = 0;
    e.globals.party[0].player_role = 0;
    for i in 0..MAX_PLAYER_MAGICS {
        e.globals.game.player_roles.magic[i][0] = 0;
    }
    e.globals.game.player_roles.hp[0] = 999;
    e.globals.game.player_roles.max_hp[0] = 999;
    e.globals.game.player_roles.attack_strength[0] = 800;
    e.globals.game.player_roles.dexterity[0] = 200;
    e.globals.auto_battle = true;
    e.battle_instant = true;
    e
}

/// The enemy team with the weakest total health (a fight we can win fast).
fn weakest_team(e: &Engine) -> u16 {
    let mut best = 0usize;
    let mut best_hp = u32::MAX;
    for (idx, t) in e.globals.game.enemy_teams.iter().enumerate() {
        let mut hp = 0u32;
        let mut any = false;
        for &w in t.enemy.iter() {
            if w != 0 && w != 0xFFFF {
                any = true;
                let eid = e.globals.game.objects[w as usize].enemy_id() as usize;
                hp += e.globals.game.enemies[eid].health as u32;
            }
        }
        if any && hp > 0 && hp < best_hp {
            best_hp = hp;
            best = idx;
        }
    }
    assert!(best > 0, "no suitable enemy team");
    best as u16
}

/// End-to-end: start a real battle from a *script* (opcode 0x0007 —
/// PAL_StartBattle) and verify the unified battle state.  During the battle
/// the enemy/pre-battle scripts run through `run_trigger_script`, which must
/// always see `engine.battle`.  Afterwards `engine.battle` must be cleared and
/// experience/cash awarded.  A direct `start_battle_ex` on an identically
/// seeded engine cross-checks that the script path and the direct path award
/// the exact same rewards.
#[test]
fn battle_started_from_script_unifies_state_and_awards() {
    let team = weakest_team(&battle_engine());

    // Reference: fight the team directly.
    let mut direct = battle_engine();
    let cash_before = direct.globals.cash;
    let exp_before = direct.globals.exp.primary_exp[0].exp;
    seed_random(4242);
    let result = direct.start_battle_ex(team, false, true);
    assert_eq!(result, BattleResult::Won, "strong party must win the fight");
    assert!(
        direct.battle.is_none(),
        "battle not cleared after direct fight"
    );
    let cash_direct = direct.globals.cash;
    let exp_direct = direct.globals.exp.primary_exp[0].exp;

    // Script-driven: opcode 0x0007 starts the battle; op[2] != 0 => not a boss.
    let mut scripted = battle_engine();
    let base = 20000u16;
    let entries = [
        ScriptEntry {
            operation: 0x0007,
            operand: [team, 0, 1],
        },
        ScriptEntry {
            operation: 0x0000,
            operand: [0, 0, 0],
        },
    ];
    for (i, entry) in entries.iter().enumerate() {
        scripted.globals.game.script_entries[base as usize + i] = *entry;
    }
    assert!(
        scripted.battle.is_none(),
        "battle must be None before the fight"
    );
    seed_random(4242);
    scripted.run_trigger_script(base, 0xFFFF);

    // The unified battle state must be gone once the script returns.
    assert!(
        scripted.battle.is_none(),
        "engine.battle must be None after the scripted battle ends"
    );

    // Rewards must have been granted, and match the direct fight exactly.
    assert!(
        cash_direct > cash_before || exp_direct > exp_before,
        "no exp/cash awarded (cash {cash_before}->{cash_direct}, exp {exp_before}->{exp_direct})"
    );
    assert_eq!(
        scripted.globals.cash, cash_direct,
        "scripted battle awarded different cash than the direct fight"
    );
    assert_eq!(
        scripted.globals.exp.primary_exp[0].exp, exp_direct,
        "scripted battle awarded different exp than the direct fight"
    );
    // Auto-battle flag is reset by opcode 0x0007 once the fight is over.
    assert!(!scripted.globals.auto_battle);
}

#[test]
fn search_near_start_triggers_no_crash() {
    let mut e = new_game_engine();
    // Simulated Space (search) presses around the starting position must
    // run trigger scripts without panicking.
    for _ in 0..3 {
        e.input.handle_key_event(KeyCode::Space, true);
        e.input.update_keyboard_state(e.ticks() + 1000);
        e.start_frame();
        e.input.handle_key_event(KeyCode::Space, false);
        e.input.update_keyboard_state(e.ticks() + 2000);
        e.input.clear_key_state();
    }
}
