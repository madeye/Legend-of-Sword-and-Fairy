//! Battle UI (port of SDLPAL `uibattle.c`, classic `PAL_CLASSIC` mode).
//!
//! Follows the two-argument style documented in `battle.rs`.  The UI *state
//! machine* (action / target selection, the auto-battle driver, the message
//! and floating-number bookkeeping) is ported faithfully; the pixel drawing it
//! performs — which needs the shared UI sprite sheet (`gpSpriteUI`) and the
//! text renderer (`PAL_DrawText`) that live in other modules — routes through
//! the `ui.rs` contract.  The magic/item submenus and the player status screen
//! are delegated to their real implementations in magicmenu.rs / itemmenu.rs /
//! uigame.rs (see the battle-submenu adapters at the bottom of this file).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use crate::battle::{Battle, BattleActionType, BattleMenuState, BattlePhase, BattleUiState};
use crate::fight::{battle_commit_action, battle_player_check_ready, select_auto_target};
use crate::game_loop::{Engine, BATTLE_FRAME_TIME};
use crate::global::{
    MAGICFLAG_APPLY_TO_ALL, MAGICFLAG_USABLE_TO_ENEMY, MAX_ENEMIES_IN_TEAM, MAX_PLAYER_MAGICS,
    STATUS_CONFUSED, STATUS_PARALYZED, STATUS_PUPPET, STATUS_SILENCE, STATUS_SLEEP,
};
use crate::input::{
    KEY_AUTO, KEY_DEFEND, KEY_DOWN, KEY_FLEE, KEY_FORCE, KEY_LEFT, KEY_MENU, KEY_REPEAT, KEY_RIGHT,
    KEY_SEARCH, KEY_STATUS, KEY_THROW_ITEM, KEY_UP, KEY_USE_ITEM,
};

// uibattle.c file statics.
static S_FRAME: AtomicU32 = AtomicU32::new(0);
static CUR_MISC_MENU_ITEM: AtomicI32 = AtomicI32::new(0);
static CUR_SUB_MENU_ITEM: AtomicI32 = AtomicI32::new(0);

// Battle UI action ids used by validity checks.
const UI_ACTION_ATTACK: u8 = 0;
const UI_ACTION_MAGIC: u8 = 1;
const UI_ACTION_COOP_MAGIC: u8 = 2;
const UI_ACTION_MISC: u8 = 3;

// ===========================================================================
// PAL_PlayerInfoBox.
// ===========================================================================

/// PAL_PlayerInfoBox: HP/MP box for one player.  Matches the C signature (no
/// battle argument — the C reads only the passed time-meter and the player
/// role).  This is the single implementation, shared by the battle UI and by
/// the save-slot / status flows in uigame.rs.
///
/// The numeric HP/MP go through the `ui.rs` `draw_number` contract.  The
/// decorative box background sprite, player face and time-meter bar
/// (`SPRITENUM_PLAYERINFOBOX` / `PLAYERFACE` / `SLASH` from the shared UI
/// sprite sheet) are cosmetic and intentionally not drawn here.
pub fn player_info_box(
    engine: &mut Engine,
    pos: (i32, i32),
    player_role: usize,
    _time_meter: i32,
    _time_meter_color: u8,
    _update: bool,
) {
    let max_hp = engine.globals.game.player_roles.max_hp[player_role] as u32;
    let hp = engine.globals.game.player_roles.hp[player_role] as u32;
    let max_mp = engine.globals.game.player_roles.max_mp[player_role] as u32;
    let mp = engine.globals.game.player_roles.mp[player_role] as u32;
    engine.draw_number(
        max_hp,
        4,
        (pos.0 + 47, pos.1 + 8),
        crate::ui::NumColor::Yellow,
        crate::ui::NumAlign::Right,
    );
    engine.draw_number(
        hp,
        4,
        (pos.0 + 26, pos.1 + 5),
        crate::ui::NumColor::Yellow,
        crate::ui::NumAlign::Right,
    );
    engine.draw_number(
        max_mp,
        4,
        (pos.0 + 47, pos.1 + 24),
        crate::ui::NumColor::Cyan,
        crate::ui::NumAlign::Right,
    );
    engine.draw_number(
        mp,
        4,
        (pos.0 + 26, pos.1 + 21),
        crate::ui::NumColor::Cyan,
        crate::ui::NumAlign::Right,
    );
}

// ===========================================================================
// PAL_BattleUIShowText / PAL_BattleUIShowNum / PAL_BattleUIPlayerReady.
// ===========================================================================

/// PAL_BattleUIShowText.
pub fn ui_show_text(engine: &Engine, battle: &mut Battle, text: &[u8], duration: u16) {
    if engine.ticks() < battle.ui.msg_show_time {
        battle.ui.next_msg = text.to_vec();
        battle.ui.next_msg_duration = duration;
    } else {
        battle.ui.msg = text.to_vec();
        battle.ui.msg_show_time = engine.ticks() + duration as u64;
    }
}

/// PAL_BattleUIShowNum.
pub fn ui_show_num(engine: &Engine, battle: &mut Battle, num: u16, pos: (i32, i32), color: u8) {
    for slot in battle.ui.show_num.iter_mut() {
        if slot.num == 0 {
            slot.num = num;
            slot.pos = (pos.0 - 15, pos.1);
            slot.color = color;
            slot.time = engine.ticks();
            break;
        }
    }
}

/// PAL_BattleUIPlayerReady.
pub fn ui_player_ready(_engine: &mut Engine, battle: &mut Battle, player_index: u16) {
    battle.ui.cur_player_index = player_index;
    battle.ui.state = BattleUiState::SelectMove;
    battle.ui.selected_action = 0;
    battle.ui.menu_state = BattleMenuState::Main;
}

// ===========================================================================
// Helpers.
// ===========================================================================

/// PAL_BattleUIIsActionValid (classic).
fn ui_is_action_valid(engine: &Engine, battle: &Battle, action: u8) -> bool {
    let role = engine.globals.party[battle.ui.cur_player_index as usize].player_role as usize;
    match action {
        UI_ACTION_ATTACK | UI_ACTION_MISC => true,
        UI_ACTION_MAGIC => engine.globals.player_status[role][STATUS_SILENCE] == 0,
        UI_ACTION_COOP_MAGIC => {
            if engine.globals.max_party_member_index == 0 {
                return false;
            }
            let mut healthy = 0;
            for i in 0..=engine.globals.max_party_member_index as usize {
                if crate::fight::is_player_healthy(
                    &engine.globals,
                    engine.globals.party[i].player_role as usize,
                ) {
                    healthy += 1;
                }
            }
            crate::fight::is_player_healthy(&engine.globals, role) && healthy > 1
        }
        _ => true,
    }
}

/// PAL_BattleUIPickAutoMagic.
fn ui_pick_auto_magic(engine: &Engine, player_role: usize, random_range: i32) -> u16 {
    if engine.globals.player_status[player_role][STATUS_SILENCE] != 0 {
        return 0;
    }
    let mut magic = 0u16;
    let mut max_power = 0i32;
    for i in 0..MAX_PLAYER_MAGICS {
        let w = engine.globals.game.player_roles.magic[i][player_role];
        if w == 0 {
            continue;
        }
        let magic_number = engine.globals.game.objects[w as usize].magic_number() as usize;
        let m = &engine.globals.game.magics[magic_number];
        if m.cost_mp == 1
            || m.cost_mp > engine.globals.game.player_roles.mp[player_role]
            || (m.base_damage as i16) <= 0
        {
            continue;
        }
        let power = m.base_damage as i32 + crate::global::random_long(0, random_range);
        if power > max_power {
            max_power = power;
            magic = w;
        }
    }
    magic
}

// ===========================================================================
// PAL_BattleUIUpdate.
// ===========================================================================

/// PAL_BattleUIUpdate.
pub fn ui_update(engine: &mut Engine, battle: &mut Battle) {
    ui_update_body(engine, battle);
    ui_update_end(engine, battle);
}

/// The main body; every `goto end` in the C maps to an early `return` here.
fn ui_update_body(engine: &mut Engine, battle: &mut Battle) {
    S_FRAME.fetch_add(1, Ordering::Relaxed);
    let s_frame = S_FRAME.load(Ordering::Relaxed);
    let max_party = engine.globals.max_party_member_index as usize;

    if battle.ui.auto_attack && !engine.globals.auto_battle && engine.input.pressed(KEY_MENU) {
        battle.ui.auto_attack = false;
    }

    // Auto-battle driver.
    if engine.globals.auto_battle {
        battle_player_check_ready(engine, battle);
        for i in 0..=max_party {
            if battle.player[i].state == crate::battle::FighterState::Com {
                ui_player_ready(engine, battle, i as u16);
                break;
            }
        }
        if battle.ui.state != BattleUiState::Wait {
            let role =
                engine.globals.party[battle.ui.cur_player_index as usize].player_role as usize;
            let w = ui_pick_auto_magic(engine, role, 9999);
            if w == 0 {
                battle.ui.action_type = BattleActionType::Attack;
                battle.ui.selected_index = select_auto_target(battle);
            } else {
                battle.ui.action_type = BattleActionType::Magic;
                battle.ui.object_id = w;
                if engine.globals.game.objects[w as usize].magic_flags() & MAGICFLAG_APPLY_TO_ALL
                    != 0
                {
                    battle.ui.selected_index = -1;
                } else {
                    battle.ui.selected_index = select_auto_target(battle);
                }
            }
            battle_commit_action(engine, battle, false);
        }
        return;
    }

    if engine.input.pressed(KEY_AUTO) {
        battle.ui.auto_attack = !battle.ui.auto_attack;
        battle.ui.menu_state = BattleMenuState::Main;
    }

    // Classic: no UI interaction during the PerformAction phase.
    if battle.phase == BattlePhase::PerformAction {
        return;
    }

    if !battle.ui.auto_attack {
        // Draw the player info boxes.
        for i in 0..=max_party {
            let role = engine.globals.party[i].player_role as usize;
            let mut w = battle.player[i].time_meter as u16;
            if engine.globals.player_status[role][STATUS_SLEEP] != 0
                || engine.globals.player_status[role][STATUS_CONFUSED] != 0
                || engine.globals.player_status[role][STATUS_PUPPET] != 0
            {
                w = 0;
            }
            player_info_box(
                engine,
                (91 + 77 * i as i32, 165),
                role,
                w as i32,
                0x1B,
                false,
            );
        }
    }

    if engine.input.pressed(KEY_STATUS) {
        engine.player_status();
        return;
    }

    if battle.ui.state != BattleUiState::Wait {
        let role = engine.globals.party[battle.ui.cur_player_index as usize].player_role as usize;

        if engine.globals.game.player_roles.hp[role] == 0
            && engine.globals.player_status[role][STATUS_PUPPET] != 0
        {
            battle.ui.action_type = BattleActionType::Attack;
            if engine.globals.player_can_attack_all(role) {
                battle.ui.selected_index = -1;
            } else {
                battle.ui.selected_index = select_auto_target(battle);
            }
            battle_commit_action(engine, battle, false);
            return;
        }

        if engine.globals.game.player_roles.hp[role] == 0
            || engine.globals.player_status[role][STATUS_SLEEP] != 0
            || engine.globals.player_status[role][STATUS_PARALYZED] != 0
        {
            battle.ui.action_type = BattleActionType::Pass;
            battle_commit_action(engine, battle, false);
            return;
        }

        if engine.globals.player_status[role][STATUS_CONFUSED] != 0 {
            battle.ui.action_type = BattleActionType::AttackMate;
            battle_commit_action(engine, battle, false);
            return;
        }

        if battle.ui.auto_attack {
            battle.ui.action_type = BattleActionType::Attack;
            if engine.globals.player_can_attack_all(role) {
                battle.ui.selected_index = -1;
            } else {
                battle.ui.selected_index = select_auto_target(battle);
            }
            battle_commit_action(engine, battle, false);
            return;
        }

        // (Draw the "current player" arrow — cross-module gpSpriteUI, skipped.)
    }

    match battle.ui.state {
        BattleUiState::Wait => {
            if !battle.enemy_cleared {
                battle_player_check_ready(engine, battle);
                for i in 0..=max_party {
                    if battle.player[i].state == crate::battle::FighterState::Com {
                        ui_player_ready(engine, battle, i as u16);
                        break;
                    }
                }
            }
        }

        BattleUiState::SelectMove => ui_select_move(engine, battle),

        BattleUiState::SelectTargetEnemy => ui_select_target_enemy(engine, battle, s_frame),

        BattleUiState::SelectTargetPlayer => ui_select_target_player(engine, battle),

        BattleUiState::SelectTargetEnemyAll => {
            // Classic: no manual selection.
            battle.ui.selected_index = -1;
            battle_commit_action(engine, battle, false);
        }

        BattleUiState::SelectTargetPlayerAll => {
            battle.ui.selected_index = -1;
            battle_commit_action(engine, battle, false);
        }
    }
}

/// kBattleUISelectMove handling.
fn ui_select_move(engine: &mut Engine, battle: &mut Battle) {
    let cur = battle.ui.cur_player_index as usize;
    let role = engine.globals.party[cur].player_role as usize;

    if battle.ui.menu_state == BattleMenuState::Main {
        use crate::input::{DIR_EAST, DIR_NORTH, DIR_SOUTH, DIR_WEST};
        let dir = engine.input.dir;
        if dir == DIR_NORTH {
            battle.ui.selected_action = 0;
        } else if dir == DIR_SOUTH {
            battle.ui.selected_action = 3;
        } else if dir == DIR_WEST && ui_is_action_valid(engine, battle, UI_ACTION_MAGIC) {
            battle.ui.selected_action = 1;
        } else if dir == DIR_EAST && ui_is_action_valid(engine, battle, UI_ACTION_COOP_MAGIC) {
            battle.ui.selected_action = 2;
        }
    }

    let action_of = |a: u16| -> u8 {
        match a {
            0 => UI_ACTION_ATTACK,
            1 => UI_ACTION_MAGIC,
            2 => UI_ACTION_COOP_MAGIC,
            _ => UI_ACTION_MISC,
        }
    };
    if !ui_is_action_valid(engine, battle, action_of(battle.ui.selected_action)) {
        battle.ui.selected_action = 0;
    }

    match battle.ui.menu_state {
        BattleMenuState::Main => {
            if engine.input.pressed(KEY_SEARCH) {
                match battle.ui.selected_action {
                    0 => {
                        battle.ui.action_type = BattleActionType::Attack;
                        if engine.globals.player_can_attack_all(role) {
                            battle.ui.state = BattleUiState::SelectTargetEnemyAll;
                        } else {
                            if battle.ui.prev_enemy_target != -1 {
                                battle.ui.selected_index = battle.ui.prev_enemy_target;
                            }
                            battle.ui.state = BattleUiState::SelectTargetEnemy;
                            battle.ui.selected_index = 0;
                        }
                    }
                    1 => {
                        battle.ui.menu_state = BattleMenuState::MagicSelect;
                        magic_selection_menu_init(engine, battle, role as u16, true, 0);
                    }
                    2 => {
                        let w = engine.globals.player_cooperative_magic(role);
                        battle.ui.action_type = BattleActionType::CoopMagic;
                        battle.ui.object_id = w;
                        let flags = engine.globals.game.objects[w as usize].magic_flags();
                        if flags & MAGICFLAG_USABLE_TO_ENEMY != 0 {
                            if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                                battle.ui.state = BattleUiState::SelectTargetEnemyAll;
                            } else {
                                if battle.ui.prev_enemy_target != -1 {
                                    battle.ui.selected_index = battle.ui.prev_enemy_target;
                                }
                                battle.ui.state = BattleUiState::SelectTargetEnemy;
                                battle.ui.selected_index = 0;
                            }
                        } else if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                            battle.ui.state = BattleUiState::SelectTargetPlayerAll;
                        } else {
                            battle.ui.selected_index = 0;
                            battle.ui.state = BattleUiState::SelectTargetPlayer;
                        }
                    }
                    3 => battle.ui.menu_state = BattleMenuState::Misc,
                    _ => {}
                }
            } else if engine.input.pressed(KEY_DEFEND) {
                battle.ui.action_type = BattleActionType::Defend;
                battle_commit_action(engine, battle, false);
            } else if engine.input.pressed(KEY_FORCE) {
                let w = ui_pick_auto_magic(engine, role, 60);
                if w == 0 {
                    battle.ui.action_type = BattleActionType::Attack;
                    battle.ui.selected_index = if engine.globals.player_can_attack_all(role) {
                        -1
                    } else {
                        select_auto_target(battle)
                    };
                } else {
                    battle.ui.action_type = BattleActionType::Magic;
                    battle.ui.object_id = w;
                    battle.ui.selected_index = if engine.globals.game.objects[w as usize]
                        .magic_flags()
                        & MAGICFLAG_APPLY_TO_ALL
                        != 0
                    {
                        -1
                    } else {
                        select_auto_target(battle)
                    };
                }
                battle_commit_action(engine, battle, false);
            } else if engine.input.pressed(KEY_FLEE) {
                battle.ui.action_type = BattleActionType::Flee;
                battle_commit_action(engine, battle, false);
            } else if engine.input.pressed(KEY_USE_ITEM) {
                battle.ui.menu_state = BattleMenuState::UseItemSelect;
                item_select_menu_init(engine, battle, crate::global::ITEMFLAG_USABLE);
            } else if engine.input.pressed(KEY_THROW_ITEM) {
                battle.ui.menu_state = BattleMenuState::ThrowItemSelect;
                item_select_menu_init(engine, battle, crate::global::ITEMFLAG_THROWABLE);
            } else if engine.input.pressed(KEY_REPEAT) {
                battle_commit_action(engine, battle, true);
            } else if engine.input.pressed(KEY_MENU) {
                // Revert to the previous player (classic).
                battle.player[battle.ui.cur_player_index as usize].state =
                    crate::battle::FighterState::Wait;
                battle.ui.state = BattleUiState::Wait;
                if battle.ui.cur_player_index > 0 {
                    loop {
                        battle.ui.cur_player_index -= 1;
                        battle.player[battle.ui.cur_player_index as usize].state =
                            crate::battle::FighterState::Wait;
                        let r = engine.globals.party[battle.ui.cur_player_index as usize]
                            .player_role as usize;
                        if battle.ui.cur_player_index == 0
                            || (engine.globals.game.player_roles.hp[r] != 0
                                && engine.globals.player_status[r][STATUS_CONFUSED] == 0
                                && engine.globals.player_status[r][STATUS_SLEEP] == 0
                                && engine.globals.player_status[r][STATUS_PARALYZED] == 0)
                        {
                            break;
                        }
                    }
                }
            }
        }

        BattleMenuState::MagicSelect => {
            let w = magic_selection_menu_update(engine, battle);
            if w != 0xFFFF {
                battle.ui.menu_state = BattleMenuState::Main;
                if w != 0 {
                    battle.ui.action_type = BattleActionType::Magic;
                    battle.ui.object_id = w;
                    let flags = engine.globals.game.objects[w as usize].magic_flags();
                    if flags & MAGICFLAG_USABLE_TO_ENEMY != 0 {
                        if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                            battle.ui.state = BattleUiState::SelectTargetEnemyAll;
                        } else {
                            if battle.ui.prev_enemy_target != -1 {
                                battle.ui.selected_index = battle.ui.prev_enemy_target;
                            }
                            battle.ui.state = BattleUiState::SelectTargetEnemy;
                            battle.ui.selected_index = 0;
                        }
                    } else if flags & MAGICFLAG_APPLY_TO_ALL != 0 {
                        battle.ui.state = BattleUiState::SelectTargetPlayerAll;
                    } else {
                        battle.ui.selected_index = 0;
                        battle.ui.state = BattleUiState::SelectTargetPlayer;
                    }
                }
            }
        }

        BattleMenuState::UseItemSelect => ui_use_item(engine, battle),
        BattleMenuState::ThrowItemSelect => ui_throw_item(engine, battle),
        BattleMenuState::Misc => {
            let w = ui_misc_menu_update(engine);
            if w != 0xFFFF {
                battle.ui.menu_state = BattleMenuState::Main;
                match w {
                    2 => battle.ui.menu_state = BattleMenuState::MiscItemSubMenu,
                    3 => {
                        battle.ui.action_type = BattleActionType::Defend;
                        battle_commit_action(engine, battle, false);
                    }
                    1 => battle.ui.auto_attack = true,
                    4 => {
                        battle.ui.action_type = BattleActionType::Flee;
                        battle_commit_action(engine, battle, false);
                    }
                    5 => engine.player_status(),
                    _ => {}
                }
            }
        }
        BattleMenuState::MiscItemSubMenu => {
            let w = ui_misc_item_sub_menu_update(engine);
            if w != 0xFFFF {
                battle.ui.menu_state = BattleMenuState::Main;
                match w {
                    1 => {
                        battle.ui.menu_state = BattleMenuState::UseItemSelect;
                        item_select_menu_init(engine, battle, crate::global::ITEMFLAG_USABLE);
                    }
                    2 => {
                        battle.ui.menu_state = BattleMenuState::ThrowItemSelect;
                        item_select_menu_init(engine, battle, crate::global::ITEMFLAG_THROWABLE);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// kBattleUISelectTargetEnemy handling (classic).
fn ui_select_target_enemy(engine: &mut Engine, battle: &mut Battle, _s_frame: u32) {
    let mut x: i32 = -1;
    let mut y = 0;
    for i in 0..=battle.max_enemy_index as usize {
        if battle.enemy[i].object_id != 0 {
            x = i as i32;
            y += 1;
        }
    }
    if x == -1 {
        battle.ui.state = BattleUiState::SelectMove;
        return;
    }
    if battle.ui.action_type == BattleActionType::CoopMagic
        && !ui_is_action_valid(engine, battle, UI_ACTION_COOP_MAGIC)
    {
        battle.ui.state = BattleUiState::SelectMove;
        return;
    }
    // Classic: don't bother selecting when only one enemy is left.
    if y == 1 {
        if battle.ui.selected_index == -1 {
            battle.ui.selected_index = x;
        } else {
            battle.ui.selected_index = 0;
            while (battle.ui.selected_index as usize) < MAX_ENEMIES_IN_TEAM
                && battle.enemy[battle.ui.selected_index as usize].object_id == 0
            {
                battle.ui.selected_index += 1;
            }
        }
        battle_commit_action(engine, battle, false);
        return;
    }
    if battle.ui.selected_index > x {
        battle.ui.selected_index = x;
    } else if battle.ui.selected_index < 0 {
        battle.ui.selected_index = 0;
    }
    for _ in 0..=x {
        if battle.enemy[battle.ui.selected_index as usize].object_id != 0 {
            break;
        }
        battle.ui.selected_index += 1;
        battle.ui.selected_index %= x + 1;
    }

    if engine.input.pressed(KEY_MENU) {
        battle.ui.state = BattleUiState::SelectMove;
    } else if engine.input.pressed(KEY_SEARCH) {
        battle_commit_action(engine, battle, false);
    } else if engine.input.pressed(KEY_LEFT | KEY_DOWN) {
        battle.ui.selected_index -= 1;
        if battle.ui.selected_index < 0 {
            battle.ui.selected_index = MAX_ENEMIES_IN_TEAM as i32 - 1;
        }
        while battle.ui.selected_index != 0
            && battle.enemy[battle.ui.selected_index as usize].object_id == 0
        {
            battle.ui.selected_index -= 1;
            if battle.ui.selected_index < 0 {
                battle.ui.selected_index = MAX_ENEMIES_IN_TEAM as i32 - 1;
            }
        }
    } else if engine.input.pressed(KEY_RIGHT | KEY_UP) {
        battle.ui.selected_index += 1;
        if battle.ui.selected_index >= MAX_ENEMIES_IN_TEAM as i32 {
            battle.ui.selected_index = 0;
        }
        while (battle.ui.selected_index as usize) < MAX_ENEMIES_IN_TEAM
            && battle.enemy[battle.ui.selected_index as usize].object_id == 0
        {
            battle.ui.selected_index += 1;
            if battle.ui.selected_index >= MAX_ENEMIES_IN_TEAM as i32 {
                battle.ui.selected_index = 0;
            }
        }
    }
}

/// kBattleUISelectTargetPlayer handling (classic).
fn ui_select_target_player(engine: &mut Engine, battle: &mut Battle) {
    if engine.globals.max_party_member_index == 0 {
        battle.ui.selected_index = 0;
        battle_commit_action(engine, battle, false);
        return;
    }
    let max_party = engine.globals.max_party_member_index as i32;
    if engine.input.pressed(KEY_MENU) {
        battle.ui.state = BattleUiState::SelectMove;
    } else if engine.input.pressed(KEY_SEARCH) {
        battle_commit_action(engine, battle, false);
    } else if engine.input.pressed(KEY_LEFT | KEY_DOWN) {
        if battle.ui.selected_index != 0 {
            battle.ui.selected_index -= 1;
        } else {
            battle.ui.selected_index = max_party;
        }
    } else if engine.input.pressed(KEY_RIGHT | KEY_UP) {
        if battle.ui.selected_index < max_party {
            battle.ui.selected_index += 1;
        } else {
            battle.ui.selected_index = 0;
        }
    }
}

/// PAL_BattleUIUseItem.
fn ui_use_item(engine: &mut Engine, battle: &mut Battle) {
    let selected = item_select_menu_update(engine, battle);
    if selected != 0xFFFF {
        if selected != 0 {
            battle.ui.action_type = BattleActionType::UseItem;
            battle.ui.object_id = selected;
            if engine.globals.game.objects[selected as usize].item_flags() & MAGICFLAG_APPLY_TO_ALL
                != 0
            {
                battle.ui.state = BattleUiState::SelectTargetPlayerAll;
            } else {
                battle.ui.selected_index = 0;
                battle.ui.state = BattleUiState::SelectTargetPlayer;
            }
        } else {
            battle.ui.menu_state = BattleMenuState::Main;
        }
    }
}

/// PAL_BattleUIThrowItem.
fn ui_throw_item(engine: &mut Engine, battle: &mut Battle) {
    let selected = item_select_menu_update(engine, battle);
    if selected != 0xFFFF {
        if selected != 0 {
            battle.ui.action_type = BattleActionType::ThrowItem;
            battle.ui.object_id = selected;
            if engine.globals.game.objects[selected as usize].item_flags() & MAGICFLAG_APPLY_TO_ALL
                != 0
            {
                battle.ui.state = BattleUiState::SelectTargetEnemyAll;
            } else {
                if battle.ui.prev_enemy_target != -1 {
                    battle.ui.selected_index = battle.ui.prev_enemy_target;
                }
                battle.ui.state = BattleUiState::SelectTargetEnemy;
                battle.ui.selected_index = 0;
            }
        } else {
            battle.ui.menu_state = BattleMenuState::Main;
        }
    }
}

/// PAL_BattleUIMiscMenuUpdate (classic key handling).
fn ui_misc_menu_update(engine: &mut Engine) -> u16 {
    let mut cur = CUR_MISC_MENU_ITEM.load(Ordering::Relaxed);
    // (Menu drawing is cross-module; the key handling is faithful.)
    if engine.input.pressed(KEY_UP | KEY_LEFT) {
        cur -= 1;
        if cur < 0 {
            cur = 4;
        }
    } else if engine.input.pressed(KEY_DOWN | KEY_RIGHT) {
        cur += 1;
        if cur > 4 {
            cur = 0;
        }
    } else if engine.input.pressed(KEY_SEARCH) {
        CUR_MISC_MENU_ITEM.store(cur, Ordering::Relaxed);
        return (cur + 1) as u16;
    } else if engine.input.pressed(KEY_MENU) {
        CUR_MISC_MENU_ITEM.store(cur, Ordering::Relaxed);
        return 0;
    }
    CUR_MISC_MENU_ITEM.store(cur, Ordering::Relaxed);
    0xFFFF
}

/// PAL_BattleUIMiscItemSubMenuUpdate.
fn ui_misc_item_sub_menu_update(engine: &mut Engine) -> u16 {
    let mut cur = CUR_SUB_MENU_ITEM.load(Ordering::Relaxed);
    if engine.input.pressed(KEY_UP | KEY_LEFT) {
        cur = 0;
    } else if engine.input.pressed(KEY_DOWN | KEY_RIGHT) {
        cur = 1;
    } else if engine.input.pressed(KEY_SEARCH) {
        CUR_SUB_MENU_ITEM.store(cur, Ordering::Relaxed);
        return (cur + 1) as u16;
    } else if engine.input.pressed(KEY_MENU) {
        CUR_SUB_MENU_ITEM.store(cur, Ordering::Relaxed);
        return 0;
    }
    CUR_SUB_MENU_ITEM.store(cur, Ordering::Relaxed);
    0xFFFF
}

/// The `end:` tail: expire floating numbers, clear key state.
fn ui_update_end(engine: &mut Engine, battle: &mut Battle) {
    // Cycle the pending message (classic doesn't render it, but keeps queueing).
    if engine.ticks() >= battle.ui.msg_show_time && !battle.ui.next_msg.is_empty() {
        battle.ui.msg = std::mem::take(&mut battle.ui.next_msg);
        battle.ui.msg_show_time = engine.ticks() + battle.ui.next_msg_duration as u64;
    }

    let now = engine.ticks();
    for slot in battle.ui.show_num.iter_mut() {
        if slot.num > 0 && (now - slot.time) / BATTLE_FRAME_TIME > 10 {
            slot.num = 0;
        }
        // (Drawing the number goes through the ui.rs draw_number contract;
        // omitted here to avoid per-frame allocation in the headless path.)
    }

    engine.input.clear_key_state();
}

// ===========================================================================
// Battle-submenu adapters.
//
// Thin wrappers over the real magic/item selection menus in magicmenu.rs /
// itemmenu.rs: they persist the per-menu context in `battle.ui` across UI
// frames (the C keeps it in file statics).  Reached only on the *manual*
// (non-auto-battle) code path.  The `_update` variants return 0xFFFF, the
// "not yet confirmed" sentinel, while the menu is still open.
// ===========================================================================

/// PAL_MagicSelectionMenuInit (magicmenu.rs): open the in-battle magic menu.
/// The persistent context lives in `battle.ui.magic_ctx` across UI frames.
fn magic_selection_menu_init(
    engine: &mut Engine,
    battle: &mut Battle,
    player_role: u16,
    in_battle: bool,
    default_magic: u16,
) {
    battle.ui.magic_ctx =
        Some(engine.magic_selection_menu_init(player_role, in_battle, default_magic));
}

/// PAL_MagicSelectionMenuUpdate (magicmenu.rs): draw one frame, returning the
/// selected magic (0 = cancelled, 0xFFFF = not yet confirmed).
fn magic_selection_menu_update(engine: &mut Engine, battle: &mut Battle) -> u16 {
    let r = match battle.ui.magic_ctx.as_mut() {
        Some(ctx) => engine.magic_selection_menu_update(ctx),
        None => return 0xFFFF,
    };
    if r != 0xFFFF {
        battle.ui.magic_ctx = None;
    }
    r
}

/// PAL_ItemSelectMenuInit (itemmenu.rs): open the in-battle item menu.
fn item_select_menu_init(engine: &mut Engine, battle: &mut Battle, flags: u16) {
    battle.ui.item_ctx = Some(engine.item_select_menu_init(flags));
}

/// PAL_ItemSelectMenuUpdate (itemmenu.rs): draw one frame, returning the
/// selected object ID (0 = cancelled, 0xFFFF = not yet confirmed).
fn item_select_menu_update(engine: &mut Engine, battle: &mut Battle) -> u16 {
    let r = match battle.ui.item_ctx.as_ref() {
        Some(ctx) => engine.item_select_menu_update(ctx),
        None => return 0xFFFF,
    };
    if r != 0xFFFF {
        battle.ui.item_ctx = None;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_counter_advances() {
        let a = S_FRAME.load(Ordering::Relaxed);
        S_FRAME.fetch_add(1, Ordering::Relaxed);
        assert!(S_FRAME.load(Ordering::Relaxed) > a);
    }
}
