//! Battle system core (port of SDLPAL `battle.c`, classic `PAL_CLASSIC` mode).
//!
//! # Design: the two-argument style
//!
//! SDLPAL keeps the entire battle in a single global `BATTLE g_Battle`.  The
//! Rust `Engine` (game_loop.rs) owns all the *persistent* game state but has no
//! field for the transient battle.  Rather than shoe-horning a `Battle` field
//! into `Engine` (which would make Rust's borrow checker fight every method
//! that touches both the battle and the globals), `start_battle` creates a
//! local [`Battle`] value and threads it through the port as an explicit second
//! argument: every internal routine takes `(engine: &mut Engine, battle: &mut
//! Battle)`.  This mirrors the C's `g_Battle` global cleanly — wherever the C
//! writes `g_Battle.foo`, the Rust writes `battle.foo`; wherever the C writes
//! `gpGlobals->foo`, the Rust writes `engine.globals.foo` — without touching
//! `game_loop.rs`.
//!
//! The `fight.rs` (fight.c) and `uibattle.rs` (uibattle.c) ports live in their
//! own modules but operate on the same [`Battle`] type and follow the same
//! convention.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::{Engine, BATTLE_FRAME_TIME};
use crate::global::{Enemy, PlayerRoles};
use crate::global::{
    PoisonStatus, MAX_ENEMIES_IN_TEAM, MAX_INVENTORY, MAX_LEVELS, MAX_PLAYABLE_PLAYER_ROLES,
    MAX_PLAYERS_IN_PARTY, MAX_POISONS, STATUS_ALL, STATUS_PUPPET,
};
use crate::surface::{self, Surface, SCREEN_H, SCREEN_W};

// ===========================================================================
// Public contract (kept identical to the bring-up stub).
// ===========================================================================

/// BATTLERESULT.  The discriminants match SDLPAL's `BATTLERESULT` enum
/// (battle.h) so that script opcode 0x0089 can store/restore raw values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u16)]
pub enum BattleResult {
    Won = 3,
    Lost = 1,
    Fleed = 0xFFFF,
    Terminated = 0,
    OnGoing = 1000,
    PreBattle = 1001,
    Pause = 1002,
}

impl BattleResult {
    /// Map a raw script operand (opcode 0x0089) to a [`BattleResult`].  Unknown
    /// values fall back to `OnGoing`, matching the C where any non-terminal
    /// value simply keeps the battle loop running.
    pub fn from_u16(v: u16) -> BattleResult {
        match v {
            3 => BattleResult::Won,
            1 => BattleResult::Lost,
            0xFFFF => BattleResult::Fleed,
            0 => BattleResult::Terminated,
            1001 => BattleResult::PreBattle,
            1002 => BattleResult::Pause,
            _ => BattleResult::OnGoing,
        }
    }
}

// ===========================================================================
// Battle data structures (port of battle.h).
// ===========================================================================

/// FIGHTERSTATE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FighterState {
    #[default]
    Wait,
    Com,
    Act,
}

/// BATTLEACTIONTYPE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BattleActionType {
    #[default]
    Pass,
    Defend,
    Attack,
    Magic,
    CoopMagic,
    Flee,
    ThrowItem,
    UseItem,
    AttackMate,
}

/// BATTLEACTION.
#[derive(Clone, Copy, Debug, Default)]
pub struct BattleAction {
    pub action_type: BattleActionType,
    pub action_id: u16,
    pub target: i16,
    pub remaining_time: f32,
}

/// BATTLEENEMY.
#[derive(Clone, Default)]
pub struct BattleEnemy {
    pub object_id: u16,
    pub e: Enemy,
    pub status: [u16; STATUS_ALL],
    pub time_meter: f32,
    pub poisons: [PoisonStatus; MAX_POISONS],
    pub sprite: Vec<u8>,
    pub pos: (i32, i32),
    pub pos_original: (i32, i32),
    pub current_frame: u16,
    pub state: FighterState,
    pub script_on_turn_start: u16,
    pub script_on_battle_end: u16,
    pub script_on_ready: u16,
    pub prev_hp: u16,
    pub color_shift: i32,
}

/// BATTLEPLAYER.
#[derive(Clone, Default)]
pub struct BattlePlayer {
    pub color_shift: i32,
    pub time_meter: f32,
    pub time_speed_modifier: f32,
    pub hiding_time: u16,
    pub sprite: Vec<u8>,
    pub pos: (i32, i32),
    pub pos_original: (i32, i32),
    pub current_frame: u16,
    pub state: FighterState,
    pub action: BattleAction,
    pub prev_action: BattleAction,
    pub defending: bool,
    pub second_attack: bool,
    pub prev_hp: u16,
    pub prev_mp: u16,
}

/// BATTLESPRITETYPE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BattleSpriteType {
    #[default]
    None,
    Enemy,
    Player,
    Magic,
}

/// BATTLESPRITESEQ.
#[derive(Clone, Copy, Default)]
pub struct BattleSpriteSeq {
    pub sprite_type: BattleSpriteType,
    /// Object index (`-1` = summon god, stored as `i32`).
    pub object_index: i32,
    pub pos: (i32, i32),
    pub layer_offset: i16,
    pub have_color_shift: bool,
}

pub const MAX_BATTLE_MAGICSPRITE_ITEMS: usize = 3;
pub const MAX_BATTLESPRITESEQ_ITEMS: usize =
    MAX_ENEMIES_IN_TEAM + MAX_PLAYABLE_PLAYER_ROLES + MAX_BATTLE_MAGICSPRITE_ITEMS;
pub const MAX_ACTIONQUEUE_ITEMS: usize = MAX_PLAYERS_IN_PARTY + MAX_ENEMIES_IN_TEAM * 2;

/// BATTLEPHASE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BattlePhase {
    #[default]
    SelectAction,
    PerformAction,
}

/// ACTIONQUEUE.
#[derive(Clone, Copy, Default)]
pub struct ActionQueue {
    pub is_enemy: bool,
    pub dexterity: u16,
    pub index: u16,
    pub is_second: bool,
}

/// BATTLEUISTATE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BattleUiState {
    #[default]
    Wait,
    SelectMove,
    SelectTargetEnemy,
    SelectTargetPlayer,
    SelectTargetEnemyAll,
    SelectTargetPlayerAll,
}

/// BATTLEMENUSTATE.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BattleMenuState {
    #[default]
    Main,
    MagicSelect,
    UseItemSelect,
    ThrowItemSelect,
    Misc,
    MiscItemSubMenu,
}

pub const BATTLEUI_MAX_SHOWNUM: usize = 16;

/// SHOWNUM.
#[derive(Clone, Copy, Default)]
pub struct ShowNum {
    pub num: u16,
    pub pos: (i32, i32),
    pub time: u64,
    /// NUMCOLOR value (0 = yellow, 1 = blue, 2 = cyan) — see uibattle.
    pub color: u8,
}

/// BATTLEUI.
pub struct BattleUi {
    pub state: BattleUiState,
    pub menu_state: BattleMenuState,
    pub msg: Vec<u8>,
    pub next_msg: Vec<u8>,
    pub msg_show_time: u64,
    pub next_msg_duration: u16,
    pub cur_player_index: u16,
    pub selected_action: u16,
    pub selected_index: i32,
    pub prev_enemy_target: i32,
    pub action_type: BattleActionType,
    pub object_id: u16,
    pub auto_attack: bool,
    pub show_num: [ShowNum; BATTLEUI_MAX_SHOWNUM],
    /// Live context for the in-battle magic-selection menu (magicmenu.rs),
    /// persisted across UI frames.  `Some` only while that submenu is open.
    pub(crate) magic_ctx: Option<crate::magicmenu::MagicMenuCtx>,
    /// Live context for the in-battle item-selection menu (itemmenu.rs).
    pub(crate) item_ctx: Option<crate::itemmenu::ItemMenuCtx>,
}

impl Default for BattleUi {
    fn default() -> BattleUi {
        BattleUi {
            state: BattleUiState::Wait,
            menu_state: BattleMenuState::Main,
            msg: Vec::new(),
            next_msg: Vec::new(),
            msg_show_time: 0,
            next_msg_duration: 0,
            cur_player_index: 0,
            selected_action: 0,
            selected_index: 0,
            prev_enemy_target: -1,
            action_type: BattleActionType::Pass,
            object_id: 0,
            auto_attack: false,
            show_num: [ShowNum::default(); BATTLEUI_MAX_SHOWNUM],
            magic_ctx: None,
            item_ctx: None,
        }
    }
}

/// BATTLE — g_Battle.
pub struct Battle {
    pub player: [BattlePlayer; MAX_PLAYERS_IN_PARTY],
    pub enemy: [BattleEnemy; MAX_ENEMIES_IN_TEAM],
    pub max_enemy_index: u16,

    pub scene_buf: Surface,
    pub background: Surface,
    pub background_color_shift: i16,

    pub summon_sprite: Vec<u8>,
    pub pos_summon: (i32, i32),
    pub summon_frame: i32,
    pub summon_color_shift: bool,

    pub exp_gained: i32,
    pub cash_gained: i32,

    pub is_boss: bool,
    pub enemy_cleared: bool,
    pub battle_result: BattleResult,

    pub time_charging_unit: f32,

    pub ui: BattleUi,

    /// DATA.MKF chunk 10 — the battle effect sprite (offensive-hit sprite).
    pub effect_sprite: Vec<u8>,

    pub enemy_moving: bool,
    pub hiding_time: i32,
    pub moving_player_index: u16,
    pub blow: i32,

    /// Current magic frame bitmap (`lpMagicBitmap`), stored as owned RLE bytes.
    pub magic_bitmap: Vec<u8>,

    pub sprite_draw_seq: [BattleSpriteSeq; MAX_BATTLESPRITESEQ_ITEMS],
    pub max_sprite_draw_seq_index: usize,
    pub sprite_add_lock: bool,

    // Classic-mode fields.
    pub phase: BattlePhase,
    pub action_queue: [ActionQueue; MAX_ACTIONQUEUE_ITEMS],
    pub cur_action: usize,
    pub repeat: bool,
    pub force: bool,
    pub flee: bool,
    pub prev_auto_atk: bool,
    pub prev_player_auto_atk: bool,
    pub coop_contributors: [bool; MAX_PLAYERS_IN_PARTY],
    pub this_turn_coop: bool,

    /// Headless test acceleration: when `true`, `battle_delay` and the
    /// animation frame loops skip real waiting and rendering while still
    /// running all game logic.  Battle behaviour is otherwise unchanged.
    pub instant: bool,
}

impl Battle {
    fn new() -> Battle {
        Battle {
            player: Default::default(),
            enemy: Default::default(),
            max_enemy_index: 0,
            scene_buf: Surface::screen(),
            background: Surface::screen(),
            background_color_shift: 0,
            summon_sprite: Vec::new(),
            pos_summon: (0, 0),
            summon_frame: 0,
            summon_color_shift: false,
            exp_gained: 0,
            cash_gained: 0,
            is_boss: false,
            enemy_cleared: false,
            battle_result: BattleResult::PreBattle,
            time_charging_unit: 0.0,
            ui: BattleUi::default(),
            effect_sprite: Vec::new(),
            enemy_moving: false,
            hiding_time: 0,
            moving_player_index: 0,
            blow: 0,
            magic_bitmap: Vec::new(),
            sprite_draw_seq: [BattleSpriteSeq::default(); MAX_BATTLESPRITESEQ_ITEMS],
            max_sprite_draw_seq_index: 0,
            sprite_add_lock: false,
            phase: BattlePhase::SelectAction,
            action_queue: [ActionQueue::default(); MAX_ACTIONQUEUE_ITEMS],
            cur_action: 0,
            repeat: false,
            force: false,
            flee: false,
            prev_auto_atk: false,
            prev_player_auto_atk: false,
            coop_contributors: [false; MAX_PLAYERS_IN_PARTY],
            this_turn_coop: false,
            instant: false,
        }
    }

    /// A cheap, allocation-light placeholder used only transiently: while a
    /// live battle is moved into `Engine::battle` for the duration of a script
    /// call (see `Engine::run_trigger_script_in_battle`), the borrowed slot is
    /// filled with this so nothing observes a half-moved value.  It is never
    /// rendered from and is swapped back out immediately.
    fn placeholder() -> Battle {
        let mut b = Battle::new();
        // Drop the two 320x200 scratch surfaces new() allocated; the
        // placeholder is never drawn.
        b.scene_buf = Surface::new(0, 0);
        b.background = Surface::new(0, 0);
        b
    }
}

// ---------------------------------------------------------------------------
// Positions of players on the battle screen (g_rgPlayerPos).
// ---------------------------------------------------------------------------

pub const PLAYER_POS: [[(i32, i32); 3]; 3] = [
    [(240, 170), (0, 0), (0, 0)],
    [(200, 176), (256, 152), (0, 0)],
    [(180, 180), (234, 170), (270, 146)],
];

// ===========================================================================
// Local RLE blitting helpers (port of the color-shift / mono blits from
// palcommon.c that surface.rs does not yet provide).
// ===========================================================================

fn u16_le(b: &[u8], off: usize) -> u16 {
    b[off] as u16 | ((b[off + 1] as u16) << 8)
}

fn skip_rle_header(rle: &[u8]) -> &[u8] {
    if rle.len() >= 4 && rle[0] == 0x02 && rle[1] == 0 && rle[2] == 0 && rle[3] == 0 {
        &rle[4..]
    } else {
        rle
    }
}

/// PAL_RLEBlitWithColorShift: blit an RLE bitmap, adding `shift` to the low
/// nibble of each source pixel (clamped like the original: >0x70 -> 0x0F).
pub fn blit_rle_color_shift(surf: &mut Surface, rle: &[u8], dx: i32, dy: i32, shift: i32) {
    let rle = skip_rle_header(rle);
    if rle.len() < 4 {
        return;
    }
    let w = u16_le(rle, 0) as usize;
    let h = u16_le(rle, 2) as usize;
    if w == 0 || h == 0 {
        return;
    }
    let total = w * h;
    let mut p = 4usize;
    let mut i = 0usize;
    let mut src_x = 0usize;
    let mut dst_y = dy;
    while i < total && p < rle.len() {
        let t = rle[p];
        p += 1;
        if (t & 0x80) != 0 && (t as usize) <= 0x80 + w {
            let n = (t as usize) - 0x80;
            i += n;
            src_x += n;
            while src_x >= w {
                src_x -= w;
                dst_y += 1;
            }
        } else {
            let n = t as usize;
            if p + n > rle.len() {
                return;
            }
            for k in 0..n {
                let dst_x = dx + src_x as i32;
                let mut b = (rle[p + k] as i32 & 0x0F) + shift;
                if b & 0x80 != 0 {
                    b = 0;
                } else if b & 0x70 != 0 {
                    b = 0x0F;
                }
                let color = (b as u8) | (rle[p + k] & 0xF0);
                surf.put_pixel(dst_x, dst_y, color);
                src_x += 1;
                if src_x >= w {
                    src_x = 0;
                    dst_y += 1;
                }
            }
            p += n;
            i += n;
        }
    }
}

// ===========================================================================
// PAL_BattleDrawBackground.
// ===========================================================================

/// PAL_BattleDrawBackground: copy the background into the scene buffer,
/// applying the background color shift.
pub fn draw_background(battle: &mut Battle) {
    let shift = battle.background_color_shift;
    for (dst, &src) in battle
        .scene_buf
        .pixels
        .iter_mut()
        .zip(battle.background.pixels.iter())
    {
        let mut b = (src & 0x0F) as i16 + shift;
        if b & 0x0080 != 0 {
            b = 0;
        } else if b & 0x0070 != 0 {
            b = 0x0F;
        }
        *dst = (b as u8) | (src & 0xF0);
    }
    // PAL_ApplyWave is a purely visual screen-wave effect; skipped here (not
    // used by the classic-mode logic and not exercised by the tests).
}

// ===========================================================================
// Sprite drawing (PAL_BattleDrawEnemySprites / PlayerSprites / MagicSprites).
// ===========================================================================

/// PAL_BattleDrawEnemySprites.
fn draw_enemy_sprites(battle: &mut Battle, enemy_index: usize) {
    let en = &battle.enemy[enemy_index];
    if en.object_id == 0 {
        return;
    }
    let mut pos = en.pos;
    if en.status[crate::global::STATUS_CONFUSED] > 0
        && en.status[crate::global::STATUS_SLEEP] == 0
        && en.status[crate::global::STATUS_PARALYZED] == 0
    {
        pos.0 += crate::global::random_long(-1, 1);
    }
    let frame = match surface::sprite_frame(&en.sprite, en.current_frame as usize) {
        Some(f) => f.to_vec(),
        None => return,
    };
    let x = pos.0 - surface::rle_width(&frame) as i32 / 2;
    let y = pos.1 - surface::rle_height(&frame) as i32;
    let color_shift = en.color_shift;
    if color_shift != 0 {
        blit_rle_color_shift(&mut battle.scene_buf, &frame, x, y, color_shift);
    } else {
        battle.scene_buf.blit_rle(&frame, x, y);
    }
}

/// PAL_BattleDrawPlayerSprites (`player_index == -1` -> summon god).
fn draw_player_sprites(engine: &Engine, battle: &mut Battle, player_index: i32) {
    if player_index < 0 {
        if !battle.summon_sprite.is_empty() {
            let frame =
                match surface::sprite_frame(&battle.summon_sprite, battle.summon_frame as usize) {
                    Some(f) => f.to_vec(),
                    None => return,
                };
            let x = battle.pos_summon.0 - surface::rle_width(&frame) as i32 / 2;
            let y = battle.pos_summon.1 - surface::rle_height(&frame) as i32;
            battle.scene_buf.blit_rle(&frame, x, y);
        }
        return;
    }
    let pi = player_index as usize;
    let g = &engine.globals;
    let role = g.party[pi].player_role as usize;
    let mut pos = battle.player[pi].pos;
    if g.player_status[role][crate::global::STATUS_CONFUSED] != 0
        && g.player_status[role][crate::global::STATUS_SLEEP] == 0
        && g.player_status[role][crate::global::STATUS_PARALYZED] == 0
        && g.game.player_roles.hp[role] > 0
        && !crate::fight::is_player_dying(g, role)
    {
        pos.1 += crate::global::random_long(-1, 1);
    }
    let frame = match surface::sprite_frame(
        &battle.player[pi].sprite,
        battle.player[pi].current_frame as usize,
    ) {
        Some(f) => f.to_vec(),
        None => return,
    };
    let x = pos.0 - surface::rle_width(&frame) as i32 / 2;
    let y = pos.1 - surface::rle_height(&frame) as i32;
    let color_shift = battle.player[pi].color_shift;
    if color_shift != 0 {
        blit_rle_color_shift(&mut battle.scene_buf, &frame, x, y, color_shift);
    } else if battle.hiding_time == 0 {
        battle.scene_buf.blit_rle(&frame, x, y);
    }
}

/// PAL_BattleDrawMagicSprites.
fn draw_magic_sprites(battle: &mut Battle, pos: (i32, i32)) {
    let bmp = std::mem::take(&mut battle.magic_bitmap);
    let x = pos.0 - surface::rle_width(&bmp) as i32 / 2;
    let y = pos.1 - surface::rle_height(&bmp) as i32;
    battle.scene_buf.blit_rle(&bmp, x, y);
    battle.magic_bitmap = bmp;
}

// ===========================================================================
// Sprite draw sequence management.
// ===========================================================================

/// PAL_BattleClearSpriteObject.
pub fn clear_sprite_object(battle: &mut Battle) {
    battle.sprite_draw_seq = [BattleSpriteSeq::default(); MAX_BATTLESPRITESEQ_ITEMS];
    battle.max_sprite_draw_seq_index = 0;
}

/// PAL_BattleSpriteAddUnlock.
pub fn sprite_add_unlock(battle: &mut Battle) {
    battle.sprite_add_lock = false;
    clear_sprite_object(battle);
}

/// PAL_BattleAddSpriteObject.
pub fn add_sprite_object(
    battle: &mut Battle,
    sprite_type: BattleSpriteType,
    object_index: i32,
    pos: (i32, i32),
    layer_offset: i16,
    have_color_shift: bool,
) {
    let idx = battle.max_sprite_draw_seq_index;
    if idx + 1 < MAX_BATTLESPRITESEQ_ITEMS {
        battle.sprite_draw_seq[idx] = BattleSpriteSeq {
            sprite_type,
            object_index,
            pos,
            layer_offset,
            have_color_shift,
        };
        battle.max_sprite_draw_seq_index += 1;
    }
}

/// PAL_BattleAddFighterSpriteObject.
fn add_fighter_sprite_object(engine: &Engine, battle: &mut Battle) {
    for i in 0..=battle.max_enemy_index as usize {
        add_sprite_object(
            battle,
            BattleSpriteType::Enemy,
            i as i32,
            battle.enemy[i].pos,
            0,
            battle.enemy[i].color_shift != 0,
        );
    }
    if !battle.summon_sprite.is_empty() {
        add_sprite_object(
            battle,
            BattleSpriteType::Player,
            -1,
            battle.pos_summon,
            0,
            battle.summon_color_shift,
        );
    } else {
        for i in 0..=engine.globals.max_party_member_index as usize {
            add_sprite_object(
                battle,
                BattleSpriteType::Player,
                i as i32,
                battle.player[i].pos,
                0,
                battle.player[i].color_shift != 0,
            );
        }
    }
}

/// PAL_BattleSortSpriteObjecByPos: bubble sort by (Y + layer offset), then X.
fn sort_sprite_object_by_pos(battle: &mut Battle) {
    let n = battle.max_sprite_draw_seq_index;
    if n == 0 {
        return;
    }
    for i in 0..n.saturating_sub(1) {
        for j in (i + 1)..n {
            let this_y =
                battle.sprite_draw_seq[i].pos.1 as i16 + battle.sprite_draw_seq[i].layer_offset;
            let next_y =
                battle.sprite_draw_seq[j].pos.1 as i16 + battle.sprite_draw_seq[j].layer_offset;
            if this_y > next_y {
                battle.sprite_draw_seq.swap(i, j);
            } else if this_y == next_y {
                let this_x = battle.sprite_draw_seq[i].pos.0;
                let next_x = battle.sprite_draw_seq[j].pos.0;
                if this_x < next_x {
                    battle.sprite_draw_seq.swap(i, j);
                }
            }
        }
    }
}

/// PAL_BattleDrawAllSpritesWithColorShift.
fn draw_all_sprites_with_color_shift(engine: &Engine, battle: &mut Battle, color_shift: bool) {
    sort_sprite_object_by_pos(battle);
    for i in 0..=battle.max_sprite_draw_seq_index {
        if i >= MAX_BATTLESPRITESEQ_ITEMS {
            break;
        }
        let obj = battle.sprite_draw_seq[i];
        if color_shift && !obj.have_color_shift {
            continue;
        }
        match obj.sprite_type {
            BattleSpriteType::None => {}
            BattleSpriteType::Enemy => draw_enemy_sprites(battle, obj.object_index as usize),
            BattleSpriteType::Player => draw_player_sprites(engine, battle, obj.object_index),
            BattleSpriteType::Magic => draw_magic_sprites(battle, obj.pos),
        }
    }
}

/// PAL_BattleDrawAllSprites.
fn draw_all_sprites(engine: &Engine, battle: &mut Battle) {
    draw_all_sprites_with_color_shift(engine, battle, false);
    draw_all_sprites_with_color_shift(engine, battle, true);
}

/// PAL_BattleMakeScene.
pub fn make_scene(engine: &Engine, battle: &mut Battle) {
    draw_background(battle);
    if battle.sprite_add_lock {
        clear_sprite_object(battle);
    } else {
        battle.sprite_add_lock = true;
    }
    add_fighter_sprite_object(engine, battle);
    draw_all_sprites(engine, battle);
}

/// Copy the scene buffer to the screen (`VIDEO_CopyEntireSurface`).
pub fn copy_scene_to_screen(engine: &mut Engine, battle: &Battle) {
    engine
        .screen
        .pixels
        .copy_from_slice(&battle.scene_buf.pixels);
}

// ===========================================================================
// Scene backup / fade (PAL_BattleBackupScene / PAL_BattleFadeScene).
// ===========================================================================

/// PAL_BattleFadeScene: fade in the battle scene (blend gpScreenBak toward the
/// scene buffer with the classic nibble-step pattern).
pub fn fade_scene(engine: &mut Engine, battle: &mut Battle) {
    if battle.instant {
        copy_scene_to_screen(engine, battle);
        crate::uibattle::ui_update(engine, battle);
        engine.video_update();
        return;
    }
    const RG_INDEX: [usize; 6] = [0, 3, 1, 5, 2, 4];
    let mut time = engine.ticks();
    for i in 0..12 {
        for &start in RG_INDEX.iter() {
            engine.delay_until(time);
            time = engine.ticks() + 16;
            let mut k = start;
            while k < SCREEN_W * SCREEN_H {
                let a = battle.scene_buf.pixels[k];
                let mut b = engine.screen_bak.pixels[k];
                if i > 0 {
                    if (a & 0x0F) > (b & 0x0F) {
                        b = b.wrapping_add(1);
                    } else if (a & 0x0F) < (b & 0x0F) {
                        b = b.wrapping_sub(1);
                    }
                }
                engine.screen_bak.pixels[k] = (a & 0xF0) | (b & 0x0F);
                k += 6;
            }
            engine.restore_screen();
            crate::uibattle::ui_update(engine, battle);
            engine.video_update();
        }
    }
    copy_scene_to_screen(engine, battle);
    crate::uibattle::ui_update(engine, battle);
    engine.video_update();
}

// ===========================================================================
// PAL_LoadBattleSprites / PAL_LoadBattleBackground.
// ===========================================================================

/// PAL_LoadBattleSprites.
pub(crate) fn load_battle_sprites(engine: &mut Engine, battle: &mut Battle) -> std::io::Result<()> {
    // Free previous sprites.
    for p in battle.player.iter_mut() {
        p.sprite = Vec::new();
    }
    for e in battle.enemy.iter_mut() {
        e.sprite = Vec::new();
    }
    battle.summon_sprite = Vec::new();

    let abc = engine.globals.data_dir.mkf("abc.mkf")?;

    // Player battle sprites (F.MKF).
    #[allow(clippy::needless_range_loop)]
    for i in 0..=engine.globals.max_party_member_index as usize {
        let role = engine.globals.party[i].player_role as usize;
        let s = engine.globals.player_battle_sprite(role) as usize;
        if engine.globals.files.f.chunk_size(s) == 0 {
            continue;
        }
        battle.player[i].sprite = engine.globals.files.f.chunk_decompressed(s)?;
        let (x, y) = PLAYER_POS[engine.globals.max_party_member_index as usize][i];
        battle.player[i].pos_original = (x, y);
        battle.player[i].pos = (x, y);
    }

    // Enemy battle sprites (ABC.MKF, YJ_1 compressed -> chunk_decompressed).
    for i in 0..MAX_ENEMIES_IN_TEAM {
        if battle.enemy[i].object_id == 0 {
            continue;
        }
        let enemy_id =
            engine.globals.game.objects[battle.enemy[i].object_id as usize].enemy_id() as usize;
        if abc.chunk_size(enemy_id) == 0 {
            continue;
        }
        battle.enemy[i].sprite = abc.chunk_decompressed(enemy_id)?;
        let (px, py) = engine.globals.game.enemy_pos[i][battle.max_enemy_index as usize];
        let x = px as i32;
        let mut y = py as i32;
        y += battle.enemy[i].e.y_pos_offset as i32;
        battle.enemy[i].pos_original = (x, y);
        battle.enemy[i].pos = (x, y);
    }
    Ok(())
}

/// PAL_LoadBattleBackground.
fn load_battle_background(engine: &mut Engine, battle: &mut Battle) -> std::io::Result<()> {
    let num = engine.globals.num_battle_field as usize;
    let buf = engine.globals.files.fbp.chunk_decompressed(num)?;
    battle.background = Surface::screen();
    battle.background.blit_fbp(&buf);
    Ok(())
}

// ===========================================================================
// PAL_BattleWon: award experience / cash, level up.
// ===========================================================================

/// A "wait for any key or timeout" that degrades to nothing in instant mode.
fn wait_for_any_key(engine: &mut Engine, battle: &Battle, timeout_ms: u64) {
    if battle.instant {
        return;
    }
    let deadline = engine.ticks() + timeout_ms;
    engine.input.clear_key_state();
    while engine.ticks() < deadline {
        engine.process_event();
        if engine.input.pressed(u32::MAX) || engine.quit_requested {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    engine.input.clear_key_state();
}

const BATTLEWIN_GETEXP_LABEL: u16 = 30;
const BATTLEWIN_BEATENEMY_LABEL: u16 = 9;
const BATTLEWIN_DOLLAR_LABEL: u16 = 10;

/// PAL_BattleWon: show the "you win" message and add experience for players.
/// The interactive boxes/level-up display are skipped in instant mode; all
/// stat/EXP mutations run identically.
fn battle_won(engine: &mut Engine, battle: &mut Battle) {
    let mut orig_player_roles: PlayerRoles = engine.globals.game.player_roles;

    engine.backup_screen();

    if battle.exp_gained > 0 {
        engine.play_music(if battle.is_boss { 2 } else { 3 }, false, 0.0);
        if !battle.instant {
            // Summary boxes with the "got experience" / "beat enemy" /
            // "dollar" labels, laid out exactly like PAL_BattleWon.
            let w1 = engine.word_width(BATTLEWIN_GETEXP_LABEL) + 3;
            let ww1 = (w1 - 8) << 3;
            engine.create_single_line_box((83 - ww1, 60), w1, false);
            engine.create_single_line_box((65, 105), 10, false);
            let getexp = engine.texts.word(BATTLEWIN_GETEXP_LABEL as usize);
            let beatenemy = engine.texts.word(BATTLEWIN_BEATENEMY_LABEL as usize);
            let dollar = engine.texts.word(BATTLEWIN_DOLLAR_LABEL as usize);
            engine.draw_text(&getexp, (95 - ww1, 70), 0, false, false);
            engine.draw_text(&beatenemy, (77, 115), 0, false, false);
            engine.draw_text(&dollar, (197, 115), 0, false, false);
            engine.draw_number(
                battle.exp_gained as u32,
                5,
                (182 + ww1, 74),
                crate::ui::NumColor::Yellow,
                crate::ui::NumAlign::Right,
            );
            engine.draw_number(
                battle.cash_gained as u32,
                5,
                (162, 119),
                crate::ui::NumColor::Yellow,
                crate::ui::NumAlign::Mid,
            );
            engine.video_update();
            wait_for_any_key(engine, battle, if battle.is_boss { 5500 } else { 3000 });
        }
    }

    engine.globals.cash += battle.cash_gained as u32;

    let max_party = engine.globals.max_party_member_index as usize;
    for i in 0..=max_party {
        let mut level_up = false;
        let w = engine.globals.party[i].player_role as usize;
        if engine.globals.game.player_roles.hp[w] == 0 {
            continue;
        }

        let mut exp = engine.globals.exp.primary_exp[w].exp as u32;
        exp += battle.exp_gained as u32;

        if engine.globals.game.player_roles.level[w] as usize > MAX_LEVELS {
            engine.globals.game.player_roles.level[w] = MAX_LEVELS as u16;
        }

        while exp
            >= engine.globals.game.level_up_exp[engine.globals.game.player_roles.level[w] as usize]
                as u32
        {
            exp -= engine.globals.game.level_up_exp
                [engine.globals.game.player_roles.level[w] as usize] as u32;
            if (engine.globals.game.player_roles.level[w] as usize) < MAX_LEVELS {
                level_up = true;
                engine.globals.player_level_up(w, 1);
                engine.globals.game.player_roles.hp[w] = engine.globals.game.player_roles.max_hp[w];
                engine.globals.game.player_roles.mp[w] = engine.globals.game.player_roles.max_mp[w];
            }
        }
        engine.globals.exp.primary_exp[w].exp = exp as u16;

        if level_up && !battle.instant {
            wait_for_any_key(engine, battle, 3000);
            orig_player_roles = engine.globals.game.player_roles;
        }

        // Hidden (per-attribute) EXP levels.
        let mut total_count: u32 = 0;
        total_count += engine.globals.exp.attack_exp[w].count as u32;
        total_count += engine.globals.exp.defense_exp[w].count as u32;
        total_count += engine.globals.exp.dexterity_exp[w].count as u32;
        total_count += engine.globals.exp.flee_exp[w].count as u32;
        total_count += engine.globals.exp.health_exp[w].count as u32;
        total_count += engine.globals.exp.magic_exp[w].count as u32;
        total_count += engine.globals.exp.magic_power_exp[w].count as u32;

        if total_count > 0 {
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Health,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Magic,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Attack,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::MagicPower,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Defense,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Dexterity,
                &orig_player_roles,
            );
            check_hidden_exp(
                engine,
                battle,
                w,
                total_count,
                HiddenExp::Flee,
                &orig_player_roles,
            );

            if level_up {
                engine.globals.game.player_roles.hp[w] = engine.globals.game.player_roles.max_hp[w];
                engine.globals.game.player_roles.mp[w] = engine.globals.game.player_roles.max_mp[w];
            }
        }
        orig_player_roles = engine.globals.game.player_roles;

        // Learn all magics for the current level.
        let mut j = 0;
        while j < engine.globals.game.level_up_magics.len() {
            let (mlevel, magic) = engine.globals.game.level_up_magics[j].m[w];
            if magic == 0 || mlevel > engine.globals.game.player_roles.level[w] {
                j += 1;
                continue;
            }
            if engine.globals.add_magic(w, magic) && !battle.instant {
                wait_for_any_key(engine, battle, 3000);
            }
            j += 1;
        }
    }

    // Post-battle scripts.
    for i in 0..=battle.max_enemy_index as usize {
        let s = battle.enemy[i].script_on_battle_end;
        engine.run_trigger_script_in_battle(battle, s, i as u16);
    }

    // Auto-recover: heal half the missing HP/MP.
    for i in 0..=max_party {
        let w = engine.globals.party[i].player_role as usize;
        let hp = engine.globals.game.player_roles.hp[w];
        let max_hp = engine.globals.game.player_roles.max_hp[w];
        let mp = engine.globals.game.player_roles.mp[w];
        let max_mp = engine.globals.game.player_roles.max_mp[w];
        engine.globals.game.player_roles.hp[w] = hp + (max_hp - hp) / 2;
        engine.globals.game.player_roles.mp[w] = mp + (max_mp - mp) / 2;
    }
}

#[derive(Clone, Copy)]
enum HiddenExp {
    Health,
    Magic,
    Attack,
    MagicPower,
    Defense,
    Dexterity,
    Flee,
}

/// The CHECK_HIDDEN_EXP macro from PAL_BattleWon.
fn check_hidden_exp(
    engine: &mut Engine,
    battle: &Battle,
    w: usize,
    total_count: u32,
    kind: HiddenExp,
    _orig: &PlayerRoles,
) {
    let g = &mut engine.globals;
    macro_rules! run {
        ($expfield:ident, $statfield:ident) => {{
            let mut exp: u32 = battle.exp_gained as u32;
            exp = exp.wrapping_mul(g.exp.$expfield[w].count as u32);
            exp /= total_count;
            exp = exp.wrapping_mul(2);
            exp += g.exp.$expfield[w].exp as u32;

            if g.exp.$expfield[w].level as usize > MAX_LEVELS {
                g.exp.$expfield[w].level = MAX_LEVELS as u16;
            }

            while exp >= g.game.level_up_exp[g.exp.$expfield[w].level as usize] as u32 {
                exp -= g.game.level_up_exp[g.exp.$expfield[w].level as usize] as u32;
                g.game.player_roles.$statfield[w] = g.game.player_roles.$statfield[w]
                    .wrapping_add(crate::global::random_long(1, 2) as u16);
                if (g.exp.$expfield[w].level as usize) < MAX_LEVELS {
                    g.exp.$expfield[w].level += 1;
                }
            }
            g.exp.$expfield[w].exp = exp as u16;
        }};
    }
    match kind {
        HiddenExp::Health => run!(health_exp, max_hp),
        HiddenExp::Magic => run!(magic_exp, max_mp),
        HiddenExp::Attack => run!(attack_exp, attack_strength),
        HiddenExp::MagicPower => run!(magic_power_exp, magic_strength),
        HiddenExp::Defense => run!(defense_exp, defense),
        HiddenExp::Dexterity => run!(dexterity_exp, dexterity),
        HiddenExp::Flee => run!(flee_exp, flee_rate),
    }
}

// ===========================================================================
// PAL_BattleEnemyEscape / PAL_BattlePlayerEscape.
// ===========================================================================

/// PAL_BattleEnemyEscape.
pub fn enemy_escape(engine: &mut Engine, battle: &mut Battle) {
    engine.play_sound(45);
    let mut f = true;
    while f {
        f = false;
        for j in 0..=battle.max_enemy_index as usize {
            if battle.enemy[j].object_id == 0 {
                continue;
            }
            let x = battle.enemy[j].pos.0 - 5;
            let y = battle.enemy[j].pos.1;
            battle.enemy[j].pos = (x, y);
            let w = surface::sprite_frame(&battle.enemy[j].sprite, 0)
                .map(surface::rle_width)
                .unwrap_or(0) as i32;
            if x + w > 0 {
                f = true;
            }
        }
        if !battle.instant {
            make_scene(engine, battle);
            copy_scene_to_screen(engine, battle);
            engine.video_update();
            engine.delay(10);
        }
    }
    if !battle.instant {
        engine.delay(500);
    }
    battle.battle_result = BattleResult::Terminated;
}

/// PAL_BattlePlayerEscape.
pub fn player_escape(engine: &mut Engine, battle: &mut Battle) {
    engine.play_sound(45);
    crate::fight::battle_update_fighters(engine, battle);

    let max_party = engine.globals.max_party_member_index as usize;
    for i in 0..=max_party {
        let role = engine.globals.party[i].player_role as usize;
        if engine.globals.game.player_roles.hp[role] > 0 {
            battle.player[i].current_frame = 0;
        }
    }

    for _ in 0..16 {
        for j in 0..=max_party {
            let role = engine.globals.party[j].player_role as usize;
            if engine.globals.game.player_roles.hp[role] == 0 {
                continue;
            }
            let (dx, dy) = match j {
                0 if max_party > 0 => (4, 6),
                0 | 1 => (4, 4),
                2 => (6, 3),
                _ => (0, 0),
            };
            battle.player[j].pos = (battle.player[j].pos.0 + dx, battle.player[j].pos.1 + dy);
        }
        crate::fight::battle_delay(engine, battle, 1, 0, false);
    }

    for i in 0..=max_party {
        battle.player[i].pos = (9999, 9999);
    }
    crate::fight::battle_delay(engine, battle, 1, 0, false);
    battle.battle_result = BattleResult::Fleed;
}

// ===========================================================================
// PAL_BattleMain.
// ===========================================================================

fn battle_main(engine: &mut Engine, battle: &mut Battle) -> BattleResult {
    engine.backup_screen();

    make_scene(engine, battle);
    copy_scene_to_screen(engine, battle);

    engine.play_music(0, false, 1.0);
    if !battle.instant {
        engine.delay(200);
    }

    if !battle.instant {
        engine.backup_screen();
        engine.switch_screen(5);
    }

    engine.play_music(engine.globals.num_battle_music as i32, true, 0.0);

    if engine.globals.need_to_fade_in {
        if !battle.instant {
            engine.fade_in(
                engine.globals.num_palette as usize,
                engine.globals.night_palette,
                1,
            );
        }
        engine.globals.need_to_fade_in = false;
    }

    // Pre-battle scripts for each enemy.
    for i in 0..=battle.max_enemy_index as usize {
        let s = battle.enemy[i].script_on_turn_start;
        battle.enemy[i].script_on_turn_start =
            engine.run_trigger_script_in_battle(battle, s, i as u16);
        if battle.battle_result != BattleResult::PreBattle {
            break;
        }
    }
    if battle.battle_result == BattleResult::PreBattle {
        battle.battle_result = BattleResult::OnGoing;
    }

    let mut time = engine.ticks();
    engine.input.clear_key_state();

    let mut guard = 0u64;
    loop {
        if battle.battle_result != BattleResult::OnGoing {
            break;
        }
        if !battle.instant {
            engine.delay_until(time);
            time = engine.ticks() + BATTLE_FRAME_TIME;
        }
        crate::fight::battle_start_frame(engine, battle);
        if !battle.instant {
            engine.video_update();
        }
        // Safety guard so a stuck battle can never spin forever in headless
        // test mode.
        guard += 1;
        if battle.instant && guard > 2_000_000 {
            battle.battle_result = BattleResult::Terminated;
            break;
        }
    }
    battle.battle_result
}

// ===========================================================================
// PAL_StartBattle (the public entry point) — kept as an Engine method to match
// the existing contract.
// ===========================================================================

impl Engine {
    /// PAL_StartBattle.
    pub fn start_battle(&mut self, enemy_team: u16, is_boss: bool) -> BattleResult {
        self.start_battle_ex(enemy_team, is_boss, false)
    }

    /// PAL_StartBattle with the headless `instant` acceleration flag exposed
    /// for tests.
    pub fn start_battle_ex(
        &mut self,
        enemy_team: u16,
        is_boss: bool,
        instant: bool,
    ) -> BattleResult {
        // Home the battle in the engine (single owner) so that any script
        // opcode running during the battle can reach it.  `start_battle_impl`
        // takes it back out into a local `&mut Battle` for the internal
        // two-argument routines; the two mechanisms are reconciled by
        // `run_trigger_script_in_battle`.
        let mut battle = Box::new(Battle::new());
        battle.instant = instant || self.battle_instant;
        self.battle = Some(battle);
        let result = start_battle_impl(self, enemy_team, is_boss);
        // Battle is over; the transient state must not outlive it.
        self.battle = None;
        result
    }

    /// Headless verification helper: set up a battle against `enemy_team`
    /// exactly like the pre-loop part of PAL_StartBattle and return the
    /// composed battle scene (background + enemies + players), without
    /// running the battle. Returns None if the team has no enemies.
    pub fn compose_battle_scene(&mut self, enemy_team: u16, is_boss: bool) -> Option<Surface> {
        let mut battle = Box::new(Battle::new());
        battle.instant = true;
        let b = &mut *battle;

        let mut i = 0usize;
        for j in 0..MAX_ENEMIES_IN_TEAM {
            let w = self.globals.game.enemy_teams[enemy_team as usize].enemy[j];
            if w == 0xFFFF || w == 0 {
                continue;
            }
            let enemy_id = self.globals.game.objects[w as usize].enemy_id() as usize;
            b.enemy[i].e = self.globals.game.enemies[enemy_id];
            b.enemy[i].state = FighterState::Wait;
            b.enemy[i].object_id = w;
            i += 1;
        }
        if i == 0 {
            return None;
        }
        b.max_enemy_index = i as u16 - 1;
        b.is_boss = is_boss;

        load_battle_sprites(self, b).ok()?;
        load_battle_background(self, b).ok()?;
        b.scene_buf = Surface::screen();
        make_scene(self, b);
        Some(std::mem::replace(&mut b.scene_buf, Surface::new(1, 1)))
    }

    /// Run a trigger script with the live `battle` visible to battle opcodes.
    ///
    /// The internal battle routines hold the battle as a plain `&mut Battle`
    /// (not as the owning `Box` in `self.battle`, which was moved into a local
    /// by `start_battle_impl`).  To let a re-entered script observe it, the
    /// battle value is swapped into `self.battle` for the duration of the
    /// script and swapped back afterwards.  This guarantees `self.battle` is
    /// `Some(current battle)` for every `run_trigger_script` executed from
    /// within a battle, so opcodes can never see a missing or stale battle.
    pub fn run_trigger_script_in_battle(
        &mut self,
        battle: &mut Battle,
        script_entry: u16,
        event_object_id: u16,
    ) -> u16 {
        debug_assert!(
            self.battle.is_none(),
            "run_trigger_script_in_battle re-entered with a battle already installed"
        );
        let taken = std::mem::replace(battle, Battle::placeholder());
        self.battle = Some(Box::new(taken));
        let ret = self.run_trigger_script(script_entry, event_object_id);
        *battle = *self
            .battle
            .take()
            .expect("battle vanished during trigger script");
        ret
    }
}

fn start_battle_impl(engine: &mut Engine, enemy_team: u16, is_boss: bool) -> BattleResult {
    let mut battle = engine
        .battle
        .take()
        .expect("start_battle_ex must install engine.battle before start_battle_impl");
    let battle = &mut *battle;
    // Screen waving effects.
    let prev_wave_level = engine.globals.screen_wave;
    let prev_wave_progression = engine.globals.wave_progression;
    engine.globals.wave_progression = 0;
    engine.globals.screen_wave =
        engine.globals.game.battle_fields[engine.globals.num_battle_field as usize].screen_wave;

    // Make sure everyone is alive; clear hidden EXP counts.
    let max_party = engine.globals.max_party_member_index as usize;
    for i in 0..=max_party {
        let w = engine.globals.party[i].player_role as usize;
        if engine.globals.game.player_roles.hp[w] == 0 {
            engine.globals.game.player_roles.hp[w] = 1;
            engine.globals.player_status[w][STATUS_PUPPET] = 0;
        }
        engine.globals.exp.health_exp[w].count = 0;
        engine.globals.exp.magic_exp[w].count = 0;
        engine.globals.exp.attack_exp[w].count = 0;
        engine.globals.exp.magic_power_exp[w].count = 0;
        engine.globals.exp.defense_exp[w].count = 0;
        engine.globals.exp.dexterity_exp[w].count = 0;
        engine.globals.exp.flee_exp[w].count = 0;
    }

    // Clear item-using records.
    for i in 0..MAX_INVENTORY {
        engine.globals.inventory[i].amount_in_use = 0;
    }

    // Store all enemies (classic: no non-classic dexterity HACKs).
    let mut i = 0usize;
    for j in 0..MAX_ENEMIES_IN_TEAM {
        battle.enemy[j] = BattleEnemy::default();
        let w = engine.globals.game.enemy_teams[enemy_team as usize].enemy[j];
        if w == 0xFFFF {
            continue;
        }
        if w != 0 {
            let enemy_id = engine.globals.game.objects[w as usize].enemy_id() as usize;
            battle.enemy[i].e = engine.globals.game.enemies[enemy_id];
            battle.enemy[i].state = FighterState::Wait;
            battle.enemy[i].script_on_turn_start =
                engine.globals.game.objects[w as usize].enemy_script_on_turn_start();
            battle.enemy[i].script_on_battle_end =
                engine.globals.game.objects[w as usize].enemy_script_on_battle_end();
            battle.enemy[i].script_on_ready =
                engine.globals.game.objects[w as usize].enemy_script_on_ready();
            battle.enemy[i].color_shift = 0;
        }
        battle.enemy[i].object_id = w;
        i += 1;
    }
    battle.max_enemy_index = i.saturating_sub(1) as u16;

    // Store all players.
    for i in 0..=max_party {
        battle.player[i].time_meter = 15.0;
        battle.player[i].hiding_time = 0;
        battle.player[i].state = FighterState::Wait;
        battle.player[i].defending = false;
        battle.player[i].current_frame = 0;
        battle.player[i].color_shift = 0;
    }

    // Load sprites, background, scene buffer.
    if let Err(e) = load_battle_sprites(engine, battle) {
        eprintln!("PAL_LoadBattleSprites failed: {e}");
    }
    if let Err(e) = load_battle_background(engine, battle) {
        eprintln!("PAL_LoadBattleBackground failed: {e}");
    }
    battle.scene_buf = Surface::screen();

    engine.update_equipments();

    battle.exp_gained = 0;
    battle.cash_gained = 0;
    battle.is_boss = is_boss;
    battle.enemy_cleared = false;
    battle.enemy_moving = false;
    battle.hiding_time = 0;
    battle.moving_player_index = 0;

    battle.ui = BattleUi::default();

    battle.summon_sprite = Vec::new();
    battle.background_color_shift = 0;

    engine.globals.in_battle = true;
    battle.battle_result = BattleResult::PreBattle;
    battle.sprite_add_lock = true;

    crate::fight::battle_update_fighters(engine, battle);

    // Load the battle effect sprite (DATA.MKF chunk 10).
    match engine.globals.files.data.chunk(10) {
        Ok(chunk) => battle.effect_sprite = chunk.to_vec(),
        Err(e) => eprintln!("failed to read battle effect sprite: {e}"),
    }

    battle.phase = BattlePhase::SelectAction;
    battle.repeat = false;
    battle.force = false;
    battle.flee = false;
    battle.prev_auto_atk = false;
    battle.this_turn_coop = false;

    // Run the main battle routine.
    let result = battle_main(engine, battle);

    if result == BattleResult::Won {
        battle_won(engine, battle);
    }

    // Clear item-using records.
    for w in 0..MAX_INVENTORY {
        engine.globals.inventory[w].amount_in_use = 0;
    }

    // Clear player status, poisons, temporary effects.
    engine.globals.clear_all_player_status();
    for w in 0..crate::global::MAX_PLAYER_ROLES {
        engine.globals.cure_poison_by_level(w as u16, 3);
        engine
            .globals
            .remove_equipment_effect(w as u16, crate::global::BODYPART_EXTRA);
    }

    engine.globals.in_battle = false;
    engine.play_music(engine.globals.num_music as i32, true, 1.0);

    engine.globals.wave_progression = prev_wave_progression;
    engine.globals.screen_wave = prev_wave_level;

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::seed_random;

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        Engine::new(true).expect("headless engine")
    }

    /// Load a real enemy team and verify the enemies are stored with the
    /// correct stats pulled from DATA.MKF.
    #[test]
    fn start_battle_loads_real_enemy_team() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;

        // Find a real, non-empty enemy team (index 0 is a valid team too).
        let team = e
            .globals
            .game
            .enemy_teams
            .iter()
            .position(|t| t.enemy.iter().any(|&w| w != 0 && w != 0xFFFF))
            .expect("no non-empty enemy team found");

        let mut battle = Box::new(Battle::new());
        battle.instant = true;

        // Reproduce the enemy-loading portion of PAL_StartBattle.
        let mut i = 0usize;
        for j in 0..MAX_ENEMIES_IN_TEAM {
            battle.enemy[j] = BattleEnemy::default();
            let w = e.globals.game.enemy_teams[team].enemy[j];
            if w == 0xFFFF {
                continue;
            }
            if w != 0 {
                let enemy_id = e.globals.game.objects[w as usize].enemy_id() as usize;
                battle.enemy[i].e = e.globals.game.enemies[enemy_id];
            }
            battle.enemy[i].object_id = w;
            i += 1;
        }
        battle.max_enemy_index = i.saturating_sub(1) as u16;

        let mut found = 0;
        for k in 0..=battle.max_enemy_index as usize {
            if battle.enemy[k].object_id != 0 {
                found += 1;
                // Health is loaded from the enemy record.
                let enemy_id =
                    e.globals.game.objects[battle.enemy[k].object_id as usize].enemy_id() as usize;
                assert_eq!(
                    battle.enemy[k].e.health,
                    e.globals.game.enemies[enemy_id].health
                );
            }
        }
        assert!(found > 0, "expected at least one enemy loaded");
    }

    /// A full simulated auto-battle against a weak real enemy team must
    /// terminate with a definite result in a bounded number of iterations.
    #[test]
    fn auto_battle_terminates() {
        seed_random(12345);
        let mut e = engine();
        e.globals.load_default_game().unwrap();

        // Single, strong, magic-less party member so auto-attack picks
        // physical attacks (magic does nothing without the script layer).
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;
        for i in 0..crate::global::MAX_PLAYER_MAGICS {
            e.globals.game.player_roles.magic[i][0] = 0;
        }
        e.globals.game.player_roles.hp[0] = 999;
        e.globals.game.player_roles.max_hp[0] = 999;
        e.globals.game.player_roles.attack_strength[0] = 800;
        e.globals.game.player_roles.dexterity[0] = 200;
        e.globals.auto_battle = true;

        // Pick the enemy team with the weakest total health.
        let mut best_team = 0usize;
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
                best_team = idx;
            }
        }
        assert!(best_team > 0, "no suitable enemy team");

        let result = e.start_battle_ex(best_team as u16, false, true);
        assert!(
            matches!(
                result,
                BattleResult::Won | BattleResult::Lost | BattleResult::Terminated
            ),
            "unexpected battle result: {result:?}"
        );
    }
}
