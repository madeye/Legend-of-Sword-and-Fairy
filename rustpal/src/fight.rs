//! Battle combat logic (port of SDLPAL `fight.c`, classic `PAL_CLASSIC` mode).
//!
//! Follows the two-argument style documented in `battle.rs`: every routine
//! takes `(engine: &mut Engine, battle: &mut Battle)`.  The damage math keeps
//! every constant, `RandomLong` range and `(SHORT)`/`(WORD)` cast exact.
//!
//! Rendering-only animation helpers short-circuit when `battle.instant` is set
//! (they only move sprites, tint colors, play sounds and wait — they never
//! change HP/health/state), so a headless auto-battle runs the full combat
//! logic with no real delays and no video device.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::battle::{
    copy_scene_to_screen, fade_scene, make_scene, player_escape, Battle, BattleActionType,
    BattleMenuState, BattlePhase, BattleResult, BattleUiState, FighterState, MAX_ACTIONQUEUE_ITEMS,
};
use crate::game_loop::{Engine, BATTLE_FRAME_TIME};
use crate::global::{
    random_float, random_long, Globals, ITEMFLAG_APPLY_TO_ALL, ITEMFLAG_CONSUMING,
    MAGICFLAG_APPLY_TO_ALL, MAGICFLAG_USABLE_TO_ENEMY, MAGICTYPE_APPLYTOPARTY,
    MAGICTYPE_APPLYTOPLAYER, MAGICTYPE_ATTACKALL, MAGICTYPE_ATTACKFIELD, MAGICTYPE_ATTACKWHOLE,
    MAGICTYPE_NORMAL, MAGICTYPE_SUMMON, MAGICTYPE_TRANCE, MAX_ENEMIES_IN_TEAM, MAX_INVENTORY,
    MAX_OBJECTS, MAX_PLAYERS_IN_PARTY, MAX_PLAYER_MAGICS, MAX_POISONS, NUM_MAGIC_ELEMENTAL,
    STATUS_ALL, STATUS_BRAVERY, STATUS_CONFUSED, STATUS_DUALATTACK, STATUS_HASTE, STATUS_PARALYZED,
    STATUS_PROTECT, STATUS_PUPPET, STATUS_SILENCE, STATUS_SLEEP,
};

const BATTLE_LABEL_ESCAPEFAIL: u16 = 31;

// ===========================================================================
// Simple predicates.
// ===========================================================================

/// PAL_IsPlayerDying.
pub fn is_player_dying(globals: &Globals, player_role: usize) -> bool {
    globals.game.player_roles.hp[player_role]
        < 100.min(globals.game.player_roles.max_hp[player_role] / 5)
}

/// PAL_IsPlayerHealthy.
pub fn is_player_healthy(globals: &Globals, player_role: usize) -> bool {
    !is_player_dying(globals, player_role)
        && globals.player_status[player_role][STATUS_SLEEP] == 0
        && globals.player_status[player_role][STATUS_CONFUSED] == 0
        && globals.player_status[player_role][STATUS_SILENCE] == 0
        && globals.player_status[player_role][STATUS_PARALYZED] == 0
        && globals.player_status[player_role][STATUS_PUPPET] == 0
}

/// PAL_BattleSelectAutoTargetFrom.
pub fn select_auto_target_from(battle: &Battle, begin: i32) -> i32 {
    let i = battle.ui.prev_enemy_target;
    if i >= 0
        && i <= battle.max_enemy_index as i32
        && battle.enemy[i as usize].object_id != 0
        && battle.enemy[i as usize].e.health > 0
    {
        return i;
    }
    let mut idx = if begin >= 0 { begin as usize } else { 0 };
    for _ in 0..MAX_ENEMIES_IN_TEAM {
        if battle.enemy[idx].object_id != 0 && battle.enemy[idx].e.health > 0 {
            return idx as i32;
        }
        idx = (idx + 1) % (battle.max_enemy_index as usize + 1);
    }
    -1
}

/// PAL_BattleSelectAutoTarget.
pub fn select_auto_target(battle: &Battle) -> i32 {
    select_auto_target_from(battle, 0)
}

// ===========================================================================
// The damage formulas (ported exactly).
// ===========================================================================

/// PAL_CalcBaseDamage. Formula courtesy of palxex and shenyanduxing.
pub fn calc_base_damage(attack_strength: u16, defense: u16) -> i16 {
    let str_ = attack_strength as f64;
    let def = defense as f64;
    if str_ > def {
        (str_ * 2.0 - def * 1.6 + 0.5) as i32 as i16
    } else if str_ > def * 0.6 {
        (str_ - def * 0.6 + 0.5) as i32 as i16
    } else {
        0
    }
}

/// PAL_CalcPhysicalAttackDamage.
pub fn calc_physical_attack_damage(
    attack_strength: u16,
    defense: u16,
    attack_resistance: u16,
) -> i16 {
    let mut dmg = calc_base_damage(attack_strength, defense);
    if attack_resistance != 0 {
        dmg /= attack_resistance as i16;
    }
    dmg
}

/// PAL_CalcMagicDamage.
pub fn calc_magic_damage(
    globals: &Globals,
    magic_strength: u16,
    defense: u16,
    elem_resistance: &[u16; NUM_MAGIC_ELEMENTAL],
    poison_resistance: u16,
    resistance_multiplier: u16,
    magic_object_id: u16,
) -> i16 {
    let magic_number = globals.game.objects[magic_object_id as usize].magic_number() as usize;

    // wMagicStrength *= RandomFloat(10, 11); wMagicStrength /= 10; — WORD math,
    // truncating like the original including its unsigned wraparound.
    let ms: u16 = ((magic_strength as f32 * random_float(10.0, 11.0)) as i64 as u16) / 10;

    let mut damage = calc_base_damage(ms, defense);
    damage /= 4;
    damage = damage.wrapping_add(globals.game.magics[magic_number].base_damage as i16);

    let elem = globals.game.magics[magic_number].elemental;
    if elem != 0 {
        let scale = if elem as usize > NUM_MAGIC_ELEMENTAL {
            10.0 - (poison_resistance as f32 / resistance_multiplier as f32)
        } else {
            10.0 - (elem_resistance[(elem - 1) as usize] as f32 / resistance_multiplier as f32)
        };
        damage = (damage as f32 * scale) as i32 as i16;
        damage /= 5;

        if elem as usize <= NUM_MAGIC_ELEMENTAL {
            let field = &globals.game.battle_fields[globals.num_battle_field as usize];
            let effect = field.magic_effect[(elem - 1) as usize];
            damage = damage.wrapping_mul(10i16.wrapping_add(effect));
            damage /= 10;
        }
    }
    damage
}

/// PAL_GetEnemyDexterity (classic).
fn get_enemy_dexterity(battle: &Battle, enemy_index: usize) -> i16 {
    let e = &battle.enemy[enemy_index].e;
    let s = (e.level as i16 + 6).wrapping_mul(3);
    s.wrapping_add(e.dexterity as i16)
}

/// PAL_GetPlayerActualDexterity (classic).
fn get_player_actual_dexterity(globals: &Globals, player_role: usize) -> u16 {
    let mut dex = globals.player_dexterity(player_role);
    if globals.player_status[player_role][STATUS_HASTE] != 0 {
        dex = dex.wrapping_mul(3);
    }
    dex.min(999)
}

// ===========================================================================
// PAL_BattleDelay.
// ===========================================================================

/// Advance all enemy idle gestures by one frame.
fn update_enemy_gestures(engine: &Engine, battle: &mut Battle) {
    for j in 0..=battle.max_enemy_index as usize {
        if battle.enemy[j].object_id == 0
            || battle.enemy[j].status[STATUS_SLEEP] != 0
            || battle.enemy[j].status[STATUS_PARALYZED] != 0
        {
            continue;
        }
        battle.enemy[j].e.idle_anim_speed = battle.enemy[j].e.idle_anim_speed.wrapping_sub(1);
        if battle.enemy[j].e.idle_anim_speed == 0 {
            battle.enemy[j].current_frame += 1;
            let enemy_id =
                engine.globals.game.objects[battle.enemy[j].object_id as usize].enemy_id() as usize;
            battle.enemy[j].e.idle_anim_speed =
                engine.globals.game.enemies[enemy_id].idle_anim_speed;
        }
        if battle.enemy[j].current_frame >= battle.enemy[j].e.idle_frames {
            battle.enemy[j].current_frame = 0;
        }
    }
}

/// PAL_BattleDelay.
pub fn battle_delay(
    engine: &mut Engine,
    battle: &mut Battle,
    duration: u16,
    object_id: u16,
    update_gesture: bool,
) {
    if battle.instant {
        // Rendering/timing only — no gameplay effect.
        return;
    }
    let mut time = engine.ticks() + BATTLE_FRAME_TIME;
    for _ in 0..duration {
        if update_gesture {
            update_enemy_gestures(engine, battle);
        }
        engine.delay_until(time);
        time = engine.ticks() + BATTLE_FRAME_TIME;
        make_scene(engine, battle);
        copy_scene_to_screen(engine, battle);
        crate::uibattle::ui_update(engine, battle);
        let _ = object_id; // in-scene text label is a cross-module UI stub
        engine.video_update();
    }
}

// ===========================================================================
// Stat backup / display.
// ===========================================================================

/// PAL_BattleBackupStat.
fn battle_backup_stat(engine: &Engine, battle: &mut Battle) {
    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        battle.enemy[i].prev_hp = battle.enemy[i].e.health;
    }
    for i in 0..=engine.globals.max_party_member_index as usize {
        let role = engine.globals.party[i].player_role as usize;
        battle.player[i].prev_hp = engine.globals.game.player_roles.hp[role];
        battle.player[i].prev_mp = engine.globals.game.player_roles.mp[role];
    }
}

/// PAL_BattleDisplayStatChange.
fn battle_display_stat_change(engine: &mut Engine, battle: &mut Battle) -> bool {
    let mut f = false;
    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        if battle.enemy[i].prev_hp != battle.enemy[i].e.health {
            let damage = battle.enemy[i].e.health as i16 - battle.enemy[i].prev_hp as i16;
            let x = battle.enemy[i].pos.0 - 9;
            let y = (battle.enemy[i].pos.1 - 115).max(10);
            let (n, c) = if damage < 0 {
                ((-damage) as u16, 1)
            } else {
                (damage as u16, 0)
            };
            crate::uibattle::ui_show_num(engine, battle, n, (x, y), c);
            f = true;
        }
    }
    for i in 0..=engine.globals.max_party_member_index as usize {
        let role = engine.globals.party[i].player_role as usize;
        if battle.player[i].prev_hp != engine.globals.game.player_roles.hp[role] {
            let damage =
                engine.globals.game.player_roles.hp[role] as i16 - battle.player[i].prev_hp as i16;
            let x = battle.player[i].pos.0 - 9;
            let y = (battle.player[i].pos.1 - 75).max(10);
            let (n, c) = if damage < 0 {
                ((-damage) as u16, 1)
            } else {
                (damage as u16, 0)
            };
            crate::uibattle::ui_show_num(engine, battle, n, (x, y), c);
            f = true;
        }
        if battle.player[i].prev_mp != engine.globals.game.player_roles.mp[role] {
            let damage =
                engine.globals.game.player_roles.mp[role] as i16 - battle.player[i].prev_mp as i16;
            let x = battle.player[i].pos.0 - 9;
            let y = (battle.player[i].pos.1 - 67).max(10);
            if damage > 0 {
                crate::uibattle::ui_show_num(engine, battle, damage as u16, (x, y), 2);
            }
            f = true;
        }
    }
    f
}

// ===========================================================================
// PAL_BattlePostActionCheck.
// ===========================================================================

/// PAL_BattlePostActionCheck.
fn battle_post_action_check(engine: &mut Engine, battle: &mut Battle, check_players: bool) {
    let mut fade = false;
    let mut enemy_remaining = false;

    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        if (battle.enemy[i].e.health as i16) <= 0 {
            battle.exp_gained += battle.enemy[i].e.exp as i32;
            battle.cash_gained += battle.enemy[i].e.cash as i32;
            engine.play_sound(battle.enemy[i].e.death_sound as i32);
            battle.enemy[i].object_id = 0;
            battle.enemy[i].sprite = Vec::new();
            fade = true;
            continue;
        }
        enemy_remaining = true;
    }

    if !enemy_remaining {
        battle.enemy_cleared = true;
        battle.ui.state = BattleUiState::Wait;
    }

    if check_players && !engine.globals.auto_battle {
        let max_party = engine.globals.max_party_member_index as usize;

        // Friend-death cover scripts.
        'friend: for i in 0..=max_party {
            let mut w = engine.globals.party[i].player_role as usize;
            if engine.globals.game.player_roles.hp[w] < battle.player[i].prev_hp
                && engine.globals.game.player_roles.hp[w] == 0
            {
                w = engine.globals.game.player_roles.covered_by[w] as usize;
                let mut j = 0;
                while j <= max_party {
                    if engine.globals.party[j].player_role as usize == w {
                        break;
                    }
                    j += 1;
                }
                if engine.globals.game.player_roles.hp[w] > 0
                    && engine.globals.player_status[w][STATUS_SLEEP] == 0
                    && engine.globals.player_status[w][STATUS_PARALYZED] == 0
                    && engine.globals.player_status[w][STATUS_CONFUSED] == 0
                    && j <= max_party
                {
                    let name = engine.globals.game.player_roles.name[w] as usize;
                    if engine.globals.game.objects[name].player_script_on_friend_death() != 0 {
                        battle_delay(engine, battle, 10, 0, true);
                        make_scene(engine, battle);
                        copy_scene_to_screen(engine, battle);
                        engine.video_update();
                        battle.battle_result = BattleResult::Pause;
                        let s = engine.globals.game.objects[name].player_script_on_friend_death();
                        let ns = engine.run_trigger_script_in_battle(battle, s, w as u16);
                        engine.globals.game.objects[name].set_player_script_on_friend_death(ns);
                        battle.battle_result = BattleResult::OnGoing;
                        engine.input.clear_key_state();
                        break 'friend;
                    }
                }
            }
        }

        // Dying scripts.
        'dying: for i in 0..=max_party {
            let w = engine.globals.party[i].player_role as usize;
            if engine.globals.player_status[w][STATUS_SLEEP] != 0
                || engine.globals.player_status[w][STATUS_CONFUSED] != 0
            {
                continue;
            }
            if engine.globals.game.player_roles.hp[w] < battle.player[i].prev_hp
                && engine.globals.game.player_roles.hp[w] > 0
                && is_player_dying(&engine.globals, w)
                && battle.player[i].prev_hp >= engine.globals.game.player_roles.max_hp[w] / 5
            {
                let cover = engine.globals.game.player_roles.covered_by[w] as usize;
                if engine.globals.player_status[cover][STATUS_SLEEP] != 0
                    || engine.globals.player_status[cover][STATUS_PARALYZED] != 0
                    || engine.globals.player_status[cover][STATUS_CONFUSED] != 0
                {
                    continue;
                }
                let name = engine.globals.game.player_roles.name[w] as usize;
                engine.play_sound(engine.globals.game.player_roles.dying_sound[w] as i32);
                let mut j = 0;
                while j <= max_party {
                    if engine.globals.party[j].player_role as usize == cover {
                        break;
                    }
                    j += 1;
                }
                if j > max_party || engine.globals.game.player_roles.hp[cover] == 0 {
                    continue;
                }
                if engine.globals.game.objects[name].player_script_on_dying() != 0 {
                    battle_delay(engine, battle, 10, 0, true);
                    make_scene(engine, battle);
                    copy_scene_to_screen(engine, battle);
                    engine.video_update();
                    battle.battle_result = BattleResult::Pause;
                    let s = engine.globals.game.objects[name].player_script_on_dying();
                    let ns = engine.run_trigger_script_in_battle(battle, s, w as u16);
                    engine.globals.game.objects[name].set_player_script_on_dying(ns);
                    battle.battle_result = BattleResult::OnGoing;
                    engine.input.clear_key_state();
                }
                break 'dying;
            }
        }
    }

    if fade {
        if !battle.instant {
            engine
                .screen_bak
                .pixels
                .copy_from_slice(&battle.scene_buf.pixels);
        }
        make_scene(engine, battle);
        fade_scene(engine, battle);
    }

    // Fade out the summoned god.
    if !battle.summon_sprite.is_empty() {
        battle_update_fighters(engine, battle);
        battle_delay(engine, battle, 1, 0, false);
        battle.summon_sprite = Vec::new();
        battle.background_color_shift = 0;
        if !battle.instant {
            engine
                .screen_bak
                .pixels
                .copy_from_slice(&battle.scene_buf.pixels);
        }
        make_scene(engine, battle);
        fade_scene(engine, battle);
    }
}

// ===========================================================================
// PAL_BattleUpdateFighters.
// ===========================================================================

/// PAL_BattleUpdateFighters.
pub fn battle_update_fighters(engine: &mut Engine, battle: &mut Battle) {
    for i in 0..=engine.globals.max_party_member_index as usize {
        let role = engine.globals.party[i].player_role as usize;
        if !battle.player[i].defending {
            battle.player[i].pos = battle.player[i].pos_original;
        }
        battle.player[i].color_shift = 0;

        if engine.globals.game.player_roles.hp[role] == 0 {
            if engine.globals.player_status[role][STATUS_PUPPET] == 0 {
                battle.player[i].current_frame = 2; // dead
            } else {
                battle.player[i].current_frame = 0; // puppet
            }
        } else if engine.globals.player_status[role][STATUS_SLEEP] != 0
            || is_player_dying(&engine.globals, role)
        {
            battle.player[i].current_frame = 1;
        } else if battle.player[i].defending && !battle.enemy_cleared {
            battle.player[i].current_frame = 3;
        } else {
            battle.player[i].current_frame = 0;
        }
    }

    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        battle.enemy[i].pos = battle.enemy[i].pos_original;
        battle.enemy[i].color_shift = 0;

        if battle.enemy[i].status[STATUS_SLEEP] > 0 || battle.enemy[i].status[STATUS_PARALYZED] > 0
        {
            battle.enemy[i].current_frame = 0;
            continue;
        }

        battle.enemy[i].e.idle_anim_speed = battle.enemy[i].e.idle_anim_speed.wrapping_sub(1);
        if battle.enemy[i].e.idle_anim_speed == 0 {
            battle.enemy[i].current_frame += 1;
            let enemy_id =
                engine.globals.game.objects[battle.enemy[i].object_id as usize].enemy_id() as usize;
            battle.enemy[i].e.idle_anim_speed =
                engine.globals.game.enemies[enemy_id].idle_anim_speed;
        }
        if battle.enemy[i].current_frame >= battle.enemy[i].e.idle_frames {
            battle.enemy[i].current_frame = 0;
        }
    }
}

/// PAL_BattlePlayerCheckReady.
pub fn battle_player_check_ready(engine: &Engine, battle: &mut Battle) {
    let mut fl_max = 0.0f32;
    let mut i_max = 0usize;
    for i in 0..=engine.globals.max_party_member_index as usize {
        if battle.player[i].state == FighterState::Com
            || (battle.player[i].state == FighterState::Act
                && battle.player[i].action.action_type == BattleActionType::CoopMagic)
        {
            fl_max = 0.0;
            break;
        } else if battle.player[i].state == FighterState::Wait
            && battle.player[i].time_meter > fl_max
        {
            i_max = i;
            fl_max = battle.player[i].time_meter;
        }
    }
    if fl_max >= 100.0 {
        battle.player[i_max].state = FighterState::Com;
        battle.player[i_max].defending = false;
    }
}

// ===========================================================================
// PAL_BattleStartFrame (classic phase machine).
// ===========================================================================

/// PAL_BattleStartFrame.
pub fn battle_start_frame(engine: &mut Engine, battle: &mut Battle) {
    let max_party = engine.globals.max_party_member_index as usize;
    let mut only_puppet = true;

    if !battle.enemy_cleared {
        battle_update_fighters(engine, battle);
    }

    if !battle.instant {
        make_scene(engine, battle);
        copy_scene_to_screen(engine, battle);
    }

    if battle.enemy_cleared {
        battle.battle_result = BattleResult::Won;
        engine.play_sound(0);
        return;
    }

    let mut ended = true;
    for i in 0..=max_party {
        let role = engine.globals.party[i].player_role as usize;
        if engine.globals.game.player_roles.hp[role] != 0 {
            only_puppet = false;
            ended = false;
            break;
        } else if engine.globals.player_status[role][STATUS_PUPPET] != 0 {
            only_puppet = false;
        }
    }
    if ended {
        battle.battle_result = BattleResult::Lost;
        return;
    }

    if battle.phase == BattlePhase::SelectAction {
        if battle.ui.state == BattleUiState::Wait {
            let mut i = 0usize;
            let mut coop_break = false;
            while i <= max_party {
                let role = engine.globals.party[i].player_role as usize;
                if engine.globals.game.player_roles.hp[role] == 0
                    || engine.globals.player_status[role][STATUS_SLEEP] != 0
                    || engine.globals.player_status[role][STATUS_CONFUSED] != 0
                    || engine.globals.player_status[role][STATUS_PARALYZED] != 0
                {
                    i += 1;
                    continue;
                }
                if battle.player[i].state == FighterState::Wait {
                    battle.moving_player_index = i as u16;
                    battle.player[i].state = FighterState::Com;
                    crate::uibattle::ui_player_ready(engine, battle, i as u16);
                    break;
                } else if battle.player[i].action.action_type == BattleActionType::CoopMagic {
                    coop_break = true;
                    break;
                }
                i += 1;
            }
            if i > max_party || coop_break {
                select_action_finish(engine, battle);
            }
        }
    } else if battle.cur_action >= MAX_ACTIONQUEUE_ITEMS
        || battle.action_queue[battle.cur_action].dexterity == 0xFFFF
    {
        perform_action_finish(engine, battle);
    } else {
        let i = battle.action_queue[battle.cur_action].index as usize;
        if battle.action_queue[battle.cur_action].is_enemy {
            if battle.hiding_time == 0 && !only_puppet && battle.enemy[i].object_id != 0 {
                let s = battle.enemy[i].script_on_ready;
                battle.enemy[i].script_on_ready =
                    engine.run_trigger_script_in_battle(battle, s, i as u16);
                battle.enemy_moving = true;
                battle_enemy_perform_action(engine, battle, i as u16);
                battle.enemy_moving = false;
            }
        } else if battle.player[i].state == FighterState::Act {
            let role = engine.globals.party[i].player_role as usize;
            if engine.globals.game.player_roles.hp[role] == 0 {
                if engine.globals.player_status[role][STATUS_PUPPET] == 0 {
                    battle.player[i].action.action_type = BattleActionType::Pass;
                }
            } else if engine.globals.player_status[role][STATUS_SLEEP] > 0
                || engine.globals.player_status[role][STATUS_PARALYZED] > 0
            {
                battle.player[i].action.action_type = BattleActionType::Pass;
            } else if engine.globals.player_status[role][STATUS_CONFUSED] > 0 {
                battle.player[i].action.action_type = if is_player_dying(&engine.globals, role) {
                    BattleActionType::Pass
                } else {
                    BattleActionType::AttackMate
                };
            } else if battle.player[i].action.action_type == BattleActionType::Attack
                && battle.player[i].action.action_id != 0
            {
                battle.prev_player_auto_atk = true;
            } else if battle.prev_player_auto_atk {
                battle.ui.cur_player_index = i as u16;
                battle.ui.selected_index = battle.player[i].action.target as i32;
                battle.ui.action_type = BattleActionType::Attack;
                battle_commit_action(engine, battle, false);
            }
            battle.moving_player_index = i as u16;
            battle_player_perform_action(engine, battle, i as u16);
        }
        battle.cur_action += 1;
    }

    // R / F keys and Flee affect all players.
    if battle.ui.menu_state == BattleMenuState::Main && battle.ui.state == BattleUiState::SelectMove
    {
        if engine.input.pressed(crate::input::KEY_REPEAT) {
            battle.repeat = true;
            battle.ui.auto_attack = battle.prev_auto_atk;
        } else if engine.input.pressed(crate::input::KEY_FORCE) {
            battle.force = true;
        }
    }
    if battle.repeat {
        engine.input.key_press = crate::input::KEY_REPEAT;
    } else if battle.force {
        engine.input.key_press = crate::input::KEY_FORCE;
    } else if battle.flee {
        engine.input.key_press = crate::input::KEY_FLEE;
    }

    crate::uibattle::ui_update(engine, battle);
}

/// The tail of the SelectAction phase: build & sort the action queue.
fn select_action_finish(engine: &mut Engine, battle: &mut Battle) {
    let max_party = engine.globals.max_party_member_index as usize;

    if !battle.repeat {
        for i in 0..=max_party {
            battle.player[i].prev_action = battle.player[i].action;
        }
    }

    battle.repeat = false;
    battle.force = false;
    battle.flee = false;
    battle.prev_auto_atk = battle.ui.auto_attack;
    battle.prev_player_auto_atk = false;
    battle.cur_action = 0;

    for q in battle.action_queue.iter_mut() {
        q.index = 0xFFFF;
        q.is_second = false;
        q.dexterity = 0xFFFF;
    }

    let mut j = 0usize;

    // Enemies.
    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        battle.action_queue[j].is_enemy = true;
        battle.action_queue[j].index = i as u16;
        battle.action_queue[j].is_second = false;
        let dex = get_enemy_dexterity(battle, i) as u16;
        battle.action_queue[j].dexterity = (dex as f32 * random_float(0.9, 1.1)) as i64 as u16;
        j += 1;

        if battle.enemy[i].e.dual_move != 0 {
            battle.action_queue[j].is_enemy = true;
            battle.action_queue[j].index = i as u16;
            battle.action_queue[j].is_second = false;
            let dex = get_enemy_dexterity(battle, i) as u16;
            battle.action_queue[j].dexterity = (dex as f32 * random_float(0.9, 1.1)) as i64 as u16;
            if battle.action_queue[j].dexterity <= battle.action_queue[j - 1].dexterity {
                battle.action_queue[j].is_second = true;
            } else {
                battle.action_queue[j - 1].is_second = true;
            }
            j += 1;
        }
    }

    // Players.
    for i in 0..=max_party {
        let role = engine.globals.party[i].player_role as usize;
        battle.action_queue[j].is_enemy = false;
        battle.action_queue[j].index = i as u16;

        if engine.globals.game.player_roles.hp[role] == 0
            || engine.globals.player_status[role][STATUS_SLEEP] > 0
            || engine.globals.player_status[role][STATUS_PARALYZED] > 0
        {
            battle.action_queue[j].dexterity = 0;
            battle.player[i].action.action_type = BattleActionType::Attack;
            battle.player[i].action.action_id = 0;
            battle.player[i].state = FighterState::Act;
        } else {
            let mut dex = get_player_actual_dexterity(&engine.globals, role) as u32;

            if engine.globals.player_status[role][STATUS_CONFUSED] > 0 {
                battle.player[i].action.action_type = BattleActionType::Attack;
                battle.player[i].action.action_id = 0;
                battle.player[i].state = FighterState::Act;
            }

            match battle.player[i].action.action_type {
                BattleActionType::CoopMagic => dex *= 10,
                BattleActionType::Defend => dex *= 5,
                BattleActionType::Magic => {
                    let id = battle.player[i].action.action_id as usize;
                    if engine.globals.game.objects[id].magic_flags() & MAGICFLAG_USABLE_TO_ENEMY
                        == 0
                    {
                        dex *= 3;
                    }
                }
                BattleActionType::Flee => dex /= 2,
                BattleActionType::UseItem => dex *= 3,
                _ => {}
            }

            if is_player_dying(&engine.globals, role) {
                dex /= 2;
            }

            dex = (dex as f32 * random_float(0.9, 1.1)) as i64 as u32;
            battle.action_queue[j].dexterity = dex as u16;
        }
        j += 1;
    }

    // Sort by dexterity, descending, signed like the C `(SHORT)` cast.
    for i in 0..MAX_ACTIONQUEUE_ITEMS {
        for k in i..MAX_ACTIONQUEUE_ITEMS {
            if (battle.action_queue[i].dexterity as i16) < (battle.action_queue[k].dexterity as i16)
            {
                battle.action_queue.swap(i, k);
            }
        }
    }

    battle.phase = BattlePhase::PerformAction;
}

/// The tail of the PerformAction phase: end-of-turn poisons, status decay,
/// turn-start scripts.
fn perform_action_finish(engine: &mut Engine, battle: &mut Battle) {
    let max_party = engine.globals.max_party_member_index as usize;

    for i in 0..=max_party {
        battle.player[i].defending = false;
        battle.player[i].pos = battle.player[i].pos_original;
    }

    battle_backup_stat(engine, battle);

    for i in 0..=max_party {
        let role = engine.globals.party[i].player_role as usize;
        for jp in 0..MAX_POISONS {
            if engine.globals.poison_status[jp][i].poison_id != 0 {
                let s = engine.globals.poison_status[jp][i].poison_script;
                let ns = engine.run_trigger_script_in_battle(battle, s, role as u16);
                engine.globals.poison_status[jp][i].poison_script = ns;
            }
        }
        for st in 0..STATUS_ALL {
            if engine.globals.player_status[role][st] > 0 {
                engine.globals.player_status[role][st] -= 1;
            }
        }
    }

    for i in 0..=battle.max_enemy_index as usize {
        for jp in 0..MAX_POISONS {
            if battle.enemy[i].poisons[jp].poison_id != 0 {
                let s = battle.enemy[i].poisons[jp].poison_script;
                let ns = engine.run_trigger_script_in_battle(battle, s, i as u16);
                battle.enemy[i].poisons[jp].poison_script = ns;
            }
        }
        for st in 0..STATUS_ALL {
            if battle.enemy[i].status[st] > 0 {
                battle.enemy[i].status[st] -= 1;
            }
        }
    }

    battle_post_action_check(engine, battle, false);
    if battle_display_stat_change(engine, battle) {
        battle_delay(engine, battle, 8, 0, true);
    }

    if battle.hiding_time > 0 {
        battle.hiding_time -= 1;
        if battle.hiding_time == 0 {
            make_scene(engine, battle);
            fade_scene(engine, battle);
        }
    }

    if battle.hiding_time == 0 {
        for i in 0..=battle.max_enemy_index as usize {
            if battle.enemy[i].object_id == 0 {
                continue;
            }
            let s = battle.enemy[i].script_on_turn_start;
            battle.enemy[i].script_on_turn_start =
                engine.run_trigger_script_in_battle(battle, s, i as u16);
        }
    }

    for i in 0..MAX_INVENTORY {
        engine.globals.inventory[i].amount_in_use = 0;
    }

    battle.phase = BattlePhase::SelectAction;
    battle.this_turn_coop = false;
}

// ===========================================================================
// PAL_BattleCommitAction.
// ===========================================================================

/// PAL_BattleCommitAction (classic).
pub fn battle_commit_action(engine: &mut Engine, battle: &mut Battle, repeat: bool) {
    let cur = battle.ui.cur_player_index as usize;

    if !repeat {
        battle.player[cur].action = crate::battle::BattleAction::default();
        battle.player[cur].action.action_type = battle.ui.action_type;
        battle.player[cur].action.target = battle.ui.selected_index as i16;
        if battle.ui.action_type == BattleActionType::Attack {
            battle.player[cur].action.action_id = if battle.ui.auto_attack { 1 } else { 0 };
        } else {
            battle.player[cur].action.action_id = battle.ui.object_id;
        }
    } else {
        let target = battle.player[cur].action.target;
        battle.player[cur].action = battle.player[cur].prev_action;
        battle.player[cur].action.target = target;
        if battle.player[cur].action.action_type == BattleActionType::Pass {
            battle.player[cur].action.action_type = BattleActionType::Attack;
            battle.player[cur].action.action_id = 0;
            battle.player[cur].action.target = -1;
        }
    }

    if battle.player[cur].action.action_type == BattleActionType::Magic {
        let w = battle.player[cur].action.action_id as usize;
        let magic_number = engine.globals.game.objects[w].magic_number() as usize;
        let cost = engine.globals.game.magics[magic_number].cost_mp;
        let role = engine.globals.party[cur].player_role as usize;
        if engine.globals.game.player_roles.mp[role] < cost {
            let mtype = engine.globals.game.magics[magic_number].magic_type;
            if mtype == MAGICTYPE_APPLYTOPLAYER
                || mtype == MAGICTYPE_APPLYTOPARTY
                || mtype == MAGICTYPE_TRANCE
            {
                battle.player[cur].action.action_type = BattleActionType::Defend;
            } else {
                battle.player[cur].action.action_type = BattleActionType::Attack;
                if battle.player[cur].action.target == -1 {
                    battle.player[cur].action.target = 0;
                }
                battle.player[cur].action.action_id = 0;
            }
        }
    }

    match battle.player[cur].action.action_type {
        BattleActionType::UseItem => {
            let id = battle.player[cur].action.action_id;
            if engine.globals.game.objects[id as usize].item_flags() & ITEMFLAG_CONSUMING != 0 {
                mark_item_in_use(engine, id);
            }
        }
        BattleActionType::ThrowItem => {
            let id = battle.player[cur].action.action_id;
            mark_item_in_use(engine, id);
        }
        _ => {}
    }

    if battle.ui.action_type == BattleActionType::Flee {
        battle.flee = true;
    }

    battle.player[cur].state = FighterState::Act;
    battle.ui.state = BattleUiState::Wait;
}

fn mark_item_in_use(engine: &mut Engine, item: u16) {
    for w in 0..MAX_INVENTORY {
        if engine.globals.inventory[w].item == item {
            engine.globals.inventory[w].amount_in_use += 1;
            break;
        }
    }
}

/// FIGHT_DetectMagicTargetChange.
fn detect_magic_target_change(globals: &Globals, magic_number: usize, target: i16) -> i16 {
    let mtype = globals.game.magics[magic_number].magic_type;
    if target == -1
        && (mtype == MAGICTYPE_NORMAL
            || mtype == MAGICTYPE_APPLYTOPLAYER
            || mtype == MAGICTYPE_TRANCE)
    {
        return 0;
    }
    if target != -1
        && (mtype == MAGICTYPE_ATTACKALL
            || mtype == MAGICTYPE_ATTACKWHOLE
            || mtype == MAGICTYPE_ATTACKFIELD
            || mtype == MAGICTYPE_APPLYTOPARTY
            || mtype == MAGICTYPE_SUMMON)
    {
        return -1;
    }
    target
}

// ===========================================================================
// Animations (rendering only; no-op when instant — no gameplay effect).
// ===========================================================================

/// PAL_BattleShowPlayerAttackAnim.
fn show_player_attack_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: usize,
    _critical: bool,
) {
    if battle.instant {
        return;
    }
    battle.player[player_index].current_frame = 8;
    battle_delay(engine, battle, 2, 0, true);
    battle.player[player_index].current_frame = 9;
    battle_delay(engine, battle, 1, 0, true);
    battle_display_stat_change(engine, battle);
    battle_backup_stat(engine, battle);
    battle_delay(engine, battle, 2, 0, true);
}

/// PAL_BattleShowPlayerUseItemAnim.
fn show_player_use_item_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: usize,
    object_id: u16,
) {
    if battle.instant {
        return;
    }
    engine.play_sound(28);
    battle.player[player_index].current_frame = 5;
    battle_delay(engine, battle, 4, object_id, true);
}

/// PAL_BattleShowPlayerPreMagicAnim.
pub fn show_player_pre_magic_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: usize,
    _summon: bool,
) {
    if battle.instant {
        return;
    }
    battle.player[player_index].current_frame = 5;
    let role = engine.globals.party[player_index].player_role as usize;
    engine.play_sound(engine.globals.game.player_roles.magic_sound[role] as i32);
    battle_delay(engine, battle, 3, 0, true);
}

/// PAL_BattleShowPlayerDefMagicAnim.
fn show_player_def_magic_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: usize,
    object_id: u16,
) {
    if battle.instant {
        return;
    }
    battle.player[player_index].current_frame = 6;
    let magic_number = engine.globals.game.objects[object_id as usize].magic_number() as usize;
    engine.play_sound(engine.globals.game.magics[magic_number].sound as i32);
    battle_delay(engine, battle, 4, 0, true);
}

/// PAL_BattleShowPlayerOffMagicAnim.
fn show_player_off_magic_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: i32,
    object_id: u16,
    _target: i16,
    _summon: bool,
) {
    if battle.instant {
        return;
    }
    let magic_number = engine.globals.game.objects[object_id as usize].magic_number() as usize;
    engine.play_sound(engine.globals.game.magics[magic_number].sound as i32);
    if player_index >= 0 {
        battle.player[player_index as usize].current_frame = 6;
    }
    battle_delay(engine, battle, 4, 0, true);
}

/// PAL_BattleShowEnemyMagicAnim.
fn show_enemy_magic_anim(engine: &mut Engine, battle: &mut Battle, object_id: u16) {
    if battle.instant {
        return;
    }
    let magic_number = engine.globals.game.objects[object_id as usize].magic_number() as usize;
    engine.play_sound(engine.globals.game.magics[magic_number].sound as i32);
    battle_delay(engine, battle, 4, 0, false);
}

/// PAL_BattleShowPlayerSummonMagicAnim.
fn show_player_summon_magic_anim(
    engine: &mut Engine,
    battle: &mut Battle,
    player_index: i32,
    object_id: u16,
) {
    if battle.instant {
        return;
    }
    let magic_number = engine.globals.game.objects[object_id as usize].magic_number() as usize;
    let wanted_effect = engine.globals.game.magics[magic_number].effect;
    let mut effect_magic_id = 0u16;
    for id in 0..MAX_OBJECTS {
        if engine.globals.game.objects[id].magic_number() == wanted_effect {
            effect_magic_id = id as u16;
            break;
        }
    }
    battle_delay(engine, battle, 1, 0, true);
    show_player_off_magic_anim(engine, battle, player_index, effect_magic_id, -1, true);
}

/// PAL_BattleShowPostMagicAnim.
fn show_post_magic_anim(engine: &mut Engine, battle: &mut Battle) {
    if battle.instant {
        return;
    }
    battle_delay(engine, battle, 1, 0, true);
}

/// PAL_BattleCheckHidingEffect.
fn battle_check_hiding_effect(engine: &mut Engine, battle: &mut Battle) {
    if battle.hiding_time < 0 {
        battle.hiding_time = -battle.hiding_time; // classic
        make_scene(engine, battle);
        fade_scene(engine, battle);
    }
}

// ===========================================================================
// PAL_BattlePlayerValidateAction.
// ===========================================================================

fn battle_player_validate_action(engine: &mut Engine, battle: &mut Battle, player_index: usize) {
    let role = engine.globals.party[player_index].player_role as usize;
    let object_id = battle.player[player_index].action.action_id;
    let target = battle.player[player_index].action.target;
    let max_party = engine.globals.max_party_member_index as usize;
    let mut valid = true;
    let mut to_enemy = false;

    match battle.player[player_index].action.action_type {
        BattleActionType::Attack => to_enemy = true,
        BattleActionType::Pass | BattleActionType::Defend => {}
        BattleActionType::Magic => {
            let mut has = false;
            for i in 0..MAX_PLAYER_MAGICS {
                if engine.globals.game.player_roles.magic[i][role] == object_id {
                    has = true;
                    break;
                }
            }
            if !has {
                valid = false;
            }
            let magic_number =
                engine.globals.game.objects[object_id as usize].magic_number() as usize;
            if engine.globals.player_status[role][STATUS_SILENCE] > 0 {
                valid = false;
            }
            if engine.globals.game.player_roles.mp[role]
                < engine.globals.game.magics[magic_number].cost_mp
            {
                valid = false;
            }
            let flags = engine.globals.game.objects[object_id as usize].magic_flags();
            if flags & MAGICFLAG_USABLE_TO_ENEMY != 0 {
                if !valid {
                    battle.player[player_index].action.action_type = BattleActionType::Attack;
                    battle.player[player_index].action.action_id = 0;
                } else if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                    battle.player[player_index].action.target = -1;
                } else if target == -1 {
                    battle.player[player_index].action.target =
                        select_auto_target_from(battle, target as i32) as i16;
                }
                to_enemy = true;
            } else if !valid {
                battle.player[player_index].action.action_type = BattleActionType::Defend;
            } else if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                battle.player[player_index].action.target = -1;
            } else if battle.player[player_index].action.target == -1 {
                battle.player[player_index].action.target = player_index as i16;
            }
        }
        BattleActionType::CoopMagic => {
            to_enemy = true;
            let mut total_healthy = 0;
            for i in 0..=max_party {
                let w = engine.globals.party[i].player_role as usize;
                battle.coop_contributors[i] = is_player_healthy(&engine.globals, w);
                if battle.coop_contributors[i] {
                    total_healthy += 1;
                }
            }
            if total_healthy <= 1 {
                battle.player[player_index].action.action_type = BattleActionType::Attack;
                battle.player[player_index].action.action_id = 0;
            }
            if battle.player[player_index].action.action_type == BattleActionType::CoopMagic {
                let flags = engine.globals.game.objects[object_id as usize].magic_flags();
                if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                    battle.player[player_index].action.target = -1;
                } else if target == -1 {
                    battle.player[player_index].action.target =
                        select_auto_target_from(battle, target as i32) as i16;
                }
            }
        }
        BattleActionType::Flee => {}
        BattleActionType::ThrowItem => {
            to_enemy = true;
            if engine.globals.get_item_amount(object_id) == 0 {
                battle.player[player_index].action.action_type = BattleActionType::Attack;
                battle.player[player_index].action.action_id = 0;
            } else if engine.globals.game.objects[object_id as usize].item_flags()
                & ITEMFLAG_APPLY_TO_ALL
                != 0
            {
                battle.player[player_index].action.target = -1;
            } else if battle.player[player_index].action.target == -1 {
                battle.player[player_index].action.target =
                    select_auto_target_from(battle, target as i32) as i16;
            }
        }
        BattleActionType::UseItem => {
            if engine.globals.get_item_amount(object_id) == 0 {
                battle.player[player_index].action.action_type = BattleActionType::Defend;
            } else if engine.globals.game.objects[object_id as usize].item_flags()
                & ITEMFLAG_APPLY_TO_ALL
                != 0
            {
                battle.player[player_index].action.target = -1;
            } else if battle.player[player_index].action.target == -1 {
                battle.player[player_index].action.target = player_index as i16;
            }
        }
        BattleActionType::AttackMate => {
            if engine.globals.player_status[role][STATUS_CONFUSED] == 0 {
                to_enemy = true;
                battle.player[player_index].action.action_type = BattleActionType::Attack;
                battle.player[player_index].action.action_id = 0;
            } else {
                let mut i = 0;
                while i <= max_party {
                    if i != player_index
                        && engine.globals.game.player_roles.hp
                            [engine.globals.party[i].player_role as usize]
                            != 0
                    {
                        break;
                    }
                    i += 1;
                }
                if i > max_party {
                    battle.player[player_index].action.action_type = BattleActionType::Pass;
                    battle.player[player_index].action.action_id = 0;
                }
            }
        }
    }

    if battle.player[player_index].action.action_type == BattleActionType::Attack {
        if target == -1 {
            if !engine.globals.player_can_attack_all(role) {
                battle.player[player_index].action.target =
                    select_auto_target_from(battle, target as i32) as i16;
            }
        } else if engine.globals.player_can_attack_all(role) {
            battle.player[player_index].action.target = -1;
        }
    }

    if to_enemy && battle.player[player_index].action.target >= 0 {
        let t = battle.player[player_index].action.target as usize;
        if battle.enemy[t].object_id == 0 {
            battle.player[player_index].action.target =
                select_auto_target_from(battle, battle.player[player_index].action.target as i32)
                    as i16;
        }
    }
}

// ===========================================================================
// PAL_BattlePlayerPerformAction.
// ===========================================================================

/// PAL_BattlePlayerPerformAction (classic).
pub fn battle_player_perform_action(engine: &mut Engine, battle: &mut Battle, player_index: u16) {
    let pi = player_index as usize;
    let role = engine.globals.party[pi].player_role as usize;

    battle.moving_player_index = player_index;
    battle.blow = 0;

    let orig_target = battle.player[pi].action.target;
    battle_player_validate_action(engine, battle, pi);
    battle_backup_stat(engine, battle);

    let mut target = battle.player[pi].action.target;

    match battle.player[pi].action.action_type {
        BattleActionType::Attack => {
            if !battle.this_turn_coop {
                if target != -1 {
                    let times = if engine.globals.player_status[role][STATUS_DUALATTACK] != 0 {
                        2
                    } else {
                        1
                    };
                    for t in 0..times {
                        let ti = target as usize;
                        let str_ = engine.globals.player_attack_strength(role);
                        let mut def = battle.enemy[ti].e.defense;
                        def = def.wrapping_add((battle.enemy[ti].e.level + 6).wrapping_mul(4));
                        let res = battle.enemy[ti].e.physical_resistance;
                        let mut critical = false;
                        let mut damage = calc_physical_attack_damage(str_, def, res) as i32;
                        damage += random_long(1, 2);
                        if random_long(0, 5) == 0
                            || engine.globals.player_status[role][STATUS_BRAVERY] > 0
                        {
                            damage *= 3;
                            critical = true;
                        }
                        if role == 0 && random_long(0, 11) == 0 {
                            damage *= 2;
                            critical = true;
                        }
                        damage = (damage as f32 * random_float(1.0, 1.125)) as i32;
                        if damage <= 0 {
                            damage = 1;
                        }
                        battle.enemy[ti].e.health =
                            battle.enemy[ti].e.health.wrapping_sub(damage as u16);
                        if t == 0 {
                            battle.player[pi].current_frame = 7;
                            battle_delay(engine, battle, 4, 0, true);
                        }
                        show_player_attack_anim(engine, battle, pi, critical);
                    }
                } else {
                    let times = if engine.globals.player_status[role][STATUS_DUALATTACK] != 0 {
                        2
                    } else {
                        1
                    };
                    for t in 0..times {
                        let mut division = 1i32;
                        let index = [2usize, 1, 0, 4, 3];
                        let critical = random_long(0, 5) == 0
                            || engine.globals.player_status[role][STATUS_BRAVERY] > 0;
                        if t == 0 {
                            battle.player[pi].current_frame = 7;
                            battle_delay(engine, battle, 4, 0, true);
                        }
                        for &ii in index.iter() {
                            if battle.enemy[ii].object_id == 0
                                || ii > battle.max_enemy_index as usize
                            {
                                continue;
                            }
                            let str_ = engine.globals.player_attack_strength(role);
                            let mut def = battle.enemy[ii].e.defense;
                            def = def.wrapping_add((battle.enemy[ii].e.level + 6).wrapping_mul(4));
                            let res = battle.enemy[ii].e.physical_resistance;
                            let mut damage = calc_physical_attack_damage(str_, def, res) as f32;
                            if critical {
                                damage *= 3.0;
                            }
                            damage /= division as f32;
                            if damage <= 0.0 {
                                damage = 1.0;
                            }
                            battle.enemy[ii].e.health =
                                battle.enemy[ii].e.health.wrapping_sub(damage as u16);
                            if battle.enemy[ii].object_id != 0 {
                                division *= 2;
                            }
                        }
                        if t > 0 {
                            battle.player[pi].second_attack = t == 1;
                        }
                        show_player_attack_anim(engine, battle, pi, critical);
                        battle_delay(engine, battle, 4, 0, true);
                    }
                }
                battle.player[pi].second_attack = false;
                battle_update_fighters(engine, battle);
                make_scene(engine, battle);
                battle_delay(engine, battle, 3, 0, true);
                engine.globals.exp.attack_exp[role].count += 1;
                engine.globals.exp.health_exp[role].count += random_long(2, 3) as u16;
            }
        }

        BattleActionType::AttackMate => {
            if !battle.this_turn_coop {
                let max_party = engine.globals.max_party_member_index as usize;
                let mut alive_other = false;
                for i in 0..=max_party {
                    if i == pi {
                        continue;
                    }
                    if engine.globals.game.player_roles.hp
                        [engine.globals.party[i].player_role as usize]
                        > 0
                    {
                        alive_other = true;
                        break;
                    }
                }
                if alive_other {
                    let mut st;
                    loop {
                        st = random_long(0, max_party as i32) as usize;
                        if st != pi
                            && engine.globals.game.player_roles.hp
                                [engine.globals.party[st].player_role as usize]
                                != 0
                        {
                            break;
                        }
                    }
                    let str_ = engine.globals.player_attack_strength(role);
                    let mut def = engine
                        .globals
                        .player_defense(engine.globals.party[st].player_role as usize);
                    if battle.player[st].defending {
                        def = def.wrapping_mul(2);
                    }
                    let mut damage = calc_physical_attack_damage(str_, def, 2);
                    let target_role = engine.globals.party[st].player_role as usize;
                    if engine.globals.player_status[target_role][STATUS_PROTECT] > 0 {
                        damage /= 2;
                    }
                    if damage <= 0 {
                        damage = 1;
                    }
                    if damage > engine.globals.game.player_roles.hp[target_role] as i16 {
                        damage = engine.globals.game.player_roles.hp[target_role] as i16;
                    }
                    engine.globals.game.player_roles.hp[target_role] -= damage as u16;
                    battle_display_stat_change(engine, battle);
                    battle_update_fighters(engine, battle);
                    battle_delay(engine, battle, 4, 0, true);
                }
            }
        }

        BattleActionType::CoopMagic => {
            battle.this_turn_coop = true;
            let object = engine.globals.player_cooperative_magic(role);
            let magic_number = engine.globals.game.objects[object as usize].magic_number() as usize;
            target = detect_magic_target_change(&engine.globals, magic_number, target);

            if engine.globals.game.magics[magic_number].magic_type == MAGICTYPE_SUMMON {
                show_player_pre_magic_anim(engine, battle, pi, true);
                show_player_summon_magic_anim(engine, battle, -1, object);
            } else {
                engine.play_sound(29);
                show_player_off_magic_anim(engine, battle, -1, object, target, false);
            }

            let max_party = engine.globals.max_party_member_index as usize;
            for i in 0..=max_party {
                if !battle.coop_contributors[i] {
                    continue;
                }
                let r = engine.globals.party[i].player_role as usize;
                let cost = engine.globals.game.magics[magic_number].cost_mp;
                engine.globals.game.player_roles.hp[r] =
                    engine.globals.game.player_roles.hp[r].wrapping_sub(cost);
                if (engine.globals.game.player_roles.hp[r] as i16) <= 0 {
                    engine.globals.game.player_roles.hp[r] = 1;
                }
                battle.player[i].state = FighterState::Wait;
            }

            battle_backup_stat(engine, battle);

            let mut str_: u32 = 0;
            for i in 0..=max_party {
                if !battle.coop_contributors[i] {
                    continue;
                }
                let r = engine.globals.party[i].player_role as usize;
                str_ += engine.globals.player_attack_strength(r) as u32;
                str_ += engine.globals.player_magic_strength(r) as u32;
            }
            str_ /= 4;

            if target == -1 {
                for i in 0..=battle.max_enemy_index as usize {
                    if battle.enemy[i].object_id == 0 {
                        continue;
                    }
                    let mut def = battle.enemy[i].e.defense;
                    def = def.wrapping_add((battle.enemy[i].e.level + 6).wrapping_mul(4));
                    let elem = battle.enemy[i].e.elem_resistance;
                    let poison = battle.enemy[i].e.poison_resistance;
                    let mut damage = calc_magic_damage(
                        &engine.globals,
                        str_ as u16,
                        def,
                        &elem,
                        poison,
                        1,
                        object,
                    );
                    if damage <= 0 {
                        damage = 1;
                    }
                    battle.enemy[i].e.health = battle.enemy[i].e.health.wrapping_sub(damage as u16);
                }
            } else {
                let ti = target as usize;
                let mut def = battle.enemy[ti].e.defense;
                def = def.wrapping_add((battle.enemy[ti].e.level + 6).wrapping_mul(4));
                let elem = battle.enemy[ti].e.elem_resistance;
                let poison = battle.enemy[ti].e.poison_resistance;
                let mut damage =
                    calc_magic_damage(&engine.globals, str_ as u16, def, &elem, poison, 1, object);
                if damage <= 0 {
                    damage = 1;
                }
                battle.enemy[ti].e.health = battle.enemy[ti].e.health.wrapping_sub(damage as u16);
            }

            battle_display_stat_change(engine, battle);
            show_post_magic_anim(engine, battle);
            battle_delay(engine, battle, 5, 0, true);

            if engine.globals.game.magics[magic_number].magic_type != MAGICTYPE_SUMMON {
                battle_post_action_check(engine, battle, false);
            }
        }

        BattleActionType::Defend => {
            if !battle.this_turn_coop {
                battle.player[pi].defending = true;
                engine.globals.exp.defense_exp[role].count += 2;
            }
        }

        BattleActionType::Flee => {
            if !battle.this_turn_coop {
                let str_ = engine.globals.player_flee_rate(role);
                let mut def: i32 = 0;
                for i in 0..=battle.max_enemy_index as usize {
                    if battle.enemy[i].object_id == 0 {
                        continue;
                    }
                    def += battle.enemy[i].e.dexterity as i16 as i32;
                    def += (battle.enemy[i].e.level as i32 + 6) * 4;
                }
                if def < 0 {
                    def = 0;
                }
                if str_ as i32 >= random_long(0, def) && !battle.is_boss {
                    player_escape(engine, battle);
                } else {
                    battle.player[pi].current_frame = 1;
                    battle_delay(engine, battle, 8, BATTLE_LABEL_ESCAPEFAIL, true);
                    engine.globals.exp.flee_exp[role].count += 2;
                }
            }
        }

        BattleActionType::Magic => {
            if !battle.this_turn_coop {
                let object = battle.player[pi].action.action_id;
                let magic_number =
                    engine.globals.game.objects[object as usize].magic_number() as usize;
                target = detect_magic_target_change(&engine.globals, magic_number, target);

                let is_summon =
                    engine.globals.game.magics[magic_number].magic_type == MAGICTYPE_SUMMON;
                show_player_pre_magic_anim(engine, battle, pi, is_summon);

                if !engine.globals.auto_battle {
                    let cost = engine.globals.game.magics[magic_number].cost_mp;
                    engine.globals.game.player_roles.mp[role] =
                        engine.globals.game.player_roles.mp[role].wrapping_sub(cost);
                    if (engine.globals.game.player_roles.mp[role] as i16) < 0 {
                        engine.globals.game.player_roles.mp[role] = 0;
                    }
                }

                let mtype = engine.globals.game.magics[magic_number].magic_type;
                if mtype == MAGICTYPE_APPLYTOPLAYER
                    || mtype == MAGICTYPE_APPLYTOPARTY
                    || mtype == MAGICTYPE_TRANCE
                {
                    let mut w = 0u16;
                    if battle.player[pi].action.target != -1 {
                        w = engine.globals.party[battle.player[pi].action.target as usize]
                            .player_role;
                    } else if mtype == MAGICTYPE_TRANCE {
                        w = role as u16;
                    }
                    let s = engine.globals.game.objects[object as usize].magic_script_on_use();
                    let ns = engine.run_trigger_script_in_battle(battle, s, role as u16);
                    engine.globals.game.objects[object as usize].set_magic_script_on_use(ns);

                    if engine.script.script_success {
                        show_player_def_magic_anim(engine, battle, pi, object);
                        let s =
                            engine.globals.game.objects[object as usize].magic_script_on_success();
                        let ns = engine.run_trigger_script_in_battle(battle, s, w);
                        engine.globals.game.objects[object as usize]
                            .set_magic_script_on_success(ns);
                    }
                } else {
                    let s = engine.globals.game.objects[object as usize].magic_script_on_use();
                    let ns = engine.run_trigger_script_in_battle(battle, s, role as u16);
                    engine.globals.game.objects[object as usize].set_magic_script_on_use(ns);

                    if engine.script.script_success {
                        if mtype == MAGICTYPE_SUMMON {
                            show_player_summon_magic_anim(engine, battle, pi as i32, object);
                        } else {
                            show_player_off_magic_anim(
                                engine, battle, pi as i32, object, target, false,
                            );
                        }
                        let s =
                            engine.globals.game.objects[object as usize].magic_script_on_success();
                        let ns = engine.run_trigger_script_in_battle(battle, s, target as u16);
                        engine.globals.game.objects[object as usize]
                            .set_magic_script_on_success(ns);

                        if (engine.globals.game.magics[magic_number].base_damage as i16) > 0 {
                            if target == -1 {
                                for i in 0..=battle.max_enemy_index as usize {
                                    if battle.enemy[i].object_id == 0 {
                                        continue;
                                    }
                                    let str_ = engine.globals.player_magic_strength(role);
                                    let mut def = battle.enemy[i].e.defense;
                                    def = def.wrapping_add(
                                        (battle.enemy[i].e.level + 6).wrapping_mul(4),
                                    );
                                    let elem = battle.enemy[i].e.elem_resistance;
                                    let poison = battle.enemy[i].e.poison_resistance;
                                    let mut damage = calc_magic_damage(
                                        &engine.globals,
                                        str_,
                                        def,
                                        &elem,
                                        poison,
                                        1,
                                        object,
                                    );
                                    if damage <= 0 {
                                        damage = 1;
                                    }
                                    battle.enemy[i].e.health =
                                        battle.enemy[i].e.health.wrapping_sub(damage as u16);
                                }
                            } else {
                                let ti = target as usize;
                                let str_ = engine.globals.player_magic_strength(role);
                                let mut def = battle.enemy[ti].e.defense;
                                def = def
                                    .wrapping_add((battle.enemy[ti].e.level + 6).wrapping_mul(4));
                                let elem = battle.enemy[ti].e.elem_resistance;
                                let poison = battle.enemy[ti].e.poison_resistance;
                                let mut damage = calc_magic_damage(
                                    &engine.globals,
                                    str_,
                                    def,
                                    &elem,
                                    poison,
                                    1,
                                    object,
                                );
                                if damage <= 0 {
                                    damage = 1;
                                }
                                battle.enemy[ti].e.health =
                                    battle.enemy[ti].e.health.wrapping_sub(damage as u16);
                            }
                        }
                    }
                }

                battle_display_stat_change(engine, battle);
                show_post_magic_anim(engine, battle);
                battle_delay(engine, battle, 5, 0, true);
                battle_check_hiding_effect(engine, battle);
                engine.globals.exp.magic_exp[role].count += random_long(2, 3) as u16;
                engine.globals.exp.magic_power_exp[role].count += 1;
            }
        }

        BattleActionType::ThrowItem => {
            if !battle.this_turn_coop {
                let object = battle.player[pi].action.action_id;
                battle.player[pi].current_frame = 5;
                battle_delay(engine, battle, 8, object, true);
                let s = engine.globals.game.objects[object as usize].item_script_on_throw();
                let ns = engine.run_trigger_script_in_battle(battle, s, target as u16);
                engine.globals.game.objects[object as usize].set_item_script_on_throw(ns);
                engine.globals.add_item_to_inventory(object, -1);
                battle_display_stat_change(engine, battle);
                battle_update_fighters(engine, battle);
                battle_delay(engine, battle, 4, 0, true);
                battle_check_hiding_effect(engine, battle);
            }
        }

        BattleActionType::UseItem => {
            if !battle.this_turn_coop {
                let object = battle.player[pi].action.action_id;
                show_player_use_item_anim(engine, battle, pi, object);
                let param = if target == -1 {
                    0xFFFF
                } else {
                    engine.globals.party[target as usize].player_role
                };
                let s = engine.globals.game.objects[object as usize].item_script_on_use();
                let ns = engine.run_trigger_script_in_battle(battle, s, param);
                engine.globals.game.objects[object as usize].set_item_script_on_use(ns);
                if engine.globals.game.objects[object as usize].item_flags() & ITEMFLAG_CONSUMING
                    != 0
                {
                    engine.globals.add_item_to_inventory(object, -1);
                }
                battle_check_hiding_effect(engine, battle);
                battle_update_fighters(engine, battle);
                battle_display_stat_change(engine, battle);
                battle_delay(engine, battle, 8, 0, true);
            }
        }

        BattleActionType::Pass => {}
    }

    battle.player[pi].state = FighterState::Wait;
    battle.player[pi].time_meter = 0.0;
    battle_post_action_check(engine, battle, false);
    battle.player[pi].action.target = orig_target;
}

// ===========================================================================
// Enemy AI.
// ===========================================================================

/// PAL_BattleEnemySelectEnemyTargetIndex.
fn enemy_select_enemy_target_index(battle: &Battle) -> usize {
    let mut i = random_long(0, battle.max_enemy_index as i32) as usize;
    while battle.enemy[i].object_id == 0 || battle.enemy[i].e.health == 0 {
        i = random_long(0, battle.max_enemy_index as i32) as usize;
    }
    i
}

/// PAL_BattleEnemySelectTargetIndex.
fn enemy_select_target_index(engine: &Engine) -> usize {
    let max_party = engine.globals.max_party_member_index as i32;
    let mut i = random_long(0, max_party) as usize;
    while engine.globals.game.player_roles.hp[engine.globals.party[i].player_role as usize] == 0 {
        i = random_long(0, max_party) as usize;
    }
    i
}

/// PAL_BattleEnemyPerformAction (classic).
pub fn battle_enemy_perform_action(engine: &mut Engine, battle: &mut Battle, enemy_index: u16) {
    let ei = enemy_index as usize;
    battle_backup_stat(engine, battle);
    battle.blow = 0;

    let mut target = enemy_select_target_index(engine) as i16;
    let player_role = engine.globals.party[target as usize].player_role as usize;
    let magic = battle.enemy[ei].e.magic;

    if battle.enemy[ei].status[STATUS_SLEEP] > 0
        || battle.enemy[ei].status[STATUS_PARALYZED] > 0
        || battle.hiding_time > 0
    {
        // Do nothing.
    } else if battle.enemy[ei].status[STATUS_CONFUSED] > 0 {
        let itarget = enemy_select_enemy_target_index(battle);
        if itarget != ei {
            let mut str_ = battle.enemy[ei].e.attack_strength as i16 as i32;
            str_ += (battle.enemy[ei].e.level as i32 + 6) * 6;
            let mut def = battle.enemy[itarget].e.defense as i16 as i32;
            def += (battle.enemy[itarget].e.level as i32 + 6) * 4;
            let res = battle.enemy[itarget].e.physical_resistance.max(1);
            let mut damage =
                calc_base_damage(str_.clamp(0, 65535) as u16, def.clamp(0, 65535) as u16) as i32
                    * 2
                    / res as i32;
            if damage <= 0 {
                damage = 1;
            }
            battle.enemy[itarget].e.health =
                battle.enemy[itarget].e.health.wrapping_sub(damage as u16);
            battle_display_stat_change(engine, battle);
            show_post_magic_anim(engine, battle);
            battle_delay(engine, battle, 5, 0, true);
            battle_post_action_check(engine, battle, false);
        }
    } else if magic != 0
        && (random_long(0, 9) as u16) < battle.enemy[ei].e.magic_rate
        && battle.enemy[ei].status[STATUS_SILENCE] == 0
    {
        if magic != 0xFFFF {
            let magic_number = engine.globals.game.objects[magic as usize].magic_number() as usize;
            let mut str_ = battle.enemy[ei].e.magic_strength as i16 as i32;
            str_ += (battle.enemy[ei].e.level as i32 + 6) * 6;
            if str_ < 0 {
                str_ = 0;
            }
            engine.play_sound(battle.enemy[ei].e.magic_sound as i32);

            let mut auto_defend = false;
            let mut mag_auto_defend = [false; MAX_PLAYERS_IN_PARTY];

            if engine.globals.game.magics[magic_number].magic_type != MAGICTYPE_NORMAL {
                target = -1;
                #[allow(clippy::needless_range_loop)]
                for i in 0..=engine.globals.max_party_member_index as usize {
                    let w = engine.globals.party[i].player_role as usize;
                    if engine.globals.player_status[w][STATUS_SLEEP] == 0
                        && engine.globals.player_status[w][STATUS_PARALYZED] == 0
                        && engine.globals.player_status[w][STATUS_CONFUSED] == 0
                        && random_long(0, 2) == 0
                        && engine.globals.game.player_roles.hp[w] != 0
                    {
                        mag_auto_defend[i] = true;
                        battle.player[i].current_frame = 3;
                    }
                }
            } else if engine.globals.player_status[player_role][STATUS_SLEEP] == 0
                && engine.globals.player_status[player_role][STATUS_PARALYZED] == 0
                && engine.globals.player_status[player_role][STATUS_CONFUSED] == 0
                && random_long(0, 2) == 0
            {
                auto_defend = true;
                battle.player[target as usize].current_frame = 3;
            }

            let s = engine.globals.game.objects[magic as usize].magic_script_on_use();
            let ns = engine.run_trigger_script_in_battle(battle, s, player_role as u16);
            engine.globals.game.objects[magic as usize].set_magic_script_on_use(ns);

            if engine.script.script_success {
                show_enemy_magic_anim(engine, battle, magic);
                let s = engine.globals.game.objects[magic as usize].magic_script_on_success();
                let ns = engine.run_trigger_script_in_battle(battle, s, player_role as u16);
                engine.globals.game.objects[magic as usize].set_magic_script_on_success(ns);
            }

            if (engine.globals.game.magics[magic_number].base_damage as i16) > 0 {
                if target == -1 {
                    #[allow(clippy::needless_range_loop)]
                    for i in 0..=engine.globals.max_party_member_index as usize {
                        let w = engine.globals.party[i].player_role as usize;
                        if engine.globals.game.player_roles.hp[w] == 0 {
                            continue;
                        }
                        let def = engine.globals.player_defense(w);
                        let mut elem = [0u16; NUM_MAGIC_ELEMENTAL];
                        for (x, slot) in elem.iter_mut().enumerate() {
                            *slot = 100 + engine.globals.player_elemental_resistance(w, x);
                        }
                        let poison = 100 + engine.globals.player_poison_resistance(w);
                        let mut damage = calc_magic_damage(
                            &engine.globals,
                            str_ as u16,
                            def,
                            &elem,
                            poison,
                            20,
                            magic,
                        );
                        let divisor = ((if battle.player[i].defending { 2 } else { 1 })
                            * (if engine.globals.player_status[w][STATUS_PROTECT] > 0 {
                                2
                            } else {
                                1
                            }))
                            + (if mag_auto_defend[i] { 1 } else { 0 });
                        damage /= divisor.max(1) as i16;
                        if damage > engine.globals.game.player_roles.hp[w] as i16 {
                            damage = engine.globals.game.player_roles.hp[w] as i16;
                        }
                        engine.globals.game.player_roles.hp[w] =
                            engine.globals.game.player_roles.hp[w].wrapping_sub(damage as u16);
                        if engine.globals.game.player_roles.hp[w] == 0 {
                            engine
                                .play_sound(engine.globals.game.player_roles.death_sound[w] as i32);
                        }
                    }
                } else {
                    let def = engine.globals.player_defense(player_role);
                    let mut elem = [0u16; NUM_MAGIC_ELEMENTAL];
                    for (x, slot) in elem.iter_mut().enumerate() {
                        *slot = 100 + engine.globals.player_elemental_resistance(player_role, x);
                    }
                    let poison = 100 + engine.globals.player_poison_resistance(player_role);
                    let mut damage = calc_magic_damage(
                        &engine.globals,
                        str_ as u16,
                        def,
                        &elem,
                        poison,
                        20,
                        magic,
                    );
                    let divisor =
                        ((if battle.player[target as usize].defending {
                            2
                        } else {
                            1
                        }) * (if engine.globals.player_status[player_role][STATUS_PROTECT] > 0 {
                            2
                        } else {
                            1
                        })) + (if auto_defend { 1 } else { 0 });
                    damage /= divisor.max(1) as i16;
                    if damage > engine.globals.game.player_roles.hp[player_role] as i16 {
                        damage = engine.globals.game.player_roles.hp[player_role] as i16;
                    }
                    engine.globals.game.player_roles.hp[player_role] =
                        engine.globals.game.player_roles.hp[player_role]
                            .wrapping_sub(damage as u16);
                    if engine.globals.game.player_roles.hp[player_role] == 0 {
                        engine.play_sound(
                            engine.globals.game.player_roles.death_sound[player_role] as i32,
                        );
                    }
                }
            }

            if !engine.globals.auto_battle {
                battle_display_stat_change(engine, battle);
            }
            battle.enemy[ei].current_frame = 0;
            battle.enemy[ei].pos = battle.enemy[ei].pos_original;
            battle_update_fighters(engine, battle);
            battle_post_action_check(engine, battle, true);
            battle_delay(engine, battle, 8, 0, true);
        }
    } else {
        // Physical attack.
        let mut wframe_bak = battle.player[target as usize].current_frame;
        let mut str_ = battle.enemy[ei].e.attack_strength as i16 as i32;
        str_ += (battle.enemy[ei].e.level as i32 + 6) * 6;
        if str_ < 0 {
            str_ = 0;
        }
        let mut def = engine.globals.player_defense(player_role);
        if battle.player[target as usize].defending {
            def = def.wrapping_mul(2);
        }
        engine.play_sound(battle.enemy[ei].e.attack_sound as i32);

        let mut cover_index: i32 = -1;
        let mut auto_defend = random_long(0, 16) >= 10;

        if (is_player_dying(&engine.globals, player_role)
            || engine.globals.player_status[player_role][STATUS_CONFUSED] > 0
            || engine.globals.player_status[player_role][STATUS_SLEEP] > 0
            || engine.globals.player_status[player_role][STATUS_PARALYZED] > 0)
            && auto_defend
        {
            let w = engine.globals.game.player_roles.covered_by[player_role] as usize;
            for i in 0..=engine.globals.max_party_member_index as usize {
                if engine.globals.party[i].player_role as usize == w {
                    cover_index = i as i32;
                    break;
                }
            }
            if cover_index != -1 {
                let cr = engine.globals.party[cover_index as usize].player_role as usize;
                if is_player_dying(&engine.globals, cr)
                    || engine.globals.player_status[cr][STATUS_CONFUSED] > 0
                    || engine.globals.player_status[cr][STATUS_SLEEP] > 0
                    || engine.globals.player_status[cr][STATUS_PARALYZED] > 0
                {
                    cover_index = -1;
                }
            }
        }

        if cover_index == -1
            && (engine.globals.player_status[player_role][STATUS_CONFUSED] > 0
                || engine.globals.player_status[player_role][STATUS_SLEEP] > 0
                || engine.globals.player_status[player_role][STATUS_PARALYZED] > 0)
        {
            auto_defend = false;
        }

        if cover_index != -1 {
            battle.player[cover_index as usize].current_frame = 3;
        } else if auto_defend {
            battle.player[target as usize].current_frame = 3;
        }

        if !auto_defend {
            battle.player[target as usize].current_frame = 4;
            let mut damage = calc_physical_attack_damage(
                (str_ + random_long(0, 2)).clamp(0, 65535) as u16,
                def,
                2,
            );
            damage += random_long(0, 1) as i16;
            if engine.globals.player_status[player_role][STATUS_PROTECT] != 0 {
                damage /= 2;
            }
            if (engine.globals.game.player_roles.hp[player_role] as i16) < damage {
                damage = engine.globals.game.player_roles.hp[player_role] as i16;
            }
            if damage <= 0 {
                damage = 1;
            }
            engine.globals.game.player_roles.hp[player_role] =
                engine.globals.game.player_roles.hp[player_role].wrapping_sub(damage as u16);
            battle_display_stat_change(engine, battle);
            battle.player[target as usize].color_shift = 6;
        }

        battle_delay(engine, battle, 1, 0, false);
        battle.player[target as usize].color_shift = 0;

        if engine.globals.game.player_roles.hp[player_role] == 0 {
            engine.play_sound(engine.globals.game.player_roles.death_sound[player_role] as i32);
            wframe_bak = 2;
        } else if is_player_dying(&engine.globals, player_role) {
            wframe_bak = 1;
        }

        battle_delay(engine, battle, 3, 0, false);
        battle.enemy[ei].pos = battle.enemy[ei].pos_original;
        battle.enemy[ei].current_frame = 0;
        battle_delay(engine, battle, 1, 0, false);
        battle.player[target as usize].current_frame = wframe_bak;
        battle_delay(engine, battle, 1, 0, true);
        battle_delay(engine, battle, 4, 0, true);
        battle_update_fighters(engine, battle);

        if cover_index == -1
            && !auto_defend
            && battle.enemy[ei].e.attack_equiv_item_rate >= random_long(1, 10) as u16
            && engine.globals.player_poison_resistance(player_role) < random_long(1, 100) as u16
        {
            let item = battle.enemy[ei].e.attack_equiv_item;
            let s = engine.globals.game.objects[item as usize].item_script_on_use();
            let ns = engine.run_trigger_script_in_battle(battle, s, player_role as u16);
            engine.globals.game.objects[item as usize].set_item_script_on_use(ns);
        }

        battle_post_action_check(engine, battle, true);
    }
    // Classic: enemy poison/status decay happens at end-of-turn, not here.
}

// ===========================================================================
// PAL_BattleStealFromEnemy.
// ===========================================================================

/// PAL_BattleStealFromEnemy.
pub fn battle_steal_from_enemy(
    engine: &mut Engine,
    battle: &mut Battle,
    target: u16,
    steal_rate: u16,
) {
    let player_index = battle.moving_player_index as usize;
    let ti = target as usize;

    battle.player[player_index].current_frame = 10;
    battle_delay(engine, battle, 1, 0, true);

    battle.player[player_index].state = FighterState::Wait;
    battle.player[player_index].time_meter = 0.0;
    battle_update_fighters(engine, battle);
    battle_delay(engine, battle, 1, 0, true);

    if battle.enemy[ti].e.steal_item_count > 0
        && (random_long(0, 10) as u16 <= steal_rate || steal_rate == 0)
    {
        if battle.enemy[ti].e.steal_item == 0 {
            let c = battle.enemy[ti].e.steal_item_count as i32 / random_long(2, 3);
            battle.enemy[ti].e.steal_item_count -= c as u16;
            engine.globals.cash += c as u32;
        } else {
            battle.enemy[ti].e.steal_item_count -= 1;
            let item = battle.enemy[ti].e.steal_item;
            engine.globals.add_item_to_inventory(item, 1);
        }
        // The "stolen X" message is shown via the cross-module dialog UI.
    }
}

// ===========================================================================
// PAL_BattleSimulateMagic.
// ===========================================================================

/// PAL_BattleSimulateMagic.
pub fn battle_simulate_magic(
    engine: &mut Engine,
    battle: &mut Battle,
    mut target: i16,
    magic_object_id: u16,
    base_damage: u16,
) {
    if engine.globals.game.objects[magic_object_id as usize].magic_flags() & MAGICFLAG_APPLY_TO_ALL
        != 0
    {
        target = -1;
    } else if target == -1 {
        target = select_auto_target_from(battle, target as i32) as i16;
    }

    show_player_off_magic_anim(engine, battle, -1, magic_object_id, target, false);

    let magic_number =
        engine.globals.game.objects[magic_object_id as usize].magic_number() as usize;
    if engine.globals.game.magics[magic_number].base_damage > 0 || base_damage > 0 {
        if target == -1 {
            for i in 0..=battle.max_enemy_index as usize {
                if battle.enemy[i].object_id == 0 {
                    continue;
                }
                let mut def = battle.enemy[i].e.defense as i16 as i32;
                def += (battle.enemy[i].e.level as i32 + 6) * 4;
                if def < 0 {
                    def = 0;
                }
                let elem = battle.enemy[i].e.elem_resistance;
                let poison = battle.enemy[i].e.poison_resistance;
                let mut damage = calc_magic_damage(
                    &engine.globals,
                    base_damage,
                    def as u16,
                    &elem,
                    poison,
                    1,
                    magic_object_id,
                );
                if damage < 0 {
                    damage = 0;
                }
                battle.enemy[i].e.health = battle.enemy[i].e.health.wrapping_sub(damage as u16);
            }
        } else {
            let ti = target as usize;
            let mut def = battle.enemy[ti].e.defense as i16 as i32;
            def += (battle.enemy[ti].e.level as i32 + 6) * 4;
            if def < 0 {
                def = 0;
            }
            let elem = battle.enemy[ti].e.elem_resistance;
            let poison = battle.enemy[ti].e.poison_resistance;
            let mut damage = calc_magic_damage(
                &engine.globals,
                base_damage,
                def as u16,
                &elem,
                poison,
                1,
                magic_object_id,
            );
            if damage < 0 {
                damage = 0;
            }
            battle.enemy[ti].e.health = battle.enemy[ti].e.health.wrapping_sub(damage as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battle::ActionQueue;

    fn globals() -> Globals {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        Globals::init(crate::data::DataDir::new().unwrap()).unwrap()
    }

    // PAL_CalcBaseDamage hand-computed vectors:
    //  str=100,def=50 : 100>50 -> 100*2 - 50*1.6 + 0.5 = 200-80+0.5 = 120.5 -> 120
    //  str=50, def=60 : 50>36  -> 50 - 36 + 0.5 = 14.5 -> 14
    //  str=20, def=60 : 20<=36 -> 0
    //  str=200,def=0  : 200>0  -> 400 - 0 + 0.5 = 400.5 -> 400
    #[test]
    fn calc_base_damage_vectors() {
        assert_eq!(calc_base_damage(100, 50), 120);
        assert_eq!(calc_base_damage(50, 60), 14);
        assert_eq!(calc_base_damage(20, 60), 0);
        assert_eq!(calc_base_damage(200, 0), 400);
    }

    // base(100,50)=120; res=2 -> 60; res=0 -> unchanged 120.
    #[test]
    fn calc_physical_attack_damage_vectors() {
        assert_eq!(calc_physical_attack_damage(100, 50, 2), 60);
        assert_eq!(calc_physical_attack_damage(100, 50, 0), 120);
        assert_eq!(calc_physical_attack_damage(20, 60, 2), 0);
    }

    // Action queue: sort descending by signed dexterity; 0xFFFF (-1) sinks.
    #[test]
    fn action_queue_sorts_descending() {
        let mut q = [ActionQueue::default(); MAX_ACTIONQUEUE_ITEMS];
        let dex = [30u16, 90u16, 60u16];
        for (j, &d) in dex.iter().enumerate() {
            q[j].index = j as u16;
            q[j].dexterity = d;
        }
        for slot in q.iter_mut().skip(dex.len()) {
            slot.dexterity = 0xFFFF;
            slot.index = 0xFFFF;
        }
        for i in 0..MAX_ACTIONQUEUE_ITEMS {
            for k in i..MAX_ACTIONQUEUE_ITEMS {
                if (q[i].dexterity as i16) < (q[k].dexterity as i16) {
                    q.swap(i, k);
                }
            }
        }
        assert_eq!(q[0].dexterity, 90);
        assert_eq!(q[1].dexterity, 60);
        assert_eq!(q[2].dexterity, 30);
        assert_eq!(q[3].dexterity, 0xFFFF);
    }

    #[test]
    fn calc_magic_damage_is_deterministic_with_seed() {
        let mut g = globals();
        g.load_default_game().unwrap();
        // Any object whose magic number is a valid index into the magic table
        // exercises the full formula; determinism holds regardless of the
        // magic's element/base damage.
        let magic_obj = (0..MAX_OBJECTS)
            .find(|&id| {
                let mn = g.game.objects[id].magic_number() as usize;
                mn != 0 && mn < g.game.magics.len() && g.game.magics[mn].elemental != 0
            })
            .expect("no elemental magic found") as u16;
        let elem = [100u16; NUM_MAGIC_ELEMENTAL];
        crate::global::seed_random(999);
        let d1 = calc_magic_damage(&g, 200, 50, &elem, 100, 20, magic_obj);
        crate::global::seed_random(999);
        let d2 = calc_magic_damage(&g, 200, 50, &elem, 100, 20, magic_obj);
        assert_eq!(d1, d2);
    }
}
