//! Global game state and data (port of SDLPAL global.h / global.c, DOS
//! paths). Instead of C globals, everything lives in the `Globals` struct.
//!
//! Functions from global.c that need the script interpreter
//! (PAL_UpdateEquipments, PAL_AddPoisonForPlayer) live in the script layer.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::data::DataDir;
use crate::mkf::Mkf;

pub const MAX_PLAYERS_IN_PARTY: usize = 3;
pub const MAX_PLAYER_ROLES: usize = 6;
pub const MAX_PLAYABLE_PLAYER_ROLES: usize = 5;
pub const MAX_INVENTORY: usize = 256;
pub const MAX_STORE_ITEM: usize = 9;
pub const NUM_MAGIC_ELEMENTAL: usize = 5;
pub const MAX_ENEMIES_IN_TEAM: usize = 5;
pub const MAX_PLAYER_EQUIPMENTS: usize = 6;
pub const MAX_PLAYER_MAGICS: usize = 32;
pub const MAX_SCENES: usize = 300;
pub const MAX_OBJECTS: usize = 600;
pub const MAX_EVENT_OBJECTS: usize = 5500;
pub const MAX_POISONS: usize = 16;
pub const MAX_LEVELS: usize = 99;
pub const MINIMAL_WORD_COUNT: usize = MAX_OBJECTS + 13;

// Load flags (res.h).
pub const LOAD_GLOBAL_DATA: u8 = 1 << 0;
pub const LOAD_SCENE: u8 = 1 << 1;
pub const LOAD_PLAYER_SPRITE: u8 = 1 << 2;

// Player status IDs (PAL_CLASSIC variant).
pub const STATUS_CONFUSED: usize = 0;
pub const STATUS_PARALYZED: usize = 1;
pub const STATUS_SLEEP: usize = 2;
pub const STATUS_SILENCE: usize = 3;
pub const STATUS_PUPPET: usize = 4;
pub const STATUS_BRAVERY: usize = 5;
pub const STATUS_PROTECT: usize = 6;
pub const STATUS_HASTE: usize = 7;
pub const STATUS_DUALATTACK: usize = 8;
pub const STATUS_ALL: usize = 9;

// Body parts of equipment.
pub const BODYPART_HEAD: usize = 0;
pub const BODYPART_BODY: usize = 1;
pub const BODYPART_SHOULDER: usize = 2;
pub const BODYPART_HAND: usize = 3;
pub const BODYPART_FEET: usize = 4;
pub const BODYPART_WEAR: usize = 5;
pub const BODYPART_EXTRA: usize = 6;

// Object state (sState of EVENTOBJECT).
pub const OBJSTATE_HIDDEN: i16 = 0;
pub const OBJSTATE_NORMAL: i16 = 1;
pub const OBJSTATE_BLOCKER: i16 = 2;

// Trigger modes.
pub const TRIGGER_NONE: u16 = 0;
pub const TRIGGER_SEARCH_NEAR: u16 = 1;
pub const TRIGGER_SEARCH_NORMAL: u16 = 2;
pub const TRIGGER_SEARCH_FAR: u16 = 3;
pub const TRIGGER_TOUCH_NEAR: u16 = 4;
pub const TRIGGER_TOUCH_NORMAL: u16 = 5;
pub const TRIGGER_TOUCH_FAR: u16 = 6;
pub const TRIGGER_TOUCH_FARTHER: u16 = 7;
pub const TRIGGER_TOUCH_FARTHEST: u16 = 8;

// Item flags.
pub const ITEMFLAG_USABLE: u16 = 1 << 0;
pub const ITEMFLAG_EQUIPABLE: u16 = 1 << 1;
pub const ITEMFLAG_THROWABLE: u16 = 1 << 2;
pub const ITEMFLAG_CONSUMING: u16 = 1 << 3;
pub const ITEMFLAG_APPLY_TO_ALL: u16 = 1 << 4;
pub const ITEMFLAG_SELLABLE: u16 = 1 << 5;
pub const ITEMFLAG_EQUIPABLE_BY_FIRST: u16 = 1 << 6;

// Magic flags.
pub const MAGICFLAG_USABLE_OUTSIDE_BATTLE: u16 = 1 << 0;
pub const MAGICFLAG_USABLE_IN_BATTLE: u16 = 1 << 1;
pub const MAGICFLAG_USABLE_TO_ENEMY: u16 = 1 << 3;
pub const MAGICFLAG_APPLY_TO_ALL: u16 = 1 << 4;

// Magic types.
pub const MAGICTYPE_NORMAL: u16 = 0;
pub const MAGICTYPE_ATTACKALL: u16 = 1;
pub const MAGICTYPE_ATTACKWHOLE: u16 = 2;
pub const MAGICTYPE_ATTACKFIELD: u16 = 3;
pub const MAGICTYPE_APPLYTOPLAYER: u16 = 4;
pub const MAGICTYPE_APPLYTOPARTY: u16 = 5;
pub const MAGICTYPE_TRANCE: u16 = 8;
pub const MAGICTYPE_SUMMON: u16 = 9;

// Directions (palcommon.h kDirSouth..).
pub const DIR_SOUTH: u16 = 0;
pub const DIR_WEST: u16 = 1;
pub const DIR_NORTH: u16 = 2;
pub const DIR_EAST: u16 = 3;

// ===========================================================================
// Random number generator (port of util.c lrand/RandomLong).
// ===========================================================================

static RNG_SEED: AtomicI32 = AtomicI32::new(0);

pub fn seed_random(seed: i32) {
    RNG_SEED.store(seed, Ordering::Relaxed);
}

/// One iteration of util.c's LCG (glSeed = 1664525*glSeed + 1013904223),
/// with C's silent 32-bit wraparound.
fn lcg_step(seed: i32) -> i32 {
    seed.wrapping_mul(1664525).wrapping_add(1013904223)
}

fn lrand() -> i32 {
    let mut seed = RNG_SEED.load(Ordering::Relaxed);
    if seed == 0 {
        // Cold start: C's lrand() calls lsrand(time(NULL)) — which itself does
        // one LCG step — and then falls through to lrand()'s own LCG step. So
        // the first draw advances the generator TWICE from the wall-clock seed,
        // not once.
        let time = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i32)
            .unwrap_or(1);
        seed = lcg_step(time);
    }
    seed = lcg_step(seed);
    RNG_SEED.store(seed, Ordering::Relaxed);
    (seed >> 1).wrapping_add(1073741824)
}

/// Random integer in [from, to] inclusive (RandomLong).
pub fn random_long(from: i32, to: i32) -> i32 {
    if to <= from {
        return from;
    }
    from + lrand() / (i32::MAX / (to - from + 1))
}

/// Random float in [from, to) (RandomFloat).
pub fn random_float(from: f32, to: f32) -> f32 {
    if to <= from {
        return from;
    }
    from + lrand() as f32 / (i32::MAX as f32 / (to - from))
}

// ===========================================================================
// Little-endian word stream helpers.
// ===========================================================================

pub struct WordReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> WordReader<'a> {
    pub fn new(buf: &'a [u8]) -> WordReader<'a> {
        WordReader { buf, pos: 0 }
    }

    /// Read a u16; missing bytes read as 0 (like fread into a zeroed struct).
    pub fn u16(&mut self) -> u16 {
        let v = if self.pos + 1 < self.buf.len() {
            u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]])
        } else {
            0
        };
        self.pos += 2;
        v
    }

    pub fn i16(&mut self) -> i16 {
        self.u16() as i16
    }

    pub fn u32(&mut self) -> u32 {
        let lo = self.u16() as u32;
        let hi = self.u16() as u32;
        lo | (hi << 16)
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn pos(&self) -> usize {
        self.pos
    }
}

#[derive(Default)]
pub struct WordWriter {
    pub buf: Vec<u8>,
}

impl WordWriter {
    pub fn new() -> WordWriter {
        WordWriter { buf: Vec::new() }
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }

    pub fn u32(&mut self, v: u32) {
        self.u16(v as u16);
        self.u16((v >> 16) as u16);
    }
}

// ===========================================================================
// Game data structures.
// ===========================================================================

/// EVENTOBJECT — 16 words / 32 bytes.
#[derive(Clone, Copy, Default, Debug)]
pub struct EventObject {
    pub vanish_time: i16,
    pub x: u16,
    pub y: u16,
    pub layer: i16,
    pub trigger_script: u16,
    pub auto_script: u16,
    pub state: i16,
    pub trigger_mode: u16,
    pub sprite_num: u16,
    pub sprite_frames: u16,
    pub direction: u16,
    pub current_frame_num: u16,
    pub script_idle_frame: u16,
    pub sprite_ptr_offset: u16,
    pub sprite_frames_auto: u16,
    pub script_idle_frame_count_auto: u16,
}

impl EventObject {
    pub const BYTES: usize = 32;

    fn read(r: &mut WordReader) -> EventObject {
        EventObject {
            vanish_time: r.i16(),
            x: r.u16(),
            y: r.u16(),
            layer: r.i16(),
            trigger_script: r.u16(),
            auto_script: r.u16(),
            state: r.i16(),
            trigger_mode: r.u16(),
            sprite_num: r.u16(),
            sprite_frames: r.u16(),
            direction: r.u16(),
            current_frame_num: r.u16(),
            script_idle_frame: r.u16(),
            sprite_ptr_offset: r.u16(),
            sprite_frames_auto: r.u16(),
            script_idle_frame_count_auto: r.u16(),
        }
    }

    fn write(&self, w: &mut WordWriter) {
        w.i16(self.vanish_time);
        w.u16(self.x);
        w.u16(self.y);
        w.i16(self.layer);
        w.u16(self.trigger_script);
        w.u16(self.auto_script);
        w.i16(self.state);
        w.u16(self.trigger_mode);
        w.u16(self.sprite_num);
        w.u16(self.sprite_frames);
        w.u16(self.direction);
        w.u16(self.current_frame_num);
        w.u16(self.script_idle_frame);
        w.u16(self.sprite_ptr_offset);
        w.u16(self.sprite_frames_auto);
        w.u16(self.script_idle_frame_count_auto);
    }
}

/// SCENE — 4 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Scene {
    pub map_num: u16,
    pub script_on_enter: u16,
    pub script_on_teleport: u16,
    pub event_object_index: u16,
}

impl Scene {
    fn read(r: &mut WordReader) -> Scene {
        Scene {
            map_num: r.u16(),
            script_on_enter: r.u16(),
            script_on_teleport: r.u16(),
            event_object_index: r.u16(),
        }
    }

    fn write(&self, w: &mut WordWriter) {
        w.u16(self.map_num);
        w.u16(self.script_on_enter);
        w.u16(self.script_on_teleport);
        w.u16(self.event_object_index);
    }
}

/// OBJECT — the WIN-style 7-word union. The DOS data on disk is 6 words and
/// is converted on load exactly like PAL_LoadDefaultGame / PAL_LoadGame_DOS:
/// data[6] = dos[5] (wFlags), data[5] = 0.
#[derive(Clone, Copy, Default, Debug)]
pub struct GameObject {
    pub data: [u16; 7],
}

impl GameObject {
    fn from_dos(dos: [u16; 6]) -> GameObject {
        let mut data = [0u16; 7];
        data[..6].copy_from_slice(&dos);
        data[6] = dos[5];
        data[5] = 0;
        GameObject { data }
    }

    fn to_dos(self) -> [u16; 6] {
        let mut dos = [0u16; 6];
        dos.copy_from_slice(&self.data[..6]);
        dos[5] = self.data[6];
        dos
    }

    // OBJECT_PLAYER view.
    pub fn player_script_on_friend_death(&self) -> u16 {
        self.data[2]
    }
    pub fn player_script_on_dying(&self) -> u16 {
        self.data[3]
    }
    pub fn set_player_script_on_friend_death(&mut self, v: u16) {
        self.data[2] = v;
    }
    pub fn set_player_script_on_dying(&mut self, v: u16) {
        self.data[3] = v;
    }

    // OBJECT_ITEM view (WIN layout: bitmap, price, use, equip, throw, desc, flags).
    pub fn item_bitmap(&self) -> u16 {
        self.data[0]
    }
    pub fn item_price(&self) -> u16 {
        self.data[1]
    }
    pub fn item_script_on_use(&self) -> u16 {
        self.data[2]
    }
    pub fn item_script_on_equip(&self) -> u16 {
        self.data[3]
    }
    pub fn item_script_on_throw(&self) -> u16 {
        self.data[4]
    }
    pub fn item_flags(&self) -> u16 {
        self.data[6]
    }
    pub fn set_item_script_on_use(&mut self, v: u16) {
        self.data[2] = v;
    }
    pub fn set_item_script_on_equip(&mut self, v: u16) {
        self.data[3] = v;
    }
    pub fn set_item_script_on_throw(&mut self, v: u16) {
        self.data[4] = v;
    }

    // OBJECT_MAGIC view (WIN layout: magicnum, res1, success, use, desc, res2, flags).
    pub fn magic_number(&self) -> u16 {
        self.data[0]
    }
    pub fn magic_script_on_success(&self) -> u16 {
        self.data[2]
    }
    pub fn magic_script_on_use(&self) -> u16 {
        self.data[3]
    }
    pub fn magic_flags(&self) -> u16 {
        self.data[6]
    }
    pub fn set_magic_script_on_success(&mut self, v: u16) {
        self.data[2] = v;
    }
    pub fn set_magic_script_on_use(&mut self, v: u16) {
        self.data[3] = v;
    }

    // OBJECT_ENEMY view.
    pub fn enemy_id(&self) -> u16 {
        self.data[0]
    }
    pub fn enemy_resistance_to_sorcery(&self) -> u16 {
        self.data[1]
    }
    pub fn enemy_script_on_turn_start(&self) -> u16 {
        self.data[2]
    }
    pub fn enemy_script_on_battle_end(&self) -> u16 {
        self.data[3]
    }
    pub fn enemy_script_on_ready(&self) -> u16 {
        self.data[4]
    }
    pub fn set_enemy_script_on_turn_start(&mut self, v: u16) {
        self.data[2] = v;
    }
    pub fn set_enemy_script_on_battle_end(&mut self, v: u16) {
        self.data[3] = v;
    }
    pub fn set_enemy_script_on_ready(&mut self, v: u16) {
        self.data[4] = v;
    }

    // OBJECT_POISON view.
    pub fn poison_level(&self) -> u16 {
        self.data[0]
    }
    pub fn poison_color(&self) -> u16 {
        self.data[1]
    }
    pub fn poison_player_script(&self) -> u16 {
        self.data[2]
    }
    pub fn poison_enemy_script(&self) -> u16 {
        self.data[4]
    }
    pub fn set_poison_player_script(&mut self, v: u16) {
        self.data[2] = v;
    }
    pub fn set_poison_enemy_script(&mut self, v: u16) {
        self.data[4] = v;
    }
}

/// SCRIPTENTRY — 4 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct ScriptEntry {
    pub operation: u16,
    pub operand: [u16; 3],
}

/// INVENTORY — 3 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Inventory {
    pub item: u16,
    pub amount: u16,
    pub amount_in_use: u16,
}

/// STORE — 9 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Store {
    pub items: [u16; MAX_STORE_ITEM],
}

/// ENEMY — 35 words / 70 bytes.
#[derive(Clone, Copy, Default, Debug)]
pub struct Enemy {
    pub idle_frames: u16,
    pub magic_frames: u16,
    pub attack_frames: u16,
    pub idle_anim_speed: u16,
    pub act_wait_frames: u16,
    pub y_pos_offset: u16,
    pub attack_sound: i16,
    pub action_sound: i16,
    pub magic_sound: i16,
    pub death_sound: i16,
    pub call_sound: i16,
    pub health: u16,
    pub exp: u16,
    pub cash: u16,
    pub level: u16,
    pub magic: u16,
    pub magic_rate: u16,
    pub attack_equiv_item: u16,
    pub attack_equiv_item_rate: u16,
    pub steal_item: u16,
    pub steal_item_count: u16,
    pub attack_strength: u16,
    pub magic_strength: u16,
    pub defense: u16,
    pub dexterity: u16,
    pub flee_rate: u16,
    pub poison_resistance: u16,
    pub elem_resistance: [u16; NUM_MAGIC_ELEMENTAL],
    pub physical_resistance: u16,
    pub dual_move: u16,
    pub collect_value: u16,
}

impl Enemy {
    pub const BYTES: usize = 70;

    fn read(r: &mut WordReader) -> Enemy {
        Enemy {
            idle_frames: r.u16(),
            magic_frames: r.u16(),
            attack_frames: r.u16(),
            idle_anim_speed: r.u16(),
            act_wait_frames: r.u16(),
            y_pos_offset: r.u16(),
            attack_sound: r.i16(),
            action_sound: r.i16(),
            magic_sound: r.i16(),
            death_sound: r.i16(),
            call_sound: r.i16(),
            health: r.u16(),
            exp: r.u16(),
            cash: r.u16(),
            level: r.u16(),
            magic: r.u16(),
            magic_rate: r.u16(),
            attack_equiv_item: r.u16(),
            attack_equiv_item_rate: r.u16(),
            steal_item: r.u16(),
            steal_item_count: r.u16(),
            attack_strength: r.u16(),
            magic_strength: r.u16(),
            defense: r.u16(),
            dexterity: r.u16(),
            flee_rate: r.u16(),
            poison_resistance: r.u16(),
            elem_resistance: [r.u16(), r.u16(), r.u16(), r.u16(), r.u16()],
            physical_resistance: r.u16(),
            dual_move: r.u16(),
            collect_value: r.u16(),
        }
    }
}

/// ENEMYTEAM — 5 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct EnemyTeam {
    pub enemy: [u16; MAX_ENEMIES_IN_TEAM],
}

/// PLAYERROLES — 75 rows of 6 words = 900 bytes.
#[derive(Clone, Copy, Debug)]
pub struct PlayerRoles {
    pub avatar: [u16; MAX_PLAYER_ROLES],
    pub sprite_num_in_battle: [u16; MAX_PLAYER_ROLES],
    pub sprite_num: [u16; MAX_PLAYER_ROLES],
    pub name: [u16; MAX_PLAYER_ROLES],
    pub attack_all: [u16; MAX_PLAYER_ROLES],
    pub unknown1: [u16; MAX_PLAYER_ROLES],
    pub level: [u16; MAX_PLAYER_ROLES],
    pub max_hp: [u16; MAX_PLAYER_ROLES],
    pub max_mp: [u16; MAX_PLAYER_ROLES],
    pub hp: [u16; MAX_PLAYER_ROLES],
    pub mp: [u16; MAX_PLAYER_ROLES],
    pub equipment: [[u16; MAX_PLAYER_ROLES]; MAX_PLAYER_EQUIPMENTS],
    pub attack_strength: [u16; MAX_PLAYER_ROLES],
    pub magic_strength: [u16; MAX_PLAYER_ROLES],
    pub defense: [u16; MAX_PLAYER_ROLES],
    pub dexterity: [u16; MAX_PLAYER_ROLES],
    pub flee_rate: [u16; MAX_PLAYER_ROLES],
    pub poison_resistance: [u16; MAX_PLAYER_ROLES],
    pub elemental_resistance: [[u16; MAX_PLAYER_ROLES]; NUM_MAGIC_ELEMENTAL],
    pub unknown2: [u16; MAX_PLAYER_ROLES],
    pub unknown3: [u16; MAX_PLAYER_ROLES],
    pub unknown4: [u16; MAX_PLAYER_ROLES],
    pub covered_by: [u16; MAX_PLAYER_ROLES],
    pub magic: [[u16; MAX_PLAYER_ROLES]; MAX_PLAYER_MAGICS],
    pub walk_frames: [u16; MAX_PLAYER_ROLES],
    pub cooperative_magic: [u16; MAX_PLAYER_ROLES],
    pub unknown5: [u16; MAX_PLAYER_ROLES],
    pub unknown6: [u16; MAX_PLAYER_ROLES],
    pub death_sound: [u16; MAX_PLAYER_ROLES],
    pub attack_sound: [u16; MAX_PLAYER_ROLES],
    pub weapon_sound: [u16; MAX_PLAYER_ROLES],
    pub critical_sound: [u16; MAX_PLAYER_ROLES],
    pub magic_sound: [u16; MAX_PLAYER_ROLES],
    pub cover_sound: [u16; MAX_PLAYER_ROLES],
    pub dying_sound: [u16; MAX_PLAYER_ROLES],
}

impl Default for PlayerRoles {
    fn default() -> PlayerRoles {
        let zeros = [0u16; MAX_PLAYER_ROLES];
        PlayerRoles {
            avatar: zeros,
            sprite_num_in_battle: zeros,
            sprite_num: zeros,
            name: zeros,
            attack_all: zeros,
            unknown1: zeros,
            level: zeros,
            max_hp: zeros,
            max_mp: zeros,
            hp: zeros,
            mp: zeros,
            equipment: [zeros; MAX_PLAYER_EQUIPMENTS],
            attack_strength: zeros,
            magic_strength: zeros,
            defense: zeros,
            dexterity: zeros,
            flee_rate: zeros,
            poison_resistance: zeros,
            elemental_resistance: [zeros; NUM_MAGIC_ELEMENTAL],
            unknown2: zeros,
            unknown3: zeros,
            unknown4: zeros,
            covered_by: zeros,
            magic: [zeros; MAX_PLAYER_MAGICS],
            walk_frames: zeros,
            cooperative_magic: zeros,
            unknown5: zeros,
            unknown6: zeros,
            death_sound: zeros,
            attack_sound: zeros,
            weapon_sound: zeros,
            critical_sound: zeros,
            magic_sound: zeros,
            cover_sound: zeros,
            dying_sound: zeros,
        }
    }
}

impl PlayerRoles {
    pub const BYTES: usize = 900;

    fn read_row(r: &mut WordReader) -> [u16; MAX_PLAYER_ROLES] {
        [r.u16(), r.u16(), r.u16(), r.u16(), r.u16(), r.u16()]
    }

    pub fn read(r: &mut WordReader) -> PlayerRoles {
        let mut p = PlayerRoles {
            avatar: Self::read_row(r),
            sprite_num_in_battle: Self::read_row(r),
            sprite_num: Self::read_row(r),
            name: Self::read_row(r),
            attack_all: Self::read_row(r),
            unknown1: Self::read_row(r),
            level: Self::read_row(r),
            max_hp: Self::read_row(r),
            max_mp: Self::read_row(r),
            hp: Self::read_row(r),
            mp: Self::read_row(r),
            ..PlayerRoles::default()
        };
        for row in p.equipment.iter_mut() {
            *row = Self::read_row(r);
        }
        p.attack_strength = Self::read_row(r);
        p.magic_strength = Self::read_row(r);
        p.defense = Self::read_row(r);
        p.dexterity = Self::read_row(r);
        p.flee_rate = Self::read_row(r);
        p.poison_resistance = Self::read_row(r);
        for row in p.elemental_resistance.iter_mut() {
            *row = Self::read_row(r);
        }
        p.unknown2 = Self::read_row(r);
        p.unknown3 = Self::read_row(r);
        p.unknown4 = Self::read_row(r);
        p.covered_by = Self::read_row(r);
        for row in p.magic.iter_mut() {
            *row = Self::read_row(r);
        }
        p.walk_frames = Self::read_row(r);
        p.cooperative_magic = Self::read_row(r);
        p.unknown5 = Self::read_row(r);
        p.unknown6 = Self::read_row(r);
        p.death_sound = Self::read_row(r);
        p.attack_sound = Self::read_row(r);
        p.weapon_sound = Self::read_row(r);
        p.critical_sound = Self::read_row(r);
        p.magic_sound = Self::read_row(r);
        p.cover_sound = Self::read_row(r);
        p.dying_sound = Self::read_row(r);
        p
    }

    fn write_row(w: &mut WordWriter, row: &[u16; MAX_PLAYER_ROLES]) {
        for &v in row {
            w.u16(v);
        }
    }

    pub fn write(&self, w: &mut WordWriter) {
        Self::write_row(w, &self.avatar);
        Self::write_row(w, &self.sprite_num_in_battle);
        Self::write_row(w, &self.sprite_num);
        Self::write_row(w, &self.name);
        Self::write_row(w, &self.attack_all);
        Self::write_row(w, &self.unknown1);
        Self::write_row(w, &self.level);
        Self::write_row(w, &self.max_hp);
        Self::write_row(w, &self.max_mp);
        Self::write_row(w, &self.hp);
        Self::write_row(w, &self.mp);
        for row in self.equipment.iter() {
            Self::write_row(w, row);
        }
        Self::write_row(w, &self.attack_strength);
        Self::write_row(w, &self.magic_strength);
        Self::write_row(w, &self.defense);
        Self::write_row(w, &self.dexterity);
        Self::write_row(w, &self.flee_rate);
        Self::write_row(w, &self.poison_resistance);
        for row in self.elemental_resistance.iter() {
            Self::write_row(w, row);
        }
        Self::write_row(w, &self.unknown2);
        Self::write_row(w, &self.unknown3);
        Self::write_row(w, &self.unknown4);
        Self::write_row(w, &self.covered_by);
        for row in self.magic.iter() {
            Self::write_row(w, row);
        }
        Self::write_row(w, &self.walk_frames);
        Self::write_row(w, &self.cooperative_magic);
        Self::write_row(w, &self.unknown5);
        Self::write_row(w, &self.unknown6);
        Self::write_row(w, &self.death_sound);
        Self::write_row(w, &self.attack_sound);
        Self::write_row(w, &self.weapon_sound);
        Self::write_row(w, &self.critical_sound);
        Self::write_row(w, &self.magic_sound);
        Self::write_row(w, &self.cover_sound);
        Self::write_row(w, &self.dying_sound);
    }

    /// Zero player `role`'s column in every stat row — the Rust equivalent
    /// of the PAL_RemoveEquipmentEffect HACKHACK that treats PLAYERROLES as
    /// a flat WORD array of rows.
    pub fn clear_role_column(&mut self, role: usize) {
        self.avatar[role] = 0;
        self.sprite_num_in_battle[role] = 0;
        self.sprite_num[role] = 0;
        self.name[role] = 0;
        self.attack_all[role] = 0;
        self.unknown1[role] = 0;
        self.level[role] = 0;
        self.max_hp[role] = 0;
        self.max_mp[role] = 0;
        self.hp[role] = 0;
        self.mp[role] = 0;
        for row in self.equipment.iter_mut() {
            row[role] = 0;
        }
        self.attack_strength[role] = 0;
        self.magic_strength[role] = 0;
        self.defense[role] = 0;
        self.dexterity[role] = 0;
        self.flee_rate[role] = 0;
        self.poison_resistance[role] = 0;
        for row in self.elemental_resistance.iter_mut() {
            row[role] = 0;
        }
        self.unknown2[role] = 0;
        self.unknown3[role] = 0;
        self.unknown4[role] = 0;
        self.covered_by[role] = 0;
        for row in self.magic.iter_mut() {
            row[role] = 0;
        }
        self.walk_frames[role] = 0;
        self.cooperative_magic[role] = 0;
        self.unknown5[role] = 0;
        self.unknown6[role] = 0;
        self.death_sound[role] = 0;
        self.attack_sound[role] = 0;
        self.weapon_sound[role] = 0;
        self.critical_sound[role] = 0;
        self.magic_sound[role] = 0;
        self.cover_sound[role] = 0;
        self.dying_sound[role] = 0;
    }
}

/// MAGIC — 16 words / 32 bytes. rgSpecific is a union; keep the raw word and
/// expose both interpretations.
#[derive(Clone, Copy, Default, Debug)]
pub struct MagicData {
    pub effect: u16,
    pub magic_type: u16,
    pub x_offset: u16,
    pub y_offset: u16,
    pub specific: u16, // wSummonEffect / sLayerOffset union
    pub speed: i16,
    pub keep_effect: u16,
    pub fire_delay: u16,
    pub effect_times: u16,
    pub shake: u16,
    pub wave: u16,
    pub unknown: u16,
    pub cost_mp: u16,
    pub base_damage: u16,
    pub elemental: u16,
    pub sound: i16,
}

impl MagicData {
    pub const BYTES: usize = 32;

    pub fn summon_effect(&self) -> u16 {
        self.specific
    }

    pub fn layer_offset(&self) -> i16 {
        self.specific as i16
    }

    fn read(r: &mut WordReader) -> MagicData {
        MagicData {
            effect: r.u16(),
            magic_type: r.u16(),
            x_offset: r.u16(),
            y_offset: r.u16(),
            specific: r.u16(),
            speed: r.i16(),
            keep_effect: r.u16(),
            fire_delay: r.u16(),
            effect_times: r.u16(),
            shake: r.u16(),
            wave: r.u16(),
            unknown: r.u16(),
            cost_mp: r.u16(),
            base_damage: r.u16(),
            elemental: r.u16(),
            sound: r.i16(),
        }
    }
}

/// BATTLEFIELD — 6 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct BattleField {
    pub screen_wave: u16,
    pub magic_effect: [i16; NUM_MAGIC_ELEMENTAL],
}

/// LEVELUPMAGIC for all playable roles — 10 words: (level, magic) pairs.
#[derive(Clone, Copy, Default, Debug)]
pub struct LevelUpMagicAll {
    pub m: [(u16, u16); MAX_PLAYABLE_PLAYER_ROLES],
}

/// PARTY — 5 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Party {
    pub player_role: u16,
    pub x: i16,
    pub y: i16,
    pub frame: u16,
    pub image_offset: u16,
}

/// TRAIL — 3 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Trail {
    pub x: u16,
    pub y: u16,
    pub direction: u16,
}

/// EXPERIENCE — 4 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct Experience {
    pub exp: u16,
    pub reserved: u16,
    pub level: u16,
    pub count: u16,
}

/// ALLEXPERIENCE — 8 kinds x 6 roles x 4 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct AllExperience {
    pub primary_exp: [Experience; MAX_PLAYER_ROLES],
    pub health_exp: [Experience; MAX_PLAYER_ROLES],
    pub magic_exp: [Experience; MAX_PLAYER_ROLES],
    pub attack_exp: [Experience; MAX_PLAYER_ROLES],
    pub magic_power_exp: [Experience; MAX_PLAYER_ROLES],
    pub defense_exp: [Experience; MAX_PLAYER_ROLES],
    pub dexterity_exp: [Experience; MAX_PLAYER_ROLES],
    pub flee_exp: [Experience; MAX_PLAYER_ROLES],
}

impl AllExperience {
    fn read_group(r: &mut WordReader) -> [Experience; MAX_PLAYER_ROLES] {
        let mut g = [Experience::default(); MAX_PLAYER_ROLES];
        for e in g.iter_mut() {
            *e = Experience {
                exp: r.u16(),
                reserved: r.u16(),
                level: r.u16(),
                count: r.u16(),
            };
        }
        g
    }

    fn read(r: &mut WordReader) -> AllExperience {
        AllExperience {
            primary_exp: Self::read_group(r),
            health_exp: Self::read_group(r),
            magic_exp: Self::read_group(r),
            attack_exp: Self::read_group(r),
            magic_power_exp: Self::read_group(r),
            defense_exp: Self::read_group(r),
            dexterity_exp: Self::read_group(r),
            flee_exp: Self::read_group(r),
        }
    }

    fn write_group(w: &mut WordWriter, g: &[Experience; MAX_PLAYER_ROLES]) {
        for e in g.iter() {
            w.u16(e.exp);
            w.u16(e.reserved);
            w.u16(e.level);
            w.u16(e.count);
        }
    }

    fn write(&self, w: &mut WordWriter) {
        Self::write_group(w, &self.primary_exp);
        Self::write_group(w, &self.health_exp);
        Self::write_group(w, &self.magic_exp);
        Self::write_group(w, &self.attack_exp);
        Self::write_group(w, &self.magic_power_exp);
        Self::write_group(w, &self.defense_exp);
        Self::write_group(w, &self.dexterity_exp);
        Self::write_group(w, &self.flee_exp);
    }
}

/// POISONSTATUS — 2 words.
#[derive(Clone, Copy, Default, Debug)]
pub struct PoisonStatus {
    pub poison_id: u16,
    pub poison_script: u16,
}

// ===========================================================================
// GAMEDATA and GLOBALS.
// ===========================================================================

/// Static game data loaded from the data files (GAMEDATA).
pub struct GameData {
    pub event_objects: Vec<EventObject>,
    pub scenes: Vec<Scene>,       // MAX_SCENES entries
    pub objects: Vec<GameObject>, // MAX_OBJECTS entries
    pub script_entries: Vec<ScriptEntry>,
    pub stores: Vec<Store>,
    pub enemies: Vec<Enemy>,
    pub enemy_teams: Vec<EnemyTeam>,
    pub player_roles: PlayerRoles,
    pub magics: Vec<MagicData>,
    pub battle_fields: Vec<BattleField>,
    pub level_up_magics: Vec<LevelUpMagicAll>,
    pub enemy_pos: [[(u16, u16); MAX_ENEMIES_IN_TEAM]; MAX_ENEMIES_IN_TEAM],
    pub level_up_exp: [u16; MAX_LEVELS + 1],
    pub battle_effect_index: [[u16; 2]; 10],
}

/// The archives the engine keeps open (FILES).
pub struct GameFiles {
    pub fbp: Mkf,
    pub mgo: Mkf,
    pub ball: Mkf,
    pub data: Mkf,
    pub f: Mkf,
    pub fire: Mkf,
    pub rgm: Mkf,
    pub sss: Mkf,
}

/// GLOBALVARS.
pub struct Globals {
    pub data_dir: DataDir,
    pub save_dir: PathBuf,
    pub files: GameFiles,
    pub game: GameData,

    pub cur_main_menu_item: i32,
    pub cur_system_menu_item: i32,
    pub cur_inv_menu_item: i32,
    /// Remembered caster cursor for PAL_InGameMagicMenu (C's `static WORD w`).
    pub cur_magic_menu_player: u16,
    pub cur_playing_rng: i32,
    pub current_save_slot: u8,
    pub in_main_game: bool,
    pub entering_scene: bool,
    pub need_to_fade_in: bool,
    pub in_battle: bool,
    pub auto_battle: bool,
    pub last_unequipped_item: u16,

    /// Equipment effects — one PLAYERROLES per equipment slot + 1.
    pub equipment_effect: [PlayerRoles; MAX_PLAYER_EQUIPMENTS + 1],
    pub player_status: [[u16; STATUS_ALL]; MAX_PLAYER_ROLES],

    pub viewport: (i32, i32),
    pub partyoffset: (i32, i32),
    pub layer: u16,
    pub max_party_member_index: u16,
    pub party: [Party; MAX_PLAYABLE_PLAYER_ROLES],
    pub trail: [Trail; MAX_PLAYABLE_PLAYER_ROLES],
    pub party_direction: u16,
    pub num_scene: u16,
    pub num_palette: u16,
    pub night_palette: bool,
    pub num_music: u16,
    pub num_battle_music: u16,
    pub num_battle_field: u16,
    pub collect_value: u16,
    pub screen_wave: u16,
    pub wave_progression: i16,
    pub chase_range: u16,
    pub chasespeed_change_cycles: u16,
    pub follower_num: u16,

    pub cash: u32,

    pub exp: AllExperience,
    pub poison_status: [[PoisonStatus; MAX_PLAYABLE_PLAYER_ROLES]; MAX_POISONS],
    pub inventory: [Inventory; MAX_INVENTORY],
    pub frame_num: u32,

    /// Load flags for the resource layer (res.c bLoadFlags).
    pub load_flags: u8,
}

impl Globals {
    /// PAL_InitGlobals: open all archives and load the static game data.
    pub fn init(data_dir: DataDir) -> io::Result<Globals> {
        let files = GameFiles {
            fbp: data_dir.mkf("fbp.mkf")?,
            mgo: data_dir.mkf("mgo.mkf")?,
            ball: data_dir.mkf("ball.mkf")?,
            data: data_dir.mkf("data.mkf")?,
            f: data_dir.mkf("f.mkf")?,
            fire: data_dir.mkf("fire.mkf")?,
            rgm: data_dir.mkf("rgm.mkf")?,
            sss: data_dir.mkf("sss.mkf")?,
        };
        let game = GameData::load(&files)?;
        let save_dir = data_dir.root().to_path_buf();
        Ok(Globals {
            data_dir,
            save_dir,
            files,
            game,
            cur_main_menu_item: 0,
            cur_system_menu_item: 0,
            cur_inv_menu_item: 0,
            cur_magic_menu_player: 0,
            cur_playing_rng: 0,
            current_save_slot: 1,
            in_main_game: false,
            entering_scene: false,
            need_to_fade_in: false,
            in_battle: false,
            auto_battle: false,
            last_unequipped_item: 0,
            equipment_effect: [PlayerRoles::default(); MAX_PLAYER_EQUIPMENTS + 1],
            player_status: [[0; STATUS_ALL]; MAX_PLAYER_ROLES],
            viewport: (0, 0),
            partyoffset: (0, 0),
            layer: 0,
            max_party_member_index: 0,
            party: [Party::default(); MAX_PLAYABLE_PLAYER_ROLES],
            trail: [Trail::default(); MAX_PLAYABLE_PLAYER_ROLES],
            party_direction: 0,
            num_scene: 1,
            num_palette: 0,
            night_palette: false,
            num_music: 0,
            num_battle_music: 0,
            num_battle_field: 0,
            collect_value: 0,
            screen_wave: 0,
            wave_progression: 0,
            chase_range: 1,
            chasespeed_change_cycles: 0,
            follower_num: 0,
            cash: 0,
            exp: AllExperience::default(),
            poison_status: [[PoisonStatus::default(); MAX_PLAYABLE_PLAYER_ROLES]; MAX_POISONS],
            inventory: [Inventory::default(); MAX_INVENTORY],
            frame_num: 0,
            load_flags: 0,
        })
    }

    /// PAL_InitGameData: set up game state from a save slot (0 = new game).
    /// NOTE: the C code calls PAL_UpdateEquipments() at the end; in this
    /// port that lives in the script layer and must be called by the caller.
    pub fn init_game_data(&mut self, save_slot: i32) -> io::Result<()> {
        self.current_save_slot = save_slot as u8;
        if save_slot == 0 || self.load_game(save_slot).is_err() {
            self.load_default_game()?;
        }
        self.cur_inv_menu_item = 0;
        self.in_battle = false;
        self.player_status = [[0; STATUS_ALL]; MAX_PLAYER_ROLES];
        Ok(())
    }

    /// PAL_LoadDefaultGame.
    pub fn load_default_game(&mut self) -> io::Result<()> {
        self.game.reload_default(&self.files)?;

        self.cash = 0;
        self.num_music = 0;
        self.num_palette = 0;
        self.num_scene = 1;
        self.collect_value = 0;
        self.night_palette = false;
        self.max_party_member_index = 0;
        self.viewport = (0, 0);
        self.layer = 0;
        self.follower_num = 0;
        self.chase_range = 1;

        self.inventory = [Inventory::default(); MAX_INVENTORY];
        self.poison_status = [[PoisonStatus::default(); MAX_PLAYABLE_PLAYER_ROLES]; MAX_POISONS];
        self.party = [Party::default(); MAX_PLAYABLE_PLAYER_ROLES];
        self.trail = [Trail::default(); MAX_PLAYABLE_PLAYER_ROLES];
        self.exp = AllExperience::default();

        for i in 0..MAX_PLAYER_ROLES {
            let lvl = self.game.player_roles.level[i];
            self.exp.primary_exp[i].level = lvl;
            self.exp.health_exp[i].level = lvl;
            self.exp.magic_exp[i].level = lvl;
            self.exp.attack_exp[i].level = lvl;
            self.exp.magic_power_exp[i].level = lvl;
            self.exp.defense_exp[i].level = lvl;
            self.exp.dexterity_exp[i].level = lvl;
            self.exp.flee_exp[i].level = lvl;
        }

        self.entering_scene = true;
        Ok(())
    }

    fn save_file_path(&self, slot: i32) -> PathBuf {
        self.save_dir.join(format!("{slot}.rpg"))
    }

    /// PAL_LoadGame (DOS format).
    pub fn load_game(&mut self, slot: i32) -> io::Result<()> {
        // On the web the save lives in the PAL_FILES map (seeded from
        // localStorage by web/main.js) instead of the filesystem.
        #[cfg(target_arch = "wasm32")]
        let buf = self.data_dir.read_file(&format!("{slot}.rpg"))?;
        #[cfg(not(target_arch = "wasm32"))]
        let buf = std::fs::read(self.save_file_path(slot))?;
        self.load_game_from_bytes(&buf)
    }

    pub fn load_game_from_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        // Minimum size: everything except the event objects
        // (sizeof(SAVEDGAME_DOS) - MAX_EVENT_OBJECTS * 32 = 12864).
        const MIN_SIZE: usize = 12864;
        if buf.len() < MIN_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "saved game too short",
            ));
        }
        let mut r = WordReader::new(buf);
        let _saved_times = r.u16();
        let viewport_x = r.u16();
        let viewport_y = r.u16();
        let party_member = r.u16();
        let num_scene = r.u16();
        let palette_offset = r.u16();
        let party_direction = r.u16();
        let num_music = r.u16();
        let num_battle_music = r.u16();
        let num_battle_field = r.u16();
        let screen_wave = r.u16();
        let _battle_speed = r.u16();
        let collect_value = r.u16();
        let layer = r.u16();
        let chase_range = r.u16();
        let chasespeed_change_cycles = r.u16();
        let follower_num = r.u16();
        let _reserved = (r.u16(), r.u16(), r.u16());
        let cash = r.u32();

        self.viewport = (viewport_x as i32, viewport_y as i32);
        self.max_party_member_index = party_member;
        self.num_scene = num_scene;
        self.night_palette = palette_offset != 0;
        self.party_direction = party_direction;
        self.num_music = num_music;
        self.num_battle_music = num_battle_music;
        self.num_battle_field = num_battle_field;
        self.screen_wave = screen_wave;
        self.wave_progression = 0;
        self.collect_value = collect_value;
        self.layer = layer;
        self.chase_range = chase_range;
        self.chasespeed_change_cycles = chasespeed_change_cycles;
        self.follower_num = follower_num;
        self.cash = cash;

        for p in self.party.iter_mut() {
            *p = Party {
                player_role: r.u16(),
                x: r.i16(),
                y: r.i16(),
                frame: r.u16(),
                image_offset: r.u16(),
            };
        }
        for t in self.trail.iter_mut() {
            *t = Trail {
                x: r.u16(),
                y: r.u16(),
                direction: r.u16(),
            };
        }
        self.exp = AllExperience::read(&mut r);
        self.game.player_roles = PlayerRoles::read(&mut r);
        // Poison status is stored in the save but reset on load (the C code
        // reads it into the struct, then clears gpGlobals->rgPoisonStatus).
        for _ in 0..MAX_POISONS * MAX_PLAYABLE_PLAYER_ROLES {
            r.u16();
            r.u16();
        }
        self.poison_status = [[PoisonStatus::default(); MAX_PLAYABLE_PLAYER_ROLES]; MAX_POISONS];
        for inv in self.inventory.iter_mut() {
            *inv = Inventory {
                item: r.u16(),
                amount: r.u16(),
                amount_in_use: r.u16(),
            };
        }
        for s in self.game.scenes.iter_mut() {
            *s = Scene::read(&mut r);
        }
        for obj in self.game.objects.iter_mut() {
            let dos = [r.u16(), r.u16(), r.u16(), r.u16(), r.u16(), r.u16()];
            *obj = GameObject::from_dos(dos);
        }
        for eo in self.game.event_objects.iter_mut() {
            *eo = EventObject::read(&mut r);
        }

        self.entering_scene = false;
        self.compress_inventory();
        Ok(())
    }

    /// PAL_SaveGame (DOS format).
    pub fn save_game(&self, slot: i32, saved_times: u16) -> io::Result<()> {
        let buf = self.save_game_to_bytes(saved_times);
        // On the web: update the in-worker PAL_FILES map (so loads in this
        // session see it) and post it to the main thread for localStorage.
        #[cfg(target_arch = "wasm32")]
        {
            crate::web::store_save(slot, &buf);
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::fs::write(self.save_file_path(slot), buf)
    }

    pub fn save_game_to_bytes(&self, saved_times: u16) -> Vec<u8> {
        let mut w = WordWriter::new();
        w.u16(saved_times);
        w.u16(self.viewport.0 as u16);
        w.u16(self.viewport.1 as u16);
        w.u16(self.max_party_member_index);
        w.u16(self.num_scene);
        w.u16(if self.night_palette { 0x180 } else { 0 });
        w.u16(self.party_direction);
        w.u16(self.num_music);
        w.u16(self.num_battle_music);
        w.u16(self.num_battle_field);
        w.u16(self.screen_wave);
        w.u16(2); // wBattleSpeed (classic: always 2)
        w.u16(self.collect_value);
        w.u16(self.layer);
        w.u16(self.chase_range);
        w.u16(self.chasespeed_change_cycles);
        w.u16(self.follower_num);
        w.u16(0);
        w.u16(0);
        w.u16(0); // rgwReserved2
        w.u32(self.cash);
        for p in self.party.iter() {
            w.u16(p.player_role);
            w.i16(p.x);
            w.i16(p.y);
            w.u16(p.frame);
            w.u16(p.image_offset);
        }
        for t in self.trail.iter() {
            w.u16(t.x);
            w.u16(t.y);
            w.u16(t.direction);
        }
        self.exp.write(&mut w);
        self.game.player_roles.write(&mut w);
        for row in self.poison_status.iter() {
            for ps in row.iter() {
                w.u16(ps.poison_id);
                w.u16(ps.poison_script);
            }
        }
        for inv in self.inventory.iter() {
            w.u16(inv.item);
            w.u16(inv.amount);
            w.u16(inv.amount_in_use);
        }
        for s in self.game.scenes.iter() {
            s.write(&mut w);
        }
        for obj in self.game.objects.iter() {
            for v in obj.to_dos() {
                w.u16(v);
            }
        }
        for eo in self.game.event_objects.iter() {
            eo.write(&mut w);
        }
        w.buf
    }

    // =======================================================================
    // Inventory (global.c).
    // =======================================================================

    /// PAL_CountItem: count item in inventory and equipment.
    pub fn count_item(&self, object_id: u16) -> i32 {
        if object_id == 0 {
            return 0;
        }
        let mut count = 0;
        for inv in self.inventory.iter() {
            if inv.item == object_id {
                count = inv.amount as i32;
                break;
            }
            if inv.item == 0 {
                break;
            }
        }
        for i in 0..=self.max_party_member_index as usize {
            let w = self.party[i].player_role as usize;
            for j in 0..MAX_PLAYER_EQUIPMENTS {
                if self.game.player_roles.equipment[j][w] == object_id {
                    count += 1;
                }
            }
        }
        count
    }

    /// PAL_GetItemIndexToInventory.
    pub fn get_item_index_to_inventory(&self, object_id: u16) -> (bool, usize) {
        let mut index = 0;
        while index < MAX_INVENTORY {
            if self.inventory[index].item == object_id {
                return (true, index);
            }
            if self.inventory[index].item == 0 {
                break;
            }
            index += 1;
        }
        (false, index)
    }

    /// PAL_AddItemToInventory. Returns 1 on success, 0 on failure, negative
    /// shortage amount when removal ran the item out.
    pub fn add_item_to_inventory(&mut self, object_id: u16, num: i32) -> i32 {
        if object_id == 0 {
            return 0;
        }
        let mut num = if num == 0 { 1 } else { num };
        let (found, index) = self.get_item_index_to_inventory(object_id);

        if num > 0 {
            if index >= MAX_INVENTORY {
                return 0;
            }
            if found {
                self.inventory[index].amount =
                    (self.inventory[index].amount as i32 + num).min(99) as u16;
            } else {
                self.inventory[index].item = object_id;
                if num > 99 {
                    num = 99;
                }
                self.inventory[index].amount = num as u16;
            }
            1
        } else if found {
            num = -num;
            if (self.inventory[index].amount as i32) < num {
                let shortage = num - self.inventory[index].amount as i32;
                self.inventory[index].amount = 0;
                return -shortage;
            }
            self.inventory[index].amount -= num as u16;
            if self.inventory[index].amount == 0
                && index as i32 == self.cur_inv_menu_item
                && index + 1 < MAX_INVENTORY
                && self.inventory[index + 1].amount == 0
            {
                self.cur_inv_menu_item -= 1;
            }
            1
        } else {
            0
        }
    }

    /// PAL_GetItemAmount.
    pub fn get_item_amount(&self, item: u16) -> i32 {
        for inv in self.inventory.iter() {
            if inv.item == 0 {
                break;
            }
            if inv.item == item {
                return inv.amount as i32;
            }
        }
        0
    }

    /// PAL_CompressInventory.
    pub fn compress_inventory(&mut self) {
        let mut j = 0;
        for i in 0..MAX_INVENTORY {
            if self.inventory[i].amount > 0 {
                self.inventory[j] = self.inventory[i];
                j += 1;
            }
        }
        for inv in self.inventory.iter_mut().skip(j) {
            *inv = Inventory::default();
        }
    }

    // =======================================================================
    // HP/MP, status, magic, poison (pure parts of global.c).
    // =======================================================================

    /// PAL_IncreaseHPMP.
    pub fn increase_hp_mp(&mut self, role: usize, hp: i16, mp: i16) -> bool {
        let pr = &mut self.game.player_roles;
        let orig_hp = pr.hp[role];
        let orig_mp = pr.mp[role];
        if pr.hp[role] == 0 {
            return false;
        }
        let new_hp = pr.hp[role].wrapping_add(hp as u16);
        pr.hp[role] = if (new_hp as i16) < 0 {
            0
        } else {
            new_hp.min(pr.max_hp[role])
        };
        let new_mp = pr.mp[role].wrapping_add(mp as u16);
        pr.mp[role] = if (new_mp as i16) < 0 {
            0
        } else {
            new_mp.min(pr.max_mp[role])
        };
        orig_hp != pr.hp[role] || orig_mp != pr.mp[role]
    }

    /// Find the party index for a player role, if that role is in the party.
    pub fn party_index_of_role(&self, role: u16) -> Option<usize> {
        (0..=self.max_party_member_index as usize).find(|&i| self.party[i].player_role == role)
    }

    /// PAL_RemoveEquipmentEffect.
    pub fn remove_equipment_effect(&mut self, role: u16, equip_part: usize) {
        self.equipment_effect[equip_part].clear_role_column(role as usize);

        if equip_part == BODYPART_HAND {
            self.player_status[role as usize][STATUS_DUALATTACK] = 0;
        } else if equip_part == BODYPART_WEAR {
            if let Some(idx) = self.party_index_of_role(role) {
                let mut j = 0;
                for i in 0..MAX_POISONS {
                    let w = self.poison_status[i][idx].poison_id;
                    if w == 0 {
                        break;
                    }
                    if self.game.objects[w as usize].poison_level() < 99 {
                        self.poison_status[j][idx] = self.poison_status[i][idx];
                        j += 1;
                    }
                }
                while j < MAX_POISONS {
                    self.poison_status[j][idx] = PoisonStatus::default();
                    j += 1;
                }
            }
        }
    }

    /// PAL_CurePoisonByKind.
    pub fn cure_poison_by_kind(&mut self, role: u16, poison_id: u16) {
        if let Some(idx) = self.party_index_of_role(role) {
            for i in 0..MAX_POISONS {
                if self.poison_status[i][idx].poison_id == poison_id {
                    self.poison_status[i][idx] = PoisonStatus::default();
                }
            }
        }
    }

    /// PAL_CurePoisonByLevel.
    pub fn cure_poison_by_level(&mut self, role: u16, max_level: u16) {
        if let Some(idx) = self.party_index_of_role(role) {
            for i in 0..MAX_POISONS {
                let w = self.poison_status[i][idx].poison_id;
                if self.game.objects[w as usize].poison_level() <= max_level {
                    self.poison_status[i][idx] = PoisonStatus::default();
                }
            }
        }
    }

    /// PAL_IsPlayerPoisonedByLevel.
    pub fn is_player_poisoned_by_level(&self, role: u16, min_level: u16) -> bool {
        if let Some(idx) = self.party_index_of_role(role) {
            for i in 0..MAX_POISONS {
                let w = self.poison_status[i][idx].poison_id;
                if w == 0 {
                    continue;
                }
                let level = self.game.objects[w as usize].poison_level();
                if level >= 99 {
                    continue;
                }
                if level >= min_level {
                    return true;
                }
            }
        }
        false
    }

    /// PAL_IsPlayerPoisonedByKind.
    pub fn is_player_poisoned_by_kind(&self, role: u16, poison_id: u16) -> bool {
        if let Some(idx) = self.party_index_of_role(role) {
            for i in 0..MAX_POISONS {
                if self.poison_status[i][idx].poison_id == poison_id {
                    return true;
                }
            }
        }
        false
    }

    /// PAL_GetPlayerAttackStrength.
    pub fn player_attack_strength(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.attack_strength[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.attack_strength[role]);
        }
        w
    }

    /// PAL_GetPlayerMagicStrength.
    pub fn player_magic_strength(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.magic_strength[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.magic_strength[role]);
        }
        w
    }

    /// PAL_GetPlayerDefense.
    pub fn player_defense(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.defense[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.defense[role]);
        }
        w
    }

    /// PAL_GetPlayerDexterity (classic: all equipment slots).
    pub fn player_dexterity(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.dexterity[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.dexterity[role]);
        }
        w
    }

    /// PAL_GetPlayerFleeRate.
    pub fn player_flee_rate(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.flee_rate[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.flee_rate[role]);
        }
        w
    }

    /// PAL_GetPlayerPoisonResistance (capped at 100).
    pub fn player_poison_resistance(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.poison_resistance[role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.poison_resistance[role]);
        }
        w.min(100)
    }

    /// PAL_GetPlayerElementalResistance (capped at 100).
    pub fn player_elemental_resistance(&self, role: usize, attrib: usize) -> u16 {
        let mut w = self.game.player_roles.elemental_resistance[attrib][role];
        for eff in self.equipment_effect.iter() {
            w = w.wrapping_add(eff.elemental_resistance[attrib][role]);
        }
        w.min(100)
    }

    /// PAL_GetPlayerBattleSprite.
    pub fn player_battle_sprite(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.sprite_num_in_battle[role];
        for eff in self.equipment_effect.iter() {
            if eff.sprite_num_in_battle[role] != 0 {
                w = eff.sprite_num_in_battle[role];
            }
        }
        w
    }

    /// PAL_GetPlayerCooperativeMagic.
    pub fn player_cooperative_magic(&self, role: usize) -> u16 {
        let mut w = self.game.player_roles.cooperative_magic[role];
        for eff in self.equipment_effect.iter() {
            if eff.cooperative_magic[role] != 0 {
                w = eff.cooperative_magic[role];
            }
        }
        w
    }

    /// PAL_PlayerCanAttackAll.
    pub fn player_can_attack_all(&self, role: usize) -> bool {
        self.equipment_effect
            .iter()
            .any(|eff| eff.attack_all[role] != 0)
    }

    /// PAL_AddMagic.
    pub fn add_magic(&mut self, role: usize, magic: u16) -> bool {
        for i in 0..MAX_PLAYER_MAGICS {
            if self.game.player_roles.magic[i][role] == magic {
                return false;
            }
        }
        for i in 0..MAX_PLAYER_MAGICS {
            if self.game.player_roles.magic[i][role] == 0 {
                self.game.player_roles.magic[i][role] = magic;
                return true;
            }
        }
        false
    }

    /// PAL_RemoveMagic.
    pub fn remove_magic(&mut self, role: usize, magic: u16) {
        for i in 0..MAX_PLAYER_MAGICS {
            if self.game.player_roles.magic[i][role] == magic {
                self.game.player_roles.magic[i][role] = 0;
                break;
            }
        }
    }

    /// PAL_SetPlayerStatus (classic variant).
    pub fn set_player_status(&mut self, role: usize, status_id: usize, num_round: u16) -> bool {
        match status_id {
            STATUS_CONFUSED | STATUS_SLEEP | STATUS_SILENCE | STATUS_PARALYZED => {
                if self.player_status[role][status_id] == 0 {
                    self.player_status[role][status_id] = num_round;
                }
                true
            }
            STATUS_PUPPET => {
                if self.game.player_roles.hp[role] == 0 {
                    if self.player_status[role][status_id] < num_round {
                        self.player_status[role][status_id] = num_round;
                    }
                    true
                } else {
                    false
                }
            }
            STATUS_BRAVERY | STATUS_PROTECT | STATUS_DUALATTACK | STATUS_HASTE => {
                if self.game.player_roles.hp[role] != 0
                    && self.player_status[role][status_id] < num_round
                {
                    self.player_status[role][status_id] = num_round;
                }
                true
            }
            _ => {
                debug_assert!(false, "bad status id {status_id}");
                true
            }
        }
    }

    /// PAL_RemovePlayerStatus.
    pub fn remove_player_status(&mut self, role: usize, status_id: usize) {
        // Don't remove effects of equipments.
        if self.player_status[role][status_id] <= 999 {
            self.player_status[role][status_id] = 0;
        }
    }

    /// PAL_ClearAllPlayerStatus.
    pub fn clear_all_player_status(&mut self) {
        for row in self.player_status.iter_mut() {
            for st in row.iter_mut() {
                if *st <= 999 {
                    *st = 0;
                }
            }
        }
    }

    /// PAL_PlayerLevelUp.
    pub fn player_level_up(&mut self, role: usize, num_level: u16) {
        let pr = &mut self.game.player_roles;
        pr.level[role] = (pr.level[role] + num_level).min(MAX_LEVELS as u16);

        for _ in 0..num_level {
            pr.max_hp[role] = pr.max_hp[role].wrapping_add(10 + random_long(0, 7) as u16);
            pr.max_mp[role] = pr.max_mp[role].wrapping_add(8 + random_long(0, 5) as u16);
            pr.attack_strength[role] =
                pr.attack_strength[role].wrapping_add(4 + random_long(0, 1) as u16);
            pr.magic_strength[role] =
                pr.magic_strength[role].wrapping_add(4 + random_long(0, 1) as u16);
            pr.defense[role] = pr.defense[role].wrapping_add(2 + random_long(0, 1) as u16);
            pr.dexterity[role] = pr.dexterity[role].wrapping_add(2 + random_long(0, 1) as u16);
            pr.flee_rate[role] = pr.flee_rate[role].wrapping_add(2);
        }

        pr.max_hp[role] = pr.max_hp[role].min(999);
        pr.max_mp[role] = pr.max_mp[role].min(999);
        pr.attack_strength[role] = pr.attack_strength[role].min(999);
        pr.magic_strength[role] = pr.magic_strength[role].min(999);
        pr.defense[role] = pr.defense[role].min(999);
        pr.dexterity[role] = pr.dexterity[role].min(999);
        pr.flee_rate[role] = pr.flee_rate[role].min(999);

        self.exp.primary_exp[role].exp = 0;
        self.exp.primary_exp[role].level = self.game.player_roles.level[role];
    }

    /// PAL_ReloadInNextTick.
    pub fn reload_in_next_tick(&mut self, save_slot: i32) {
        self.current_save_slot = save_slot as u8;
        self.load_flags |= LOAD_GLOBAL_DATA | LOAD_SCENE | LOAD_PLAYER_SPRITE;
        self.entering_scene = true;
        self.need_to_fade_in = true;
        self.frame_num = 0;
    }
}

impl GameData {
    /// PAL_InitGlobalGameData + PAL_ReadGlobalGameData; the per-game parts
    /// (event objects / scenes / objects / player roles) are loaded via
    /// `reload_default` so the struct starts fully populated.
    pub fn load(files: &GameFiles) -> io::Result<GameData> {
        let sss = &files.sss;
        let data = &files.data;

        // Counts determined by chunk sizes, exactly like PAL_DOALLOCATE.
        let n_event_object = sss.chunk_size(0) / EventObject::BYTES;
        let n_script_entry = sss.chunk_size(4) / 8;
        let n_store = data.chunk_size(0) / (MAX_STORE_ITEM * 2);
        let n_enemy = data.chunk_size(1) / Enemy::BYTES;
        let n_enemy_team = data.chunk_size(2) / (MAX_ENEMIES_IN_TEAM * 2);
        let n_magic = data.chunk_size(4) / MagicData::BYTES;
        let n_battle_field = data.chunk_size(5) / 12;
        let n_level_up_magic = data.chunk_size(6) / (MAX_PLAYABLE_PLAYER_ROLES * 4);

        let mut g = GameData {
            event_objects: vec![EventObject::default(); n_event_object],
            scenes: vec![Scene::default(); MAX_SCENES],
            objects: vec![GameObject::default(); MAX_OBJECTS],
            script_entries: vec![ScriptEntry::default(); n_script_entry],
            stores: vec![Store::default(); n_store],
            enemies: vec![Enemy::default(); n_enemy],
            enemy_teams: vec![EnemyTeam::default(); n_enemy_team],
            player_roles: PlayerRoles::default(),
            magics: vec![MagicData::default(); n_magic],
            battle_fields: vec![BattleField::default(); n_battle_field],
            level_up_magics: vec![LevelUpMagicAll::default(); n_level_up_magic],
            enemy_pos: [[(0, 0); MAX_ENEMIES_IN_TEAM]; MAX_ENEMIES_IN_TEAM],
            level_up_exp: [0; MAX_LEVELS + 1],
            battle_effect_index: [[0; 2]; 10],
        };
        g.read_global_game_data(files)?;
        g.reload_default(files)?;
        Ok(g)
    }

    /// PAL_ReadGlobalGameData.
    fn read_global_game_data(&mut self, files: &GameFiles) -> io::Result<()> {
        // Script entries: sss.mkf #4.
        {
            let mut r = WordReader::new(files.sss.chunk(4)?);
            for e in self.script_entries.iter_mut() {
                *e = ScriptEntry {
                    operation: r.u16(),
                    operand: [r.u16(), r.u16(), r.u16()],
                };
            }
        }
        // Stores: data.mkf #0.
        {
            let mut r = WordReader::new(files.data.chunk(0)?);
            for s in self.stores.iter_mut() {
                for item in s.items.iter_mut() {
                    *item = r.u16();
                }
            }
        }
        // Enemies: data.mkf #1.
        {
            let mut r = WordReader::new(files.data.chunk(1)?);
            for e in self.enemies.iter_mut() {
                *e = Enemy::read(&mut r);
            }
        }
        // Enemy teams: data.mkf #2.
        {
            let mut r = WordReader::new(files.data.chunk(2)?);
            for t in self.enemy_teams.iter_mut() {
                for e in t.enemy.iter_mut() {
                    *e = r.u16();
                }
            }
        }
        // Magics: data.mkf #4.
        {
            let mut r = WordReader::new(files.data.chunk(4)?);
            for m in self.magics.iter_mut() {
                *m = MagicData::read(&mut r);
            }
        }
        // Battle fields: data.mkf #5.
        {
            let mut r = WordReader::new(files.data.chunk(5)?);
            for b in self.battle_fields.iter_mut() {
                b.screen_wave = r.u16();
                for e in b.magic_effect.iter_mut() {
                    *e = r.i16();
                }
            }
        }
        // Level-up magics: data.mkf #6.
        {
            let mut r = WordReader::new(files.data.chunk(6)?);
            for l in self.level_up_magics.iter_mut() {
                for m in l.m.iter_mut() {
                    *m = (r.u16(), r.u16());
                }
            }
        }
        // Battle effect index: data.mkf #11.
        {
            let mut r = WordReader::new(files.data.chunk(11)?);
            for row in self.battle_effect_index.iter_mut() {
                row[0] = r.u16();
                row[1] = r.u16();
            }
        }
        // Enemy positions: data.mkf #13.
        {
            let mut r = WordReader::new(files.data.chunk(13)?);
            for row in self.enemy_pos.iter_mut() {
                for pos in row.iter_mut() {
                    *pos = (r.u16(), r.u16());
                }
            }
        }
        // Level-up EXP table: data.mkf #14.
        {
            let mut r = WordReader::new(files.data.chunk(14)?);
            for v in self.level_up_exp.iter_mut() {
                *v = r.u16();
            }
        }
        Ok(())
    }

    /// The data-file part of PAL_LoadDefaultGame: event objects, scenes,
    /// objects (with DOS->WIN conversion) and player roles.
    pub fn reload_default(&mut self, files: &GameFiles) -> io::Result<()> {
        {
            let mut r = WordReader::new(files.sss.chunk(0)?);
            for eo in self.event_objects.iter_mut() {
                *eo = EventObject::read(&mut r);
            }
        }
        {
            let mut r = WordReader::new(files.sss.chunk(1)?);
            for s in self.scenes.iter_mut() {
                *s = Scene::read(&mut r);
            }
        }
        {
            let mut r = WordReader::new(files.sss.chunk(2)?);
            for obj in self.objects.iter_mut() {
                let dos = [r.u16(), r.u16(), r.u16(), r.u16(), r.u16(), r.u16()];
                *obj = GameObject::from_dos(dos);
            }
        }
        {
            let mut r = WordReader::new(files.data.chunk(3)?);
            self.player_roles = PlayerRoles::read(&mut r);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> DataDir {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        DataDir::new().expect("game data dir")
    }

    fn globals() -> Globals {
        Globals::init(data()).expect("init globals")
    }

    #[test]
    fn lcg_step_matches_c_constants() {
        // util.c: glSeed = 1664525 * glSeed + 1013904223, 32-bit wraparound.
        assert_eq!(lcg_step(0), 1_013_904_223);
        assert_eq!(
            lcg_step(12345),
            12345i32.wrapping_mul(1664525).wrapping_add(1013904223)
        );
        // Overflow wraps silently rather than panicking (C's unsigned math).
        assert_eq!(
            lcg_step(i32::MAX),
            i32::MAX.wrapping_mul(1664525).wrapping_add(1013904223)
        );
        // Cold-start seeding advances the LCG twice (lsrand + lrand), so a draw
        // from wall-clock T is derived from lcg_step(lcg_step(T)), not one step.
        let t = 1_600_000_000;
        let cold_seed = lcg_step(lcg_step(t));
        assert_ne!(cold_seed, lcg_step(t));
    }

    #[test]
    fn loads_real_game_data() {
        let g = globals();
        // The DOS data set: verify counts derived from real chunk sizes.
        assert!(g.game.event_objects.len() > 1000, "too few event objects");
        assert!(
            g.game.script_entries.len() > 10000,
            "too few script entries"
        );
        assert!(g.game.enemies.len() > 100, "too few enemies");
        assert!(g.game.enemy_teams.len() > 100, "too few enemy teams");
        assert!(g.game.magics.len() > 100, "too few magics");
        assert!(!g.game.stores.is_empty());
        assert!(!g.game.battle_fields.is_empty());
        assert!(!g.game.level_up_magics.is_empty());
        // Scene 1 must reference a valid map.
        assert!(g.game.scenes[0].map_num > 0);
        // Level-up EXP table must be nonzero and non-decreasing overall.
        assert!(g.game.level_up_exp[1] > 0);
        assert!(g.game.level_up_exp[50] >= g.game.level_up_exp[10]);
    }

    #[test]
    fn default_game_state() {
        let mut g = globals();
        g.load_default_game().unwrap();
        assert_eq!(g.num_scene, 1);
        assert_eq!(g.cash, 0);
        assert!(g.entering_scene);
        // Li Xiaoyao (role 0) starts alive at level >= 1.
        assert!(g.game.player_roles.level[0] >= 1);
        assert!(g.game.player_roles.max_hp[0] > 0);
        // Exp levels mirror role levels.
        assert_eq!(g.exp.primary_exp[0].level, g.game.player_roles.level[0]);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut g = globals();
        g.load_default_game().unwrap();
        g.cash = 12345;
        g.num_scene = 42;
        g.max_party_member_index = 1;
        g.party[0].player_role = 0;
        g.party[0].x = 100;
        g.party[0].y = -3;
        g.add_item_to_inventory(0x100, 5);
        g.game.player_roles.hp[0] = 77;

        let bytes = g.save_game_to_bytes(9);
        // Fixed part is 12864 bytes; event objects follow.
        assert_eq!(bytes.len(), 12864 + g.game.event_objects.len() * 32);

        let mut g2 = globals();
        g2.load_game_from_bytes(&bytes).unwrap();
        assert_eq!(g2.cash, 12345);
        assert_eq!(g2.num_scene, 42);
        assert_eq!(g2.max_party_member_index, 1);
        assert_eq!(g2.party[0].x, 100);
        assert_eq!(g2.party[0].y, -3);
        assert_eq!(g2.get_item_amount(0x100), 5);
        assert_eq!(g2.game.player_roles.hp[0], 77);
        assert!(!g2.entering_scene);
    }

    #[test]
    fn inventory_operations() {
        let mut g = globals();
        g.load_default_game().unwrap();
        assert_eq!(g.add_item_to_inventory(10, 3), 1);
        assert_eq!(g.get_item_amount(10), 3);
        assert_eq!(g.add_item_to_inventory(10, 200), 1);
        assert_eq!(g.get_item_amount(10), 99); // capped
        assert_eq!(g.add_item_to_inventory(10, -99), 1);
        assert_eq!(g.get_item_amount(10), 0);
        // Removing more than we have reports the shortage.
        assert_eq!(g.add_item_to_inventory(11, 2), 1);
        assert_eq!(g.add_item_to_inventory(11, -5), -3);
        // Removing an item we don't have fails.
        assert_eq!(g.add_item_to_inventory(999, -1), 0);
        g.compress_inventory();
        assert_eq!(g.inventory[0].item, 0);
    }

    #[test]
    fn status_and_magic() {
        let mut g = globals();
        g.load_default_game().unwrap();
        g.game.player_roles.hp[0] = 50;
        assert!(g.set_player_status(0, STATUS_BRAVERY, 3));
        assert_eq!(g.player_status[0][STATUS_BRAVERY], 3);
        // "bad" status doesn't overwrite an existing one
        assert!(g.set_player_status(0, STATUS_SLEEP, 4));
        assert!(g.set_player_status(0, STATUS_SLEEP, 9));
        assert_eq!(g.player_status[0][STATUS_SLEEP], 4);
        // puppet requires a dead player
        assert!(!g.set_player_status(0, STATUS_PUPPET, 2));
        g.remove_player_status(0, STATUS_SLEEP);
        assert_eq!(g.player_status[0][STATUS_SLEEP], 0);
        // equipment effects (>999) are not cleared
        g.player_status[0][STATUS_DUALATTACK] = 1000;
        g.clear_all_player_status();
        assert_eq!(g.player_status[0][STATUS_DUALATTACK], 1000);

        assert!(g.add_magic(0, 0x123));
        assert!(!g.add_magic(0, 0x123));
        g.remove_magic(0, 0x123);
        assert!(g.add_magic(0, 0x123));
    }

    #[test]
    fn level_up_caps_stats() {
        seed_random(42);
        let mut g = globals();
        g.load_default_game().unwrap();
        let lvl = g.game.player_roles.level[0];
        g.player_level_up(0, 5);
        assert_eq!(g.game.player_roles.level[0], (lvl + 5).min(99));
        g.player_level_up(0, 200);
        assert_eq!(g.game.player_roles.level[0], 99);
        assert!(g.game.player_roles.max_hp[0] <= 999);
        assert_eq!(g.exp.primary_exp[0].exp, 0);
    }

    #[test]
    fn hp_mp_changes() {
        let mut g = globals();
        g.load_default_game().unwrap();
        let role = 0;
        g.game.player_roles.max_hp[role] = 100;
        g.game.player_roles.max_mp[role] = 50;
        g.game.player_roles.hp[role] = 60;
        g.game.player_roles.mp[role] = 20;
        assert!(g.increase_hp_mp(role, 100, 0));
        assert_eq!(g.game.player_roles.hp[role], 100); // capped at max
        assert!(g.increase_hp_mp(role, -200, 0));
        assert_eq!(g.game.player_roles.hp[role], 0); // floored at 0
                                                     // Dead player: no effect.
        assert!(!g.increase_hp_mp(role, 10, 10));
    }
}
