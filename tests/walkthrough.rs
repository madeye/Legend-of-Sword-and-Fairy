//! Headless end-to-end integration test: start a new game exactly like
//! PAL_GameMain, run the scene-enter script, then walk the player around the
//! starting room with simulated key input and verify movement, rendering and
//! obstacle behavior against the real game data.

use rustpal::battle::BattleResult;
use rustpal::game_loop::Engine;
use rustpal::global::{seed_random, EventObject, ScriptEntry, MAX_PLAYER_MAGICS, MAX_PLAYER_ROLES};
use std::collections::{HashMap, HashSet, VecDeque};
use winit::keyboard::KeyCode;

fn new_game_engine() -> Engine {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
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
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
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
    assert_eq!(
        scripted.battle_records.len(),
        1,
        "scripted 0x0007 must record exactly one real battle"
    );
    assert_eq!(scripted.battle_records[0].enemy_team, team);
    assert_eq!(scripted.battle_records[0].result, BattleResult::Won);
    assert!(
        direct.battle_records.len() == 1 && direct.battle_records[0].result == BattleResult::Won,
        "direct fight must also go through start_battle_ex"
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

/// 扬州 比武招亲 (enemy team 188) is the story gate into 火麒麟洞. Losing
/// it runs opcode `0x004E` and reloads the new-game slot, which is the
/// 余杭-inn wipe seen in stalled playthroughs.
#[test]
fn yangzhou_contest_boss_is_winnable_with_instant_auto_battle() {
    let mut e = battle_engine();
    e.globals.max_party_member_index = 2;
    e.globals.party[0].player_role = 0;
    e.globals.party[1].player_role = 1;
    e.globals.party[2].player_role = 2;
    seed_random(4242);
    let result = e.start_battle_ex(188, true, true);
    assert_eq!(
        result,
        BattleResult::Won,
        "比武招亲 boss (team 188) must be winnable under instant auto-battle"
    );
    assert!(e.battle.is_none());
    assert_eq!(e.battle_records.last().map(|r| r.enemy_team), Some(188));
}

fn arm_like_autoplay(e: &mut Engine) {
    let roles = &mut e.globals.game.player_roles;
    for role in 0..MAX_PLAYER_ROLES {
        roles.max_hp[role] = roles.max_hp[role].max(9999);
        roles.hp[role] = roles.max_hp[role];
        roles.max_mp[role] = roles.max_mp[role].max(9999);
        roles.mp[role] = roles.max_mp[role];
        roles.attack_strength[role] = roles.attack_strength[role].max(2000);
        roles.magic_strength[role] = roles.magic_strength[role].max(2000);
        roles.defense[role] = roles.defense[role].max(400);
        roles.dexterity[role] = roles.dexterity[role].max(300);
        roles.poison_resistance[role] = 100;
    }
}

/// The live playthrough arms every role before each frame, then the 比武
/// enter script (0x0075 + 0x0007) starts team 188. That path must win; a
/// loss reloads slot 0 and dumps the story back to 余杭.
#[test]
fn yangzhou_contest_via_story_script_wins_with_autoplay_party() {
    let mut e = battle_engine();
    arm_like_autoplay(&mut e);
    e.globals.max_party_member_index = 1;
    e.globals.party[0].player_role = 0;
    e.globals.party[1].player_role = 1;
    e.globals.in_battle = false;
    seed_random(4242);
    // 28509: 0x0075 set party 李逍遥/赵灵儿/林月如, heal, then 0x0007 team 188.
    e.run_trigger_script(28509, 0xFFFF);
    let rec = e
        .battle_records
        .last()
        .expect("0x0007 must record team 188");
    assert_eq!(rec.enemy_team, 188);
    assert_eq!(
        rec.result,
        BattleResult::Won,
        "story 0x0007 path for 比武 must win under autoplay stats; got {:?}",
        rec.result
    );
}

const QIXING_WALK_DIRS: [(KeyCode, (i32, i32)); 4] = [
    (KeyCode::ArrowUp, (16, -8)),
    (KeyCode::ArrowRight, (16, 8)),
    (KeyCode::ArrowDown, (-16, 8)),
    (KeyCode::ArrowLeft, (-16, -8)),
];

fn player_pos(e: &Engine) -> (i32, i32) {
    (
        e.globals.viewport.0 + e.globals.partyoffset.0,
        e.globals.viewport.1 + e.globals.partyoffset.1,
    )
}

fn event_pos(event: EventObject) -> (i32, i32) {
    (event.x as i16 as i32, event.y as i16 as i32)
}

fn can_search_event_from(position: (i32, i32), event: (i32, i32), mode: u16) -> bool {
    let limit = (mode as usize * 6).saturating_sub(4).min(13);
    for direction in 0..4 {
        let (x_offset, y_offset) = match direction {
            0 => (-16, 8),
            1 => (-16, -8),
            2 => (16, -8),
            _ => (16, 8),
        };
        let (mut x, mut y) = position;
        let mut range = [(0i32, 0i32); 13];
        range[0] = position;
        for index in 0..4 {
            range[index * 3 + 1] = (x + x_offset, y + y_offset);
            range[index * 3 + 2] = (x, y + y_offset * 2);
            range[index * 3 + 3] = (x + x_offset * 2, y);
            x += x_offset;
            y += y_offset;
        }
        if range[..limit].contains(&event) {
            return true;
        }
    }
    false
}

fn path_to_event_on_collision(e: &Engine, event_id: u16) -> Option<VecDeque<KeyCode>> {
    let event = *e.globals.game.event_objects.get(event_id as usize - 1)?;
    let start = player_pos(e);
    let goal = event_pos(event);
    let reached = |position| can_search_event_from(position, goal, event.trigger_mode.max(1));
    if reached(start) {
        return Some(VecDeque::new());
    }

    let try_path = |check_event_objects: bool| -> Option<VecDeque<KeyCode>> {
        let mut queue = VecDeque::from([start]);
        let mut previous: HashMap<(i32, i32), ((i32, i32), KeyCode)> = HashMap::new();
        let mut seen = HashSet::from([start]);
        let mut found = None;
        while let Some(position) = queue.pop_front() {
            if reached(position) {
                found = Some(position);
                break;
            }
            if seen.len() > 200_000 {
                break;
            }
            for &(key, delta) in &QIXING_WALK_DIRS {
                let next = (position.0 + delta.0, position.1 + delta.1);
                if !(0..8192).contains(&next.0)
                    || !(0..4096).contains(&next.1)
                    || seen.contains(&next)
                    || e.check_obstacle_with_range(next, check_event_objects, 0, true)
                {
                    continue;
                }
                seen.insert(next);
                previous.insert(next, (position, key));
                queue.push_back(next);
            }
        }
        let mut at = found?;
        let mut reversed = Vec::new();
        while at != start {
            let &(before, key) = previous.get(&at)?;
            reversed.push(key);
            at = before;
        }
        reversed.reverse();
        Some(reversed.into())
    };

    try_path(true).or_else(|| try_path(false))
}

fn walk_keys(e: &mut Engine, path: &VecDeque<KeyCode>) -> u32 {
    let mut tile_moves = 0u32;
    for &key in path {
        let before = player_pos(e);
        e.input.handle_key_event(key, true);
        e.input.update_keyboard_state(e.ticks() + 1000);
        e.start_frame();
        e.input.handle_key_event(key, false);
        e.input.update_keyboard_state(e.ticks() + 2000);
        e.input.clear_key_state();
        if player_pos(e) != before {
            tile_moves += 1;
        } else {
            eprintln!("walk stalled at {before:?} key={key:?}");
        }
    }
    tile_moves
}

/// 锁妖塔 七星磐龙阵: after 镇狱明王, the real warp script 29171 (opcode
/// `0x0059`) enters scene 144. The enter script places the party, then each
/// of the seven pillar objects is reached on map 185's live collision before
/// its `0x0007` 神龙 fight. Completing the last pillar warps to scene 149.
#[test]
fn qixing_dragon_pillars_are_seven_scripted_battles_after_mingwang() {
    let mut e = battle_engine();
    arm_like_autoplay(&mut e);
    e.globals.max_party_member_index = 2;
    e.globals.party[0].player_role = 0;
    e.globals.party[1].player_role = 1;
    e.globals.party[2].player_role = 2;
    seed_random(4242);

    e.run_trigger_script(26079, 2422);
    assert!(
        e.battle_records
            .iter()
            .any(|r| r.enemy_team == 43 && r.result == BattleResult::Won),
        "镇狱明王 (team 43) must be won through opcode 0x0007"
    );
    assert!(
        e.globals.game.event_objects[2422 - 1].state <= 0,
        "镇狱明王 event must be cleared after the fight"
    );

    e.run_trigger_script(29171, 0xFFFF);
    let flags = e.res.load_resources(&mut e.globals).expect("load 七星 map");
    assert!(flags.scene, "scene 144 map must load after 0x0059");
    assert_eq!(
        e.globals.num_scene, 144,
        "script 29171 must enter 七星磐龙阵 via opcode 0x0059"
    );
    assert!(
        e.res.map.as_ref().is_some_and(|map| map.num > 0),
        "七星 map must be the real MAP.MKF chunk"
    );

    let pos_before_enter = player_pos(&e);
    e.start_frame();
    let spawn = player_pos(&e);
    let spawn_viewport = e.globals.viewport;
    let spawn_offset = e.globals.partyoffset;
    assert_eq!(
        spawn,
        (0x25 * 32, 0x11 * 16),
        "scene-enter 0x0046 must place the party on the 七星 map"
    );
    assert_ne!(
        pos_before_enter, spawn,
        "enter script must move the party off the previous scene's coordinates"
    );
    assert_eq!(e.globals.num_scene, 144);

    let start_scene = e.globals.num_scene;
    let mut stopped_by_block = false;
    for key in [
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowUp,
        KeyCode::ArrowRight,
    ] {
        let before = player_pos(&e);
        e.input.handle_key_event(key, true);
        for _ in 0..48 {
            let prev = player_pos(&e);
            e.input.update_keyboard_state(e.ticks() + 1000);
            e.start_frame();
            e.input.clear_key_state();
            if e.globals.num_scene != start_scene {
                break;
            }
            if player_pos(&e) == prev && prev != before {
                stopped_by_block = true;
                break;
            }
        }
        e.input.handle_key_event(key, false);
        let after = player_pos(&e);
        let dist = (after.0 - before.0).abs() + (after.1 - before.1).abs() * 2;
        if dist < 48 * 16 {
            stopped_by_block = true;
        }
        if e.globals.num_scene != start_scene {
            break;
        }
    }
    assert!(
        stopped_by_block,
        "holding a direction on 七星磐龙阵 must be stopped by blocked tiles"
    );

    // Restore the enter-script spawn so each pillar is reached from the
    // real 0x0046 landing, not from a wall-probe tile.
    e.globals.viewport = spawn_viewport;
    e.globals.partyoffset = spawn_offset;
    assert_eq!(player_pos(&e), spawn);
    assert_eq!(e.globals.num_scene, 144);

    const PILLARS: [(u16, u16); 7] = [
        (2466, 305),
        (2467, 306),
        (2468, 307),
        (2469, 308),
        (2470, 309),
        (2471, 310),
        (2472, 311),
    ];
    let mut tile_moves = 0u32;
    let mut longest_path = 0usize;
    for (event_id, team) in PILLARS {
        arm_like_autoplay(&mut e);
        e.globals.auto_battle = true;
        let before_walk = player_pos(&e);
        let goal = event_pos(e.globals.game.event_objects[event_id as usize - 1]);
        let path = path_to_event_on_collision(&e, event_id)
            .unwrap_or_else(|| panic!("no collision path to 七星 pillar {event_id}"));
        longest_path = longest_path.max(path.len());
        eprintln!(
            "pillar {event_id} team={team} from={before_walk:?} goal={goal:?} path={}",
            path.len()
        );
        assert!(
            !path.is_empty() || can_search_event_from(before_walk, goal, 2),
            "pillar {event_id} must be walked or already in search range"
        );
        let moved = walk_keys(&mut e, &path);
        tile_moves += moved;
        eprintln!(
            "pillar {event_id} after={:?} moved={moved} in_range={}",
            player_pos(&e),
            can_search_event_from(player_pos(&e), goal, 2)
        );
        assert_eq!(
            e.globals.num_scene, 144,
            "walking to pillar {event_id} must not assign a later scene"
        );
        assert!(
            can_search_event_from(player_pos(&e), goal, 2),
            "party must be in search range of pillar {event_id} before 0x0007; at {:?} goal {:?} path {} moved {moved}",
            player_pos(&e),
            goal,
            path.len()
        );
        let script = e.globals.game.event_objects[event_id as usize - 1].trigger_script;
        e.run_trigger_script(script, event_id);
        let rec = e
            .battle_records
            .last()
            .expect("pillar 0x0007 must record a battle");
        assert_eq!(rec.enemy_team, team, "pillar event {event_id}");
        assert_eq!(rec.result, BattleResult::Won, "pillar team {team}");
        assert!(e.battle.is_none());
    }

    assert!(
        longest_path >= 8,
        "at least one pillar must require a real collision walk, longest path={longest_path}"
    );
    assert!(
        tile_moves >= 16,
        "expected many intra-maze tile moves on scene 144, got {tile_moves}"
    );

    let dragon_wins = e
        .battle_records
        .iter()
        .filter(|r| (305..=311).contains(&r.enemy_team) && r.result == BattleResult::Won)
        .count();
    assert_eq!(dragon_wins, 7);
    assert_eq!(
        e.globals.num_scene, 149,
        "the seventh pillar must leave 七星磐龙阵 via opcode 0x0059"
    );
}

/// 仙灵岛外围 (scene 16) is reached by the real boat teleport script
/// (opcode `0x0059`), then walked on the live blocked-tile map. The test
/// never writes `num_scene` itself.
#[test]
fn maze_island_walks_on_real_collision_without_assigning_scene() {
    let mut e = new_game_engine();
    e.battle_instant = true;
    e.globals.auto_battle = true;

    // Script 8452 is the DOS data's island-shore teleport: set party
    // position, then 0x0059 to scene 16. Running it is the same path the
    // story boat uses, not a test-only scene poke.
    const ISLAND_SHORE_TELEPORT: u16 = 8452;
    e.run_trigger_script(ISLAND_SHORE_TELEPORT, 0xFFFF);
    assert_eq!(
        e.globals.num_scene, 16,
        "island teleport script must enter scene 16 via opcode 0x0059"
    );
    let flags = e
        .res
        .load_resources(&mut e.globals)
        .expect("load island map");
    assert!(flags.scene, "scene resources must load after 0x0059");
    assert!(
        e.res.map.as_ref().is_some_and(|map| map.num > 0),
        "island map must be the real MAP.MKF chunk"
    );

    let start_pos = (
        e.globals.viewport.0 + e.globals.partyoffset.0,
        e.globals.viewport.1 + e.globals.partyoffset.1,
    );
    let start_scene = e.globals.num_scene;
    let mut tile_moves = 0u32;
    let mut stopped_by_block = false;

    'dirs: for key in [
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowUp,
        KeyCode::ArrowRight,
    ] {
        let before = (
            e.globals.viewport.0 + e.globals.partyoffset.0,
            e.globals.viewport.1 + e.globals.partyoffset.1,
        );
        e.input.handle_key_event(key, true);
        for _ in 0..48 {
            let prev = (
                e.globals.viewport.0 + e.globals.partyoffset.0,
                e.globals.viewport.1 + e.globals.partyoffset.1,
            );
            e.input.update_keyboard_state(e.ticks() + 1000);
            e.start_frame();
            e.input.clear_key_state();
            let now = (
                e.globals.viewport.0 + e.globals.partyoffset.0,
                e.globals.viewport.1 + e.globals.partyoffset.1,
            );
            if now != prev {
                tile_moves += 1;
            }
            // A live trigger may change the scene. This test never writes
            // `num_scene`; a scripted leave is the authentic exit path.
            if e.globals.num_scene != start_scene {
                e.input.handle_key_event(key, false);
                break 'dirs;
            }
        }
        e.input.handle_key_event(key, false);
        let after = (
            e.globals.viewport.0 + e.globals.partyoffset.0,
            e.globals.viewport.1 + e.globals.partyoffset.1,
        );
        let dist = (after.0 - before.0).abs() + (after.1 - before.1).abs() * 2;
        // 48 frames of walking at 16px/step would cover 48 tiles if unbounded.
        if dist < 48 * 16 {
            stopped_by_block = true;
        }
    }

    let end_pos = (
        e.globals.viewport.0 + e.globals.partyoffset.0,
        e.globals.viewport.1 + e.globals.partyoffset.1,
    );
    assert_ne!(start_pos, end_pos, "party never moved on the island map");
    assert!(
        tile_moves >= 16,
        "expected many intra-maze tile moves, got {tile_moves}"
    );
    assert!(
        stopped_by_block || e.globals.num_scene != start_scene,
        "holding a direction was never stopped by blocked tiles"
    );
    // The test body never assigns `num_scene`. Any leave is the island's
    // own teleport/trigger script.
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
