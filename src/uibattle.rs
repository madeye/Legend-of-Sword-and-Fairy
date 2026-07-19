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
use crate::surface;
use crate::ui::{MenuItem, MENUITEM_COLOR, MENUITEM_COLOR_CONFIRMED};

// uibattle.c file statics.
static S_FRAME: AtomicU32 = AtomicU32::new(0);
static CUR_MISC_MENU_ITEM: AtomicI32 = AtomicI32::new(0);
static CUR_SUB_MENU_ITEM: AtomicI32 = AtomicI32::new(0);

// Battle UI action ids used by validity checks.
const UI_ACTION_ATTACK: u8 = 0;
const UI_ACTION_MAGIC: u8 = 1;
const UI_ACTION_COOP_MAGIC: u8 = 2;
const UI_ACTION_MISC: u8 = 3;

// Shared UI sprite sheet frames (ui.h / uibattle.h SPRITENUM_*).
const SPRITENUM_PLAYERINFOBOX: usize = 18;
const SPRITENUM_SLASH: usize = 39;
const SPRITENUM_BATTLEICON_ATTACK: usize = 40;
const SPRITENUM_BATTLEICON_MAGIC: usize = 41;
const SPRITENUM_BATTLEICON_COOPMAGIC: usize = 42;
const SPRITENUM_BATTLEICON_MISCMENU: usize = 43;
const SPRITENUM_PLAYERFACE_FIRST: usize = 48;
const SPRITENUM_BATTLE_ARROW_SELECTEDPLAYER_RED: usize = 66;
const SPRITENUM_BATTLE_ARROW_SELECTEDPLAYER: usize = 67;
const SPRITENUM_BATTLE_ARROW_CURRENTPLAYER_RED: usize = 68;
const SPRITENUM_BATTLE_ARROW_CURRENTPLAYER: usize = 69;

/// BATTLEUI labels (uibattle.h): word ids of the misc-menu / sub-menu items.
const BATTLEUI_LABEL_USEITEM: u16 = 23;
const BATTLEUI_LABEL_THROWITEM: u16 = 24;
const BATTLEUI_LABEL_AUTO: u16 = 56;
const BATTLEUI_LABEL_INVENTORY: u16 = 57;
const BATTLEUI_LABEL_DEFEND: u16 = 58;
const BATTLEUI_LABEL_FLEE: u16 = 59;
const BATTLEUI_LABEL_STATUS: u16 = 60;

/// The four battle menu icons: (sprite, position, action id).
const BATTLE_MENU_ICONS: [(usize, (i32, i32), u8); 4] = [
    (SPRITENUM_BATTLEICON_ATTACK, (27, 140), UI_ACTION_ATTACK),
    (SPRITENUM_BATTLEICON_MAGIC, (0, 155), UI_ACTION_MAGIC),
    (
        SPRITENUM_BATTLEICON_COOPMAGIC,
        (54, 155),
        UI_ACTION_COOP_MAGIC,
    ),
    (SPRITENUM_BATTLEICON_MISCMENU, (27, 170), UI_ACTION_MISC),
];

// ===========================================================================
// PAL_PlayerInfoBox.
// ===========================================================================

/// PAL_PlayerInfoBox: HP/MP box for one player.  Matches the C signature (no
/// battle argument — the C reads only the passed time-meter and the player
/// role).  This is the single implementation, shared by the battle UI and by
/// the save-slot / status flows in uigame.rs.  Classic mode: no time meter.
pub fn player_info_box(
    engine: &mut Engine,
    pos: (i32, i32),
    player_role: usize,
    _time_meter: i32,
    _time_meter_color: u8,
    _update: bool,
) {
    // On-box status markers (confused/slow/sleep/silence): word, offset,
    // color — the remaining statuses have no marker in the C table.
    const STATUS_WORD: [u16; 4] = [0x1D, 0x1B, 0x1C, 0x1A];
    const STATUS_POS: [(i32, i32); 4] = [(35, 19), (44, 12), (54, 1), (55, 20)];
    const STATUS_COLOR: [u8; 4] = [0x5F, 0xBF, 0x0E, 0x3C];

    // The box background.
    if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, SPRITENUM_PLAYERINFOBOX) {
        engine.screen.blit_rle(f, pos.0, pos.1);
    }

    // The player face, tinted by the strongest active poison (dead players
    // are drawn in black/white).
    let mut max_level = 0u16;
    let mut poison_color = 0xFFu16;
    let party_index = (0..=engine.globals.max_party_member_index as usize)
        .find(|&i| engine.globals.party[i].player_role as usize == player_role);
    if let Some(pi) = party_index {
        for i in 0..crate::global::MAX_POISONS {
            let w = engine.globals.poison_status[i][pi].poison_id;
            if w != 0 && engine.globals.game.objects[w as usize].poison_level() <= 3 {
                let level = engine.globals.game.objects[w as usize].poison_level();
                if level >= max_level {
                    max_level = level;
                    poison_color = engine.globals.game.objects[w as usize].poison_color();
                }
            }
        }
    }
    if engine.globals.game.player_roles.hp[player_role] == 0 {
        poison_color = 0;
    }
    if let Some(f) = surface::sprite_frame(
        &engine.ui.sprite_ui,
        SPRITENUM_PLAYERFACE_FIRST + player_role,
    ) {
        if poison_color == 0xFF {
            engine.screen.blit_rle(f, pos.0 - 2, pos.1 - 4);
        } else {
            engine
                .screen
                .blit_rle_mono_color(f, pos.0 - 2, pos.1 - 4, poison_color as u8, 0);
        }
    }

    // HP / MP with the dividing slashes (classic layout).
    if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, SPRITENUM_SLASH) {
        engine.screen.blit_rle(f, pos.0 + 49, pos.1 + 6);
        engine.screen.blit_rle(f, pos.0 + 49, pos.1 + 22);
    }

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

    // Status markers.
    if engine.globals.game.player_roles.hp[player_role] > 0 {
        for i in 0..STATUS_WORD.len() {
            if engine.globals.player_status[player_role][i] > 0 {
                let word = engine.texts.word(STATUS_WORD[i] as usize);
                engine.draw_text(
                    &word,
                    (pos.0 + STATUS_POS[i].0, pos.1 + STATUS_POS[i].1),
                    STATUS_COLOR[i],
                    true,
                    false,
                );
            }
        }
    }
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

    // Demo-recording pilot: emulate a player pressing "confirm" at a human
    // cadence whenever the UI waits for a command (used by the demo
    // recording example; never set in normal play).
    let now = engine.ticks();
    if let Some(next) = engine.demo_pilot.as_mut() {
        let waiting = (battle.ui.state == BattleUiState::SelectMove
            && battle.ui.menu_state == BattleMenuState::Main)
            || battle.ui.state == BattleUiState::SelectTargetEnemy;
        if !waiting {
            *next = (*next).max(now + 700);
        } else if now >= *next {
            engine.input.key_press |= KEY_SEARCH;
            *next = now + 800;
        }
    }

    if battle.ui.auto_attack && !engine.globals.auto_battle {
        if engine.input.pressed(KEY_MENU) {
            battle.ui.auto_attack = false;
        } else {
            // Draw the "auto attack" indicator.
            let text = engine.texts.word(BATTLEUI_LABEL_AUTO as usize);
            let w = engine.text_width(&text);
            engine.draw_text(
                &text,
                (312 - w, 10),
                crate::ui::MENUITEM_COLOR_CONFIRMED,
                true,
                false,
            );
        }
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

        // Draw the arrow on the current player's head.
        let spr = if s_frame & 1 != 0 {
            SPRITENUM_BATTLE_ARROW_CURRENTPLAYER
        } else {
            SPRITENUM_BATTLE_ARROW_CURRENTPLAYER_RED
        };
        let (px, py) = crate::battle::PLAYER_POS[max_party][battle.ui.cur_player_index as usize];
        if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, spr) {
            engine.screen.blit_rle(f, px - 8, py - 74);
        }
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

        BattleUiState::SelectTargetPlayer => ui_select_target_player(engine, battle, s_frame),

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

    // Draw the four command icons: the selected one in full color, the other
    // valid ones dimmed, invalid ones darker still.
    for (i, &(spr, pos, action)) in BATTLE_MENU_ICONS.iter().enumerate() {
        let valid = ui_is_action_valid(engine, battle, action);
        if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, spr) {
            if battle.ui.selected_action as usize == i {
                engine.screen.blit_rle(f, pos.0, pos.1);
            } else if valid {
                engine.screen.blit_rle_mono_color(f, pos.0, pos.1, 0, -4);
            } else {
                engine.screen.blit_rle_mono_color(f, pos.0, pos.1, 0x10, -4);
            }
        }
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
                        let cur = battle.ui.cur_player_index as usize;
                        battle.player[cur].state = crate::battle::FighterState::Wait;

                        // Release the inventory reservation that
                        // battle_commit_action (mark_item_in_use) placed when
                        // this player committed a throw/consume-item action
                        // (uibattle.c ~1239-1264): throw always releases; use
                        // releases only for ITEMFLAG_CONSUMING items.
                        let action = battle.player[cur].action;
                        let release = match action.action_type {
                            BattleActionType::ThrowItem => true,
                            BattleActionType::UseItem => {
                                engine.globals.game.objects[action.action_id as usize].item_flags()
                                    & crate::global::ITEMFLAG_CONSUMING
                                    != 0
                            }
                            _ => false,
                        };
                        if release {
                            for inv in 0..crate::global::MAX_INVENTORY {
                                if engine.globals.inventory[inv].item == action.action_id {
                                    engine.globals.inventory[inv].amount_in_use -= 1;
                                    break;
                                }
                            }
                        }

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
fn ui_select_target_enemy(engine: &mut Engine, battle: &mut Battle, s_frame: u32) {
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

    // Highlight the selected enemy (blinking brighter on odd frames).
    if s_frame & 1 != 0 {
        let en = &battle.enemy[battle.ui.selected_index as usize];
        if let Some(f) = surface::sprite_frame(&en.sprite, en.current_frame as usize) {
            let hx = en.pos.0 - surface::rle_width(f) as i32 / 2;
            let hy = en.pos.1 - surface::rle_height(f) as i32;
            crate::battle::blit_rle_color_shift(&mut engine.screen, f, hx, hy, 7);
        }
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
fn ui_select_target_player(engine: &mut Engine, battle: &mut Battle, s_frame: u32) {
    // Classic: don't bother selecting when only 1 player is in the party.
    // Like C (uibattle.c ~1546-1555, no `break` in the `#ifdef PAL_CLASSIC`
    // block), there is NO early-out here — after committing, control falls
    // through to the icon-graying draw, the arrow draw, and the same-frame
    // KEY_MENU/SEARCH/LEFT/RIGHT handling below.
    if engine.globals.max_party_member_index == 0 {
        battle.ui.selected_index = 0;
        battle_commit_action(engine, battle, false);
    }
    let max_party = engine.globals.max_party_member_index as i32;

    // Gray the command icons and draw the arrow above the selected player.
    for &(spr, pos, _) in BATTLE_MENU_ICONS.iter() {
        if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, spr) {
            engine.screen.blit_rle_mono_color(f, pos.0, pos.1, 0, -4);
        }
    }
    let spr = if s_frame & 1 != 0 {
        SPRITENUM_BATTLE_ARROW_SELECTEDPLAYER_RED
    } else {
        SPRITENUM_BATTLE_ARROW_SELECTEDPLAYER
    };
    let (px, py) = crate::battle::PLAYER_POS[max_party as usize][battle.ui.selected_index as usize];
    if let Some(f) = surface::sprite_frame(&engine.ui.sprite_ui, spr) {
        engine.screen.blit_rle(f, px - 8, py - 67);
    }

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

/// PAL_BattleUIDrawMiscMenu (classic): the box + the 5 misc-menu labels, with
/// the current selection highlighted (shimmering when selected, confirmed color
/// when confirmed).
fn draw_misc_menu(engine: &mut Engine, current_item: i32, confirmed: bool) {
    let items = [
        MenuItem {
            value: 0,
            num_word: BATTLEUI_LABEL_AUTO,
            enabled: true,
            pos: (16, 32),
        },
        MenuItem {
            value: 1,
            num_word: BATTLEUI_LABEL_INVENTORY,
            enabled: true,
            pos: (16, 50),
        },
        MenuItem {
            value: 2,
            num_word: BATTLEUI_LABEL_DEFEND,
            enabled: true,
            pos: (16, 68),
        },
        MenuItem {
            value: 3,
            num_word: BATTLEUI_LABEL_FLEE,
            enabled: true,
            pos: (16, 86),
        },
        MenuItem {
            value: 4,
            num_word: BATTLEUI_LABEL_STATUS,
            enabled: true,
            pos: (16, 104),
        },
    ];

    let columns = engine.menu_text_max_width(&items) - 1;
    engine.create_box((2, 20), 4, columns, 0, false);

    for (i, it) in items.iter().enumerate() {
        let color = if i as i32 == current_item {
            if confirmed {
                MENUITEM_COLOR_CONFIRMED
            } else {
                engine.menuitem_color_selected()
            }
        } else {
            MENUITEM_COLOR
        };
        let word = engine.texts.word(it.num_word as usize);
        engine.draw_text(&word, it.pos, color, true, false);
    }
}

/// PAL_BattleUIMiscMenuUpdate (classic).
fn ui_misc_menu_update(engine: &mut Engine) -> u16 {
    let mut cur = CUR_MISC_MENU_ITEM.load(Ordering::Relaxed);

    // Draw the menu (box + highlighted labels) every frame it is open.
    draw_misc_menu(engine, cur, false);

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

    // Classic: redraw the misc menu with INVENTORY highlighted+confirmed, then
    // draw the Use/Throw sub-menu box + labels (uibattle.c ~502-522).
    draw_misc_menu(engine, 1, true);
    let items = [
        MenuItem {
            value: 0,
            num_word: BATTLEUI_LABEL_USEITEM,
            enabled: true,
            pos: (44, 62),
        },
        MenuItem {
            value: 1,
            num_word: BATTLEUI_LABEL_THROWITEM,
            enabled: true,
            pos: (44, 80),
        },
    ];
    let columns = engine.menu_text_max_width(&items) - 1;
    engine.create_box((30, 50), 1, columns, 0, false);
    for (i, it) in items.iter().enumerate() {
        let color = if i as i32 == cur {
            engine.menuitem_color_selected()
        } else {
            MENUITEM_COLOR
        };
        let word = engine.texts.word(it.num_word as usize);
        engine.draw_text(&word, it.pos, color, true, false);
    }

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

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        Engine::new(true).expect("headless engine")
    }

    #[test]
    fn frame_counter_advances() {
        let a = S_FRAME.load(Ordering::Relaxed);
        S_FRAME.fetch_add(1, Ordering::Relaxed);
        assert!(S_FRAME.load(Ordering::Relaxed) > a);
    }

    /// GAP 9: cancelling back through a committed throw-item action must
    /// release the inventory reservation that the commit placed
    /// (uibattle.c ~1239-1264).
    #[test]
    fn cancel_releases_thrown_item_reservation() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        e.globals.max_party_member_index = 1;
        e.globals.party[0].player_role = 0;
        e.globals.party[1].player_role = 1;
        e.globals.game.player_roles.hp[0] = 100;
        e.globals.game.player_roles.hp[1] = 100;

        // Player 0 committed "throw item X" (amount 1, one reserved).
        let item = 1u16;
        e.globals.inventory[0].item = item;
        e.globals.inventory[0].amount = 1;
        e.globals.inventory[0].amount_in_use = 1;

        let mut battle = Box::new(Battle::new());
        battle.ui.state = BattleUiState::SelectMove;
        battle.ui.menu_state = BattleMenuState::Main;
        battle.ui.cur_player_index = 1;
        battle.player[0].action.action_type = BattleActionType::ThrowItem;
        battle.player[0].action.action_id = item;

        // Player 1 presses KEY_MENU to cancel back to player 0.
        e.input.key_press = KEY_MENU;
        ui_select_move(&mut e, &mut battle);

        assert_eq!(battle.ui.cur_player_index, 0);
        assert_eq!(
            e.globals.inventory[0].amount_in_use, 0,
            "throw-item reservation must be released when cancelling back"
        );
    }

    /// GAP 9: a UseItem action without ITEMFLAG_CONSUMING must NOT be
    /// released on cancel (only consuming items are reserved / released).
    #[test]
    fn cancel_keeps_reservation_for_nonconsuming_use_item() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        e.globals.max_party_member_index = 1;
        e.globals.party[0].player_role = 0;
        e.globals.party[1].player_role = 1;
        e.globals.game.player_roles.hp[0] = 100;
        e.globals.game.player_roles.hp[1] = 100;

        // A UseItem action on an item that is not consuming (clear the
        // ITEMFLAG_CONSUMING bit in the object's flags word, data[6]).
        let item = 1u16;
        e.globals.game.objects[item as usize].data[6] &= !crate::global::ITEMFLAG_CONSUMING;
        e.globals.inventory[0].item = item;
        e.globals.inventory[0].amount = 1;
        e.globals.inventory[0].amount_in_use = 1;

        let mut battle = Box::new(Battle::new());
        battle.ui.state = BattleUiState::SelectMove;
        battle.ui.menu_state = BattleMenuState::Main;
        battle.ui.cur_player_index = 1;
        battle.player[0].action.action_type = BattleActionType::UseItem;
        battle.player[0].action.action_id = item;

        e.input.key_press = KEY_MENU;
        ui_select_move(&mut e, &mut battle);

        assert_eq!(
            e.globals.inventory[0].amount_in_use, 1,
            "non-consuming use-item reservation must be left untouched"
        );
    }

    /// GAP 18: with a single party member, SelectTargetPlayer commits and
    /// then falls through to the draw + key handling (no early return), so
    /// the target resets to 0 and the action is committed without panic.
    #[test]
    fn select_target_player_single_member_commits_and_falls_through() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;

        let mut battle = Box::new(Battle::new());
        battle.ui.state = BattleUiState::SelectTargetPlayer;
        battle.ui.cur_player_index = 0;
        battle.ui.selected_index = 5;
        battle.ui.action_type = BattleActionType::Defend;

        ui_select_target_player(&mut e, &mut battle, 1);

        assert_eq!(battle.ui.selected_index, 0, "commit resets the target to 0");
        assert_eq!(
            battle.ui.state,
            BattleUiState::Wait,
            "the action was committed"
        );
        assert_eq!(battle.player[0].state, crate::battle::FighterState::Act);
    }

    /// GAP 10: the misc menu and its item sub-menu draw routines run every
    /// frame the menu is open; exercise them (box + labels) and confirm the
    /// no-key path returns the "not confirmed" sentinel without panicking.
    #[test]
    fn misc_menu_draw_does_not_panic() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        assert_eq!(ui_misc_menu_update(&mut e), 0xFFFF);
        assert_eq!(ui_misc_item_sub_menu_update(&mut e), 0xFFFF);
    }
}
