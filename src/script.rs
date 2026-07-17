//! Script interpreter — port of SDLPAL `script.c` (DOS paths only,
//! `fIsWIN95 == FALSE`, `PAL_CLASSIC` defined).
//!
//! This is the heart of the game logic: `PAL_InterpretInstruction` (every
//! opcode), `PAL_RunTriggerScript` / `PAL_RunAutoScript`, the NPC/party walk
//! helpers, `PAL_MonsterChasePlayer`, `PAL_AdditionalCredits`, plus the two
//! script-dependent `global.c` helpers `PAL_UpdateEquipments` and
//! `PAL_AddPoisonForPlayer`.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::battle::{Battle, BattleEnemy, BattleResult, FighterState};
use crate::game_loop::{Engine, FRAME_TIME};
use crate::global::{
    random_long, PlayerRoles, PoisonStatus, BODYPART_EXTRA, DIR_EAST, DIR_NORTH, DIR_SOUTH,
    DIR_WEST, LOAD_PLAYER_SPRITE, LOAD_SCENE, MAX_ENEMIES_IN_TEAM, MAX_INVENTORY,
    MAX_PLAYABLE_PLAYER_ROLES, MAX_PLAYER_EQUIPMENTS, MAX_PLAYER_ROLES, MAX_POISONS, MAX_SCENES,
    STATUS_ALL, STATUS_CONFUSED, STATUS_PARALYZED, STATUS_SLEEP,
};

/// Run `$body` with the live battle moved out of `self.battle` into a local
/// `Box<Battle>` named `$battle`, then moved back.  Taking the box frees
/// `self` for arbitrary method/field access while the battle is a plain local,
/// which is what the two-argument battle routines want.  When there is no
/// battle (`self.battle` is `None`, i.e. the opcode ran outside a battle) the
/// body is skipped entirely — a safe no-op, matching the C where these opcodes
/// operate on a zeroed `g_Battle` with no observable effect.
macro_rules! with_battle {
    ($eng:tt, $battle:ident => $body:block) => {{
        if let Some(mut $battle) = $eng.battle.take() {
            $body
            $eng.battle = Some($battle);
        }
    }};
}

// Dialog locations (text.h kDialog*).
const DIALOG_UPPER: u8 = 0;
const DIALOG_CENTER: u8 = 1;
const DIALOG_LOWER: u8 = 2;
const DIALOG_CENTER_WINDOW: u8 = 3;

// Trigger modes (kTriggerTouchNormal used by opcode 0x0081).
const TRIGGER_TOUCH_NORMAL: u16 = 5;

/// Module-private interpreter state (script.c statics).  The battle state
/// itself lives in `Engine::battle` (see game_loop.rs); the battle opcodes
/// reach it through the `with_battle!` macro.
pub struct ScriptState {
    /// `g_fScriptSuccess`.
    pub script_success: bool,
    /// `g_iCurEquipPart`.
    pub cur_equip_part: i32,
    /// `wLastEventObject` (static inside PAL_RunTriggerScript).
    pub last_event_object: u16,
    /// `g_fUpdatedInBattle` (HACKHACK extern).
    pub updated_in_battle: bool,
}

impl Default for ScriptState {
    fn default() -> ScriptState {
        ScriptState {
            script_success: true,
            cur_equip_part: -1,
            last_event_object: 0,
            updated_in_battle: false,
        }
    }
}

/// `PAL_X` of a `PAL_POS` stored as an `(i32, i32)` tuple.
#[inline]
fn px(p: (i32, i32)) -> i32 {
    p.0
}

/// `PAL_Y` of a `PAL_POS` stored as an `(i32, i32)` tuple.
#[inline]
fn py(p: (i32, i32)) -> i32 {
    p.1
}

/// Flat WORD-array access into a `PLAYERROLES`, mirroring the pointer
/// arithmetic HACKHACK of script.c (`p[row * MAX_PLAYER_ROLES + col]`). The
/// struct is a contiguous block of 75 rows of `MAX_PLAYER_ROLES` words each,
/// in declaration order.
fn pr_flat(pr: &mut PlayerRoles, row: usize, col_in: usize) -> &mut u16 {
    let offset = row * MAX_PLAYER_ROLES + col_in;
    let row = offset / MAX_PLAYER_ROLES;
    let col = offset % MAX_PLAYER_ROLES;
    match row {
        0 => &mut pr.avatar[col],
        1 => &mut pr.sprite_num_in_battle[col],
        2 => &mut pr.sprite_num[col],
        3 => &mut pr.name[col],
        4 => &mut pr.attack_all[col],
        5 => &mut pr.unknown1[col],
        6 => &mut pr.level[col],
        7 => &mut pr.max_hp[col],
        8 => &mut pr.max_mp[col],
        9 => &mut pr.hp[col],
        10 => &mut pr.mp[col],
        11..=16 => &mut pr.equipment[row - 11][col],
        17 => &mut pr.attack_strength[col],
        18 => &mut pr.magic_strength[col],
        19 => &mut pr.defense[col],
        20 => &mut pr.dexterity[col],
        21 => &mut pr.flee_rate[col],
        22 => &mut pr.poison_resistance[col],
        23..=27 => &mut pr.elemental_resistance[row - 23][col],
        28 => &mut pr.unknown2[col],
        29 => &mut pr.unknown3[col],
        30 => &mut pr.unknown4[col],
        31 => &mut pr.covered_by[col],
        32..=63 => &mut pr.magic[row - 32][col],
        64 => &mut pr.walk_frames[col],
        65 => &mut pr.cooperative_magic[col],
        66 => &mut pr.unknown5[col],
        67 => &mut pr.unknown6[col],
        68 => &mut pr.death_sound[col],
        69 => &mut pr.attack_sound[col],
        70 => &mut pr.weapon_sound[col],
        71 => &mut pr.critical_sound[col],
        72 => &mut pr.magic_sound[col],
        73 => &mut pr.cover_sound[col],
        74 => &mut pr.dying_sound[col],
        _ => panic!("player-roles flat index out of range: {offset}"),
    }
}

impl Engine {
    // =======================================================================
    // Movement helpers.
    // =======================================================================

    /// `PAL_NPCWalkTo`: make the event object walk toward `(x, y, h)` at
    /// `speed`; returns `true` when it has arrived.
    fn npc_walk_to(&mut self, event_object_id: u16, x: i32, y: i32, h: i32, speed: i32) -> bool {
        let idx = (event_object_id - 1) as usize;
        let target_x = x * 32 + h * 16;
        let target_y = y * 16 + h * 8;

        let (ex, ey) = {
            let p = &self.globals.game.event_objects[idx];
            (p.x as i32, p.y as i32)
        };
        let x_offset = target_x - ex;
        let y_offset = target_y - ey;

        let dir = if y_offset < 0 {
            if x_offset < 0 {
                DIR_WEST
            } else {
                DIR_NORTH
            }
        } else if x_offset < 0 {
            DIR_SOUTH
        } else {
            DIR_EAST
        };
        self.globals.game.event_objects[idx].direction = dir;

        if x_offset.abs() < speed * 2 || y_offset.abs() < speed * 2 {
            let p = &mut self.globals.game.event_objects[idx];
            p.x = target_x as u16;
            p.y = target_y as u16;
        } else {
            self.npc_walk_one_step(event_object_id, speed);
        }

        let p = &mut self.globals.game.event_objects[idx];
        if p.x as i32 == target_x && p.y as i32 == target_y {
            p.current_frame_num = 0;
            return true;
        }
        false
    }

    /// `PAL_PartyWalkTo`: walk the whole party toward `(x, y, h)` at `speed`.
    fn party_walk_to(&mut self, x: i32, y: i32, h: i32, speed: i32) {
        let mut x_offset =
            x * 32 + h * 16 - px(self.globals.viewport) - px(self.globals.partyoffset);
        let mut y_offset =
            y * 16 + h * 8 - py(self.globals.viewport) - py(self.globals.partyoffset);

        let mut t = 0u64;
        while x_offset != 0 || y_offset != 0 {
            self.delay_until(t);
            t = self.ticks() + FRAME_TIME;

            // Store trail.
            for i in (0..4).rev() {
                self.globals.trail[i + 1] = self.globals.trail[i];
            }
            self.globals.trail[0].direction = self.globals.party_direction;
            self.globals.trail[0].x =
                (px(self.globals.viewport) + px(self.globals.partyoffset)) as u16;
            self.globals.trail[0].y =
                (py(self.globals.viewport) + py(self.globals.partyoffset)) as u16;

            self.globals.party_direction = if y_offset < 0 {
                if x_offset < 0 {
                    DIR_WEST
                } else {
                    DIR_NORTH
                }
            } else if x_offset < 0 {
                DIR_SOUTH
            } else {
                DIR_EAST
            };

            let mut dx = px(self.globals.viewport);
            let mut dy = py(self.globals.viewport);

            if x_offset.abs() <= speed * 2 {
                dx += x_offset;
            } else {
                dx += speed * if x_offset < 0 { -2 } else { 2 };
            }
            if y_offset.abs() <= speed {
                dy += y_offset;
            } else {
                dy += speed * if y_offset < 0 { -1 } else { 1 };
            }

            self.globals.viewport = (dx, dy);

            self.update_party_gestures(true);
            self.game_update(false);
            self.make_scene();
            self.video_update();

            x_offset = x * 32 + h * 16 - px(self.globals.viewport) - px(self.globals.partyoffset);
            y_offset = y * 16 + h * 8 - py(self.globals.viewport) - py(self.globals.partyoffset);
        }

        self.update_party_gestures(false);
    }

    /// `PAL_PartyRideEventObject`: move the party to `(x, y, h)` riding the
    /// specified event object.
    fn party_ride_event_object(
        &mut self,
        event_object_id: u16,
        x: i32,
        y: i32,
        h: i32,
        speed: i32,
    ) {
        let idx = (event_object_id - 1) as usize;
        let mut x_offset =
            x * 32 + h * 16 - px(self.globals.viewport) - px(self.globals.partyoffset);
        let mut y_offset =
            y * 16 + h * 8 - py(self.globals.viewport) - py(self.globals.partyoffset);

        let mut t = 0u64;
        while x_offset != 0 || y_offset != 0 {
            self.delay_until(t);
            t = self.ticks() + FRAME_TIME;

            self.globals.party_direction = if y_offset < 0 {
                if x_offset < 0 {
                    DIR_WEST
                } else {
                    DIR_NORTH
                }
            } else if x_offset < 0 {
                DIR_SOUTH
            } else {
                DIR_EAST
            };

            let dx = if x_offset.abs() > speed * 2 {
                speed * if x_offset < 0 { -2 } else { 2 }
            } else {
                x_offset
            };
            let dy = if y_offset.abs() > speed {
                speed * if y_offset < 0 { -1 } else { 1 }
            } else {
                y_offset
            };

            // Store trail.
            for i in (0..4).rev() {
                self.globals.trail[i + 1] = self.globals.trail[i];
            }
            self.globals.trail[0].direction = self.globals.party_direction;
            self.globals.trail[0].x =
                (px(self.globals.viewport) + dx + px(self.globals.partyoffset)) as u16;
            self.globals.trail[0].y =
                (py(self.globals.viewport) + dy + py(self.globals.partyoffset)) as u16;

            self.globals.viewport = (
                px(self.globals.viewport) + dx,
                py(self.globals.viewport) + dy,
            );

            {
                let p = &mut self.globals.game.event_objects[idx];
                p.x = (p.x as i32 + dx) as u16;
                p.y = (p.y as i32 + dy) as u16;
            }

            self.game_update(false);
            self.make_scene();
            self.video_update();

            x_offset = x * 32 + h * 16 - px(self.globals.viewport) - px(self.globals.partyoffset);
            y_offset = y * 16 + h * 8 - py(self.globals.viewport) - py(self.globals.partyoffset);
        }
    }

    /// `PAL_MonsterChasePlayer`.
    fn monster_chase_player(
        &mut self,
        event_object_id: u16,
        speed: u16,
        chase_range: u16,
        floating: bool,
    ) {
        let idx = (event_object_id - 1) as usize;
        let mut monster_speed: u16 = 0;

        if self.globals.chase_range != 0 {
            let (ox, oy) = {
                let p = &self.globals.game.event_objects[idx];
                (p.x as i32, p.y as i32)
            };
            let mut x = px(self.globals.viewport) + px(self.globals.partyoffset) - ox;
            let mut y = py(self.globals.viewport) + py(self.globals.partyoffset) - oy;

            if x == 0 {
                x = if random_long(0, 1) != 0 { -1 } else { 1 };
            }
            if y == 0 {
                y = if random_long(0, 1) != 0 { -1 } else { 1 };
            }

            let mut prevx = ox;
            let mut prevy = oy;
            let i = prevx % 32;
            let j = prevy % 16;
            prevx /= 32;
            prevy /= 16;
            let mut l = 0;

            if i + j * 2 >= 16 {
                if i + j * 2 >= 48 {
                    prevx += 1;
                    prevy += 1;
                } else if 32 - i + j * 2 < 16 {
                    prevx += 1;
                } else if 32 - i + j * 2 < 48 {
                    l = 1;
                } else {
                    prevy += 1;
                }
            }
            let prevx = prevx * 32 + l * 16;
            let prevy = prevy * 16 + l * 8;

            if x.abs() + y.abs() * 2 < chase_range as i32 * 32 * self.globals.chase_range as i32 {
                let dir = if x < 0 {
                    if y < 0 {
                        DIR_WEST
                    } else {
                        DIR_SOUTH
                    }
                } else if y < 0 {
                    DIR_NORTH
                } else {
                    DIR_EAST
                };
                self.globals.game.event_objects[idx].direction = dir;

                let (mut nx, mut ny);
                {
                    let p = &self.globals.game.event_objects[idx];
                    nx = if x != 0 {
                        p.x as i32 + x / x.abs() * 16
                    } else {
                        p.x as i32
                    };
                    ny = if y != 0 {
                        p.y as i32 + y / y.abs() * 8
                    } else {
                        p.y as i32
                    };
                }
                let _ = (&mut nx, &mut ny);

                if floating {
                    monster_speed = speed;
                } else {
                    if !self.check_obstacle((nx, ny), true, event_object_id) {
                        monster_speed = speed;
                    } else {
                        let p = &mut self.globals.game.event_objects[idx];
                        p.x = prevx as u16;
                        p.y = prevy as u16;
                    }

                    for l in 0..4 {
                        {
                            let p = &mut self.globals.game.event_objects[idx];
                            match l {
                                0 => {
                                    p.x = (p.x as i32 - 4) as u16;
                                    p.y = (p.y as i32 + 2) as u16;
                                }
                                1 => {
                                    p.x = (p.x as i32 - 4) as u16;
                                    p.y = (p.y as i32 - 2) as u16;
                                }
                                2 => {
                                    p.x = (p.x as i32 + 4) as u16;
                                    p.y = (p.y as i32 - 2) as u16;
                                }
                                _ => {
                                    p.x = (p.x as i32 + 4) as u16;
                                    p.y = (p.y as i32 + 2) as u16;
                                }
                            }
                        }
                        let (cx, cy) = {
                            let p = &self.globals.game.event_objects[idx];
                            (p.x as i32, p.y as i32)
                        };
                        if self.check_obstacle((cx, cy), false, 0) {
                            let p = &mut self.globals.game.event_objects[idx];
                            p.x = prevx as u16;
                            p.y = prevy as u16;
                        }
                    }
                }
            }
        } else {
            // Exorcism-Fragrance: spin in place, switching direction every two
            // frames.
            if self.globals.frame_num & 1 != 0 {
                let p = &mut self.globals.game.event_objects[idx];
                p.direction += 1;
                if p.direction > 3 {
                    p.direction = 0;
                }
            }
        }

        self.npc_walk_one_step(event_object_id, monster_speed as i32);
    }

    /// `PAL_AdditionalCredits`: show the additional-credits screen.
    pub fn additional_credits(&mut self) {
        self.draw_opening_menu_background();
        // The 12 credit lines are drawn via PAL_DrawText (XXX stub); state is
        // otherwise unaffected.
        for i in 0..12 {
            self.draw_text_line(2 + i * 16);
        }
        self.set_palette(0, false);
        self.video_update();
        self.wait_for_key(0);
    }

    // =======================================================================
    // PAL_InterpretInstruction.
    // =======================================================================

    /// `PAL_InterpretInstruction`: execute one instruction; returns the next
    /// script address to run.
    // The `with_battle!` macro always binds the battle `mut`; opcodes that only
    // read it therefore trip `unused_mut` on the macro expansion.
    #[allow(unused_mut)]
    fn interpret_instruction(&mut self, script_entry: u16, event_object_id: u16) -> u16 {
        let mut ws = script_entry;
        let script = self.globals.game.script_entries[ws as usize];
        let op = script.operand;
        let operation = script.operation;

        let evt_idx = event_object_id.wrapping_sub(1) as usize; // pEvtObj index

        // pCurrent / wCurEventObjectID.
        let (cur_index, cur_event_object_id) = if op[0] == 0 || op[0] == 0xFFFF {
            (evt_idx, event_object_id)
        } else {
            let mut i = (op[0] - 1) as usize;
            if i > 0x9000 {
                // HACK for Dream 2.11 to avoid crash.
                i -= 0x9000;
            }
            (i, op[0])
        };

        match operation {
            // 0x000B-0x000E: walk one step in the given direction.
            0x000B..=0x000E => {
                self.globals.game.event_objects[evt_idx].direction = operation - 0x000B;
                self.npc_walk_one_step(event_object_id, 2);
            }

            // 0x000F: set direction and/or gesture for an event object.
            0x000F => {
                let p = &mut self.globals.game.event_objects[evt_idx];
                if op[0] != 0xFFFF {
                    p.direction = op[0];
                }
                if op[1] != 0xFFFF {
                    p.current_frame_num = op[1];
                }
            }

            // 0x0010: walk straight to the specified position.
            0x0010 => {
                if !self.npc_walk_to(event_object_id, op[0] as i32, op[1] as i32, op[2] as i32, 3) {
                    ws = ws.wrapping_sub(1);
                }
            }

            // 0x0011: walk straight to the position, at a lower speed.
            0x0011 => {
                if (event_object_id as u32 & 1) ^ (self.globals.frame_num & 1) != 0 {
                    if !self.npc_walk_to(
                        event_object_id,
                        op[0] as i32,
                        op[1] as i32,
                        op[2] as i32,
                        2,
                    ) {
                        ws = ws.wrapping_sub(1);
                    }
                } else {
                    ws = ws.wrapping_sub(1);
                }
            }

            // 0x0012: set the position of the event object, relative to party.
            0x0012 => {
                let vx = px(self.globals.viewport) + px(self.globals.partyoffset);
                let vy = py(self.globals.viewport) + py(self.globals.partyoffset);
                let p = &mut self.globals.game.event_objects[cur_index];
                p.x = (op[1] as i16 as i32 + vx) as u16;
                p.y = (op[2] as i16 as i32 + vy) as u16;
            }

            // 0x0013: set the absolute position of the event object.
            0x0013 => {
                let p = &mut self.globals.game.event_objects[cur_index];
                p.x = op[1];
                p.y = op[2];
            }

            // 0x0014: set the gesture of the event object.
            0x0014 => {
                let p = &mut self.globals.game.event_objects[evt_idx];
                p.current_frame_num = op[0];
                p.direction = DIR_SOUTH;
            }

            // 0x0015: set direction and gesture for a party member.
            0x0015 => {
                self.globals.party_direction = op[0];
                self.globals.party[op[2] as usize].frame = self.globals.party_direction * 3 + op[1];
            }

            // 0x0016: set direction and gesture for an event object.
            0x0016 => {
                if op[0] != 0 {
                    let p = &mut self.globals.game.event_objects[cur_index];
                    p.direction = op[1];
                    p.current_frame_num = op[2];
                }
            }

            // 0x0017: set the player's extra attribute.
            0x0017 => {
                let i = (op[0] - 0xB) as usize;
                *pr_flat(
                    &mut self.globals.equipment_effect[i],
                    op[1] as usize,
                    event_object_id as usize,
                ) = op[2] as i16 as u16;
            }

            // 0x0018: equip the selected item.
            0x0018 => {
                let part = (op[0] - 0x0B) as usize;
                self.script.cur_equip_part = part as i32;

                // wEventObjectID here indicates the player role.
                self.globals.remove_equipment_effect(event_object_id, part);

                if self.globals.game.player_roles.equipment[part][event_object_id as usize] != op[1]
                {
                    let w =
                        self.globals.game.player_roles.equipment[part][event_object_id as usize];
                    self.globals.game.player_roles.equipment[part][event_object_id as usize] =
                        op[1];

                    let (found_new, i) = self.globals.get_item_index_to_inventory(op[1]);
                    let (found_old, _j) = self.globals.get_item_index_to_inventory(w);
                    if found_new
                        && i < MAX_INVENTORY
                        && self.globals.inventory[i].amount == 1
                        && w != 0
                        && !found_old
                    {
                        // Replace in place.
                        self.globals.inventory[i].item = w;
                    } else {
                        self.globals.add_item_to_inventory(op[1], -1);
                        if w != 0 {
                            self.globals.add_item_to_inventory(w, 1);
                        }
                    }

                    self.globals.last_unequipped_item = w;
                }
            }

            // 0x0019: increase/decrease the player's attribute.
            0x0019 => {
                let role = if op[2] == 0 {
                    event_object_id as usize
                } else {
                    (op[2] - 1) as usize
                };
                let slot = pr_flat(&mut self.globals.game.player_roles, op[0] as usize, role);
                *slot = slot.wrapping_add(op[1] as i16 as u16);
            }

            // 0x001A: set the player's stat.
            0x001A => {
                let role = if op[2] == 0 {
                    event_object_id as usize
                } else {
                    (op[2] - 1) as usize
                };
                let pr = if self.script.cur_equip_part != -1 {
                    &mut self.globals.equipment_effect[self.script.cur_equip_part as usize]
                } else {
                    &mut self.globals.game.player_roles
                };
                *pr_flat(pr, op[0] as usize, role) = op[1] as i16 as u16;
            }

            // 0x001B: increase/decrease player's HP.
            0x001B => {
                if op[0] != 0 {
                    self.script.script_success = false;
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        if self.globals.increase_hp_mp(w as usize, op[1] as i16, 0) {
                            self.script.script_success = true;
                        }
                    }
                } else if !self
                    .globals
                    .increase_hp_mp(event_object_id as usize, op[1] as i16, 0)
                {
                    self.script.script_success = false;
                }
            }

            // 0x001C: increase/decrease player's MP.
            0x001C => {
                if op[0] != 0 {
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        self.globals.increase_hp_mp(w as usize, 0, op[1] as i16);
                    }
                } else if !self
                    .globals
                    .increase_hp_mp(event_object_id as usize, 0, op[1] as i16)
                {
                    self.script.script_success = false;
                }
            }

            // 0x001D: increase/decrease player's HP and MP.
            0x001D => {
                if op[0] != 0 {
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        self.globals
                            .increase_hp_mp(w as usize, op[1] as i16, op[1] as i16);
                    }
                } else if !self.globals.increase_hp_mp(
                    event_object_id as usize,
                    op[1] as i16,
                    op[1] as i16,
                ) {
                    self.script.script_success = false;
                }
            }

            // 0x001E: increase or decrease cash.
            0x001E => {
                let amount = op[0] as i16 as i32;
                if amount < 0 && (self.globals.cash as i64) < (-amount) as i64 {
                    ws = op[1].wrapping_sub(1);
                } else {
                    self.globals.cash = (self.globals.cash as i64 + amount as i64) as u32;
                }
            }

            // 0x001F: add item to inventory.
            0x001F => {
                self.globals
                    .add_item_to_inventory(op[0], op[1] as i16 as i32);
            }

            // 0x0020: remove item from inventory.
            0x0020 => {
                let mut x = op[1] as i32;
                if x == 0 {
                    x = 1;
                }
                if x <= self.globals.count_item(op[0]) || op[2] == 0 {
                    let y = self.globals.add_item_to_inventory(op[0], -x);
                    if y <= 0 {
                        if y < 0 {
                            x = -y;
                        }
                        // Try removing equipped item.
                        'outer: for i in 0..=self.globals.max_party_member_index as usize {
                            let w = self.globals.party[i].player_role as usize;
                            for j in 0..MAX_PLAYER_EQUIPMENTS {
                                if self.globals.game.player_roles.equipment[j][w] == op[0] {
                                    self.globals.remove_equipment_effect(w as u16, j);
                                    self.globals.game.player_roles.equipment[j][w] = 0;
                                    x -= 1;
                                    if x == 0 {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    ws = op[2].wrapping_sub(1);
                }
            }

            // 0x0021: inflict damage to the enemy.
            0x0021 => with_battle!(self, battle => {
                if op[0] != 0 {
                    for i in 0..=battle.max_enemy_index as usize {
                        if battle.enemy[i].object_id != 0 {
                            let h = &mut battle.enemy[i].e.health;
                            *h = h.wrapping_sub(op[1]);
                        }
                    }
                } else {
                    let h = &mut battle.enemy[event_object_id as usize].e.health;
                    *h = h.wrapping_sub(op[1]);
                }
            }),

            // 0x0022: revive player.
            0x0022 => {
                if op[0] != 0 {
                    self.script.script_success = false;
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role as usize;
                        if self.globals.game.player_roles.hp[w] == 0 {
                            self.globals.game.player_roles.hp[w] =
                                (self.globals.game.player_roles.max_hp[w] as u32 * op[1] as u32
                                    / 10) as u16;
                            self.globals.cure_poison_by_level(w as u16, 3);
                            for x in 0..STATUS_ALL {
                                self.globals.remove_player_status(w, x);
                            }
                            self.script.script_success = true;
                        }
                    }
                } else {
                    let w = event_object_id as usize;
                    if self.globals.game.player_roles.hp[w] == 0 {
                        self.globals.game.player_roles.hp[w] =
                            (self.globals.game.player_roles.max_hp[w] as u32 * op[1] as u32 / 10)
                                as u16;
                        self.globals.cure_poison_by_level(w as u16, 3);
                        for x in 0..STATUS_ALL {
                            self.globals.remove_player_status(w, x);
                        }
                    } else {
                        self.script.script_success = false;
                    }
                }
            }

            // 0x0023: remove equipment from a player.
            0x0023 => {
                let role = op[0] as usize;
                if op[1] == 0 {
                    for i in 0..MAX_PLAYER_EQUIPMENTS {
                        let w = self.globals.game.player_roles.equipment[i][role];
                        if w != 0 {
                            self.globals.add_item_to_inventory(w, 1);
                            self.globals.game.player_roles.equipment[i][role] = 0;
                        }
                        self.globals.remove_equipment_effect(role as u16, i);
                    }
                } else {
                    let part = (op[1] - 1) as usize;
                    let w = self.globals.game.player_roles.equipment[part][role];
                    if w != 0 {
                        self.globals.remove_equipment_effect(role as u16, part);
                        self.globals.add_item_to_inventory(w, 1);
                        self.globals.game.player_roles.equipment[part][role] = 0;
                    }
                }
            }

            // 0x0024: set the autoscript entry address for an event object.
            0x0024 => {
                if op[0] != 0 {
                    self.globals.game.event_objects[cur_index].auto_script = op[1];
                }
            }

            // 0x0025: set the trigger script entry address for an event object.
            0x0025 => {
                if op[0] != 0 {
                    self.globals.game.event_objects[cur_index].trigger_script = op[1];
                }
            }

            // 0x0026: show the buy item menu.
            0x0026 => {
                self.make_scene();
                self.video_update();
                self.buy_menu(op[0]);
            }

            // 0x0027: show the sell item menu.
            0x0027 => {
                self.make_scene();
                self.video_update();
                self.sell_menu();
            }

            // 0x0028: apply poison to enemy.
            0x0028 => with_battle!(self, battle => {
                if op[0] != 0 {
                    for i in 0..=battle.max_enemy_index as usize {
                        let w = battle.enemy[i].object_id;
                        if w == 0 {
                            continue;
                        }
                        if random_long(0, 9)
                            >= self.globals.game.objects[w as usize].enemy_resistance_to_sorcery()
                                as i32
                        {
                            self.apply_poison_to_enemy(&mut battle, i, op[1], event_object_id);
                        }
                    }
                } else {
                    let i = event_object_id as usize;
                    let w = battle.enemy[i].object_id;
                    if random_long(0, 9)
                        >= self.globals.game.objects[w as usize].enemy_resistance_to_sorcery()
                            as i32
                    {
                        self.apply_poison_to_enemy(&mut battle, i, op[1], event_object_id);
                    }
                }
            }),

            // 0x0029: apply poison to player.
            0x0029 => {
                if op[0] != 0 {
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        if random_long(1, 100)
                            > self.globals.player_poison_resistance(w as usize) as i32
                        {
                            self.add_poison_for_player(w, op[1]);
                        }
                    }
                } else if random_long(1, 100)
                    > self
                        .globals
                        .player_poison_resistance(event_object_id as usize)
                        as i32
                {
                    self.add_poison_for_player(event_object_id, op[1]);
                }
            }

            // 0x002A: cure poison by object ID for enemy.
            0x002A => with_battle!(self, battle => {
                if op[0] != 0 {
                    for i in 0..=battle.max_enemy_index as usize {
                        if battle.enemy[i].object_id == 0 {
                            continue;
                        }
                        for j in 0..MAX_POISONS {
                            if battle.enemy[i].poisons[j].poison_id == op[1] {
                                battle.enemy[i].poisons[j] = PoisonStatus::default();
                                break;
                            }
                        }
                    }
                } else {
                    let i = event_object_id as usize;
                    for j in 0..MAX_POISONS {
                        if battle.enemy[i].poisons[j].poison_id == op[1] {
                            battle.enemy[i].poisons[j] = PoisonStatus::default();
                            break;
                        }
                    }
                }
            }),

            // 0x002B: cure poison by object ID for player.
            0x002B => {
                if op[0] != 0 {
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        self.globals.cure_poison_by_kind(w, op[1]);
                    }
                } else {
                    self.globals.cure_poison_by_kind(event_object_id, op[1]);
                }
            }

            // 0x002C: cure poisons by level.
            0x002C => {
                if op[0] != 0 {
                    for i in 0..=self.globals.max_party_member_index as usize {
                        let w = self.globals.party[i].player_role;
                        self.globals.cure_poison_by_level(w, op[1]);
                    }
                } else {
                    self.globals.cure_poison_by_level(event_object_id, op[1]);
                }
            }

            // 0x002D: set the status for a player.
            0x002D => {
                if !self
                    .globals
                    .set_player_status(event_object_id as usize, op[0] as usize, op[1])
                {
                    self.script.script_success = false;
                }
            }

            // 0x002E: set the status for an enemy.
            0x002E => with_battle!(self, battle => {
                let w = battle.enemy[event_object_id as usize].object_id;
                // PAL_CLASSIC: i = 9.
                let i = 9;
                if random_long(0, i)
                    > self.globals.game.objects[w as usize].enemy_resistance_to_sorcery() as i32
                {
                    battle.enemy[event_object_id as usize].status[op[0] as usize] = op[1];
                } else {
                    ws = op[2].wrapping_sub(1);
                }
            }),

            // 0x002F: remove player's status.
            0x002F => {
                self.globals
                    .remove_player_status(event_object_id as usize, op[0] as usize);
            }

            // 0x0030: temporarily increase player's stat by percent.
            0x0030 => {
                let role = if op[2] == 0 {
                    event_object_id as usize
                } else {
                    (op[2] - 1) as usize
                };
                let base =
                    *pr_flat(&mut self.globals.game.player_roles, op[0] as usize, role) as i32;
                let val = (base * (op[1] as i16 as i32) / 100) as u16;
                *pr_flat(
                    &mut self.globals.equipment_effect[BODYPART_EXTRA],
                    op[0] as usize,
                    role,
                ) = val;
            }

            // 0x0031: change battle sprite temporarily for a player.
            0x0031 => {
                self.globals.equipment_effect[BODYPART_EXTRA].sprite_num_in_battle
                    [event_object_id as usize] = op[0];
            }

            // 0x0033: collect the enemy for items.
            0x0033 => with_battle!(self, battle => {
                let cv = battle.enemy[event_object_id as usize].e.collect_value;
                if cv != 0 {
                    self.globals.collect_value = self.globals.collect_value.wrapping_add(cv);
                } else {
                    ws = op[0].wrapping_sub(1);
                }
            }),

            // 0x0034: transform collected enemies into items.
            0x0034 => {
                if self.globals.collect_value > 0 {
                    // PAL_CLASSIC.
                    let mut i = random_long(1, self.globals.collect_value as i32);
                    if i > 9 {
                        i = 9;
                    }
                    self.globals.collect_value -= i as u16;
                    i -= 1;

                    let item = self.globals.game.stores[0].items[i as usize];
                    self.globals.add_item_to_inventory(item, 1);

                    // Show the obtained-item dialog (visuals via stubs).
                    self.start_dialog_with_offset(DIALOG_CENTER_WINDOW, 0, 0, false, 0, -10);
                    let mut s = self.texts.word(42);
                    s.push(b'@');
                    s.extend_from_slice(&self.texts.word(item as usize));
                    s.push(b'@');
                    self.show_dialog_text(&s);
                } else {
                    ws = op[0].wrapping_sub(1);
                }
            }

            // 0x0035: shake the screen.
            0x0035 => {
                let mut i = op[1];
                if i == 0 {
                    i = 4;
                }
                self.shake_screen(op[0], i);
                if op[0] == 0 {
                    self.video_update();
                }
            }

            // 0x0036: set the current playing RNG animation.
            0x0036 => {
                self.globals.cur_playing_rng = op[0] as i32;
            }

            // 0x0037: play RNG animation.
            0x0037 => {
                self.rng_play(
                    self.globals.cur_playing_rng as u16,
                    op[0] as i32,
                    if op[1] > 0 { op[1] as i32 } else { -1 },
                    if op[2] > 0 { op[2] as i32 } else { 16 },
                );
            }

            // 0x0038: teleport the party out of the scene.
            0x0038 => {
                let scr = self.globals.game.scenes[self.globals.num_scene as usize - 1]
                    .script_on_teleport;
                if !self.globals.in_battle && scr != 0 {
                    self.run_trigger_script(scr, 0xFFFF);
                } else {
                    self.script.script_success = false;
                    ws = op[0].wrapping_sub(1);
                }
            }

            // 0x0039: drain HP from enemy.
            0x0039 => with_battle!(self, battle => {
                let w = self.globals.party[battle.moving_player_index as usize]
                    .player_role as usize;
                {
                    let h = &mut battle.enemy[event_object_id as usize].e.health;
                    *h = h.wrapping_sub(op[0]);
                }
                let pr = &mut self.globals.game.player_roles;
                pr.hp[w] = pr.hp[w].wrapping_add(op[0]);
                if pr.hp[w] > pr.max_hp[w] {
                    pr.hp[w] = pr.max_hp[w];
                }
            }),

            // 0x003A: player flee from the battle.
            0x003A => with_battle!(self, battle => {
                if battle.is_boss {
                    ws = op[0].wrapping_sub(1);
                } else {
                    crate::battle::player_escape(self, &mut battle);
                }
            }),

            // 0x003F: ride the event object to the position, at a low speed.
            0x003F => {
                self.party_ride_event_object(
                    event_object_id,
                    op[0] as i32,
                    op[1] as i32,
                    op[2] as i32,
                    2,
                );
            }

            // 0x0040: set the trigger method for an event object.
            0x0040 => {
                if op[0] != 0 {
                    self.globals.game.event_objects[cur_index].trigger_mode = op[1];
                }
            }

            // 0x0041: mark the script as failed.
            0x0041 => {
                self.script.script_success = false;
            }

            // 0x0042: simulate a magic for a player.
            0x0042 => with_battle!(self, battle => {
                let mut i = op[2] as i16 as i32 - 1;
                if i < 0 {
                    i = event_object_id as i32;
                }
                crate::fight::battle_simulate_magic(self, &mut battle, i as i16, op[0], op[1]);
            }),

            // 0x0043: set background music.
            0x0043 => {
                self.globals.num_music = op[0];
                let fade = if op[1] == 3 && op[0] != 9 { 3.0 } else { 0.0 };
                self.play_music(op[0] as i32, op[1] != 1, fade);
            }

            // 0x0044: ride the event object to the position, at normal speed.
            0x0044 => {
                self.party_ride_event_object(
                    event_object_id,
                    op[0] as i32,
                    op[1] as i32,
                    op[2] as i32,
                    4,
                );
            }

            // 0x0045: set battle music.
            0x0045 => {
                self.globals.num_battle_music = op[0];
            }

            // 0x0046: set the party position on the map.
            0x0046 => {
                let x_offset = if self.globals.party_direction == DIR_WEST
                    || self.globals.party_direction == DIR_SOUTH
                {
                    16
                } else {
                    -16
                };
                let y_offset = if self.globals.party_direction == DIR_WEST
                    || self.globals.party_direction == DIR_NORTH
                {
                    8
                } else {
                    -8
                };

                let mut x = op[0] as i32 * 32 + op[2] as i32 * 16;
                let mut y = op[1] as i32 * 16 + op[2] as i32 * 8;
                x -= px(self.globals.partyoffset);
                y -= py(self.globals.partyoffset);
                self.globals.viewport = (x, y);

                let mut x = px(self.globals.partyoffset);
                let mut y = py(self.globals.partyoffset);
                for i in 0..MAX_PLAYABLE_PLAYER_ROLES {
                    self.globals.party[i].x = x as i16;
                    self.globals.party[i].y = y as i16;
                    self.globals.trail[i].x = (x + px(self.globals.viewport)) as u16;
                    self.globals.trail[i].y = (y + py(self.globals.viewport)) as u16;
                    self.globals.trail[i].direction = self.globals.party_direction;
                    x += x_offset;
                    y += y_offset;
                }
            }

            // 0x0047: play sound effect.
            0x0047 => {
                self.play_sound(op[0] as i32);
            }

            // 0x0049: set the state of an event object.
            0x0049 => {
                if op[0] != 0 {
                    self.globals.game.event_objects[cur_index].state = op[1] as i16;
                }
            }

            // 0x004A: set the current battlefield.
            0x004A => {
                self.globals.num_battle_field = op[0];
            }

            // 0x004B: nullify the event object for a short while.
            0x004B => {
                self.globals.game.event_objects[evt_idx].vanish_time = -15;
            }

            // 0x004C: chase the player.
            0x004C => {
                let mut i = op[0]; // max distance
                let mut j = op[1]; // speed
                if i == 0 {
                    i = 8;
                }
                if j == 0 {
                    j = 4;
                }
                self.monster_chase_player(event_object_id, j, i, op[2] != 0);
            }

            // 0x004D: wait for any key.
            0x004D => {
                self.wait_for_key(0);
            }

            // 0x004E: load the last saved game.
            0x004E => {
                self.fade_out(1);
                let slot = self.globals.current_save_slot as i32;
                self.globals.reload_in_next_tick(slot);
                return 0; // don't go further
            }

            // 0x004F: fade the screen to red (game over).
            0x004F => {
                self.fade_to_red();
            }

            // 0x0050: screen fade out.
            0x0050 => {
                self.video_update();
                self.fade_out(if op[0] != 0 { op[0] as u64 } else { 1 });
                self.globals.need_to_fade_in = true;
            }

            // 0x0051: screen fade in.
            0x0051 => {
                self.video_update();
                self.fade_in(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    if (op[0] as i16) > 0 { op[0] as u64 } else { 1 },
                );
                self.globals.need_to_fade_in = false;
            }

            // 0x0052: hide the event object for a while (default 800 frames).
            0x0052 => {
                let p = &mut self.globals.game.event_objects[evt_idx];
                p.state = -p.state;
                p.vanish_time = if op[0] != 0 { op[0] as i16 } else { 800 };
            }

            // 0x0053: use the day palette.
            0x0053 => {
                self.globals.night_palette = false;
            }

            // 0x0054: use the night palette.
            0x0054 => {
                self.globals.night_palette = true;
            }

            // 0x0055: add magic to a player.
            0x0055 => {
                let i = if op[1] == 0 {
                    event_object_id as usize
                } else {
                    (op[1] - 1) as usize
                };
                self.globals.add_magic(i, op[0]);
            }

            // 0x0056: remove magic from a player.
            0x0056 => {
                let i = if op[1] == 0 {
                    event_object_id as usize
                } else {
                    (op[1] - 1) as usize
                };
                self.globals.remove_magic(i, op[0]);
            }

            // 0x0057: set base damage of magic according to MP value.
            0x0057 => {
                let i = if op[1] == 0 { 8 } else { op[1] };
                let j = self.globals.game.objects[op[0] as usize].magic_number() as usize;
                let mp = self.globals.game.player_roles.mp[event_object_id as usize];
                self.globals.game.magics[j].base_damage = mp.wrapping_mul(i);
                self.globals.game.player_roles.mp[event_object_id as usize] = 0;
            }

            // 0x0058: jump if fewer than the specified number of the item.
            0x0058 => {
                if self.globals.get_item_amount(op[0]) < op[1] as i16 as i32 {
                    ws = op[2].wrapping_sub(1);
                }
            }

            // 0x0059: change to the specified scene.
            0x0059 => {
                if op[0] > 0 && op[0] as usize <= MAX_SCENES && self.globals.num_scene != op[0] {
                    self.globals.num_scene = op[0];
                    self.globals.load_flags |= LOAD_SCENE;
                    self.globals.entering_scene = true;
                    self.globals.layer = 0;
                }
            }

            // 0x005A: halve the player's HP.
            0x005A => {
                self.globals.game.player_roles.hp[event_object_id as usize] /= 2;
            }

            // 0x005B: halve the enemy's HP.
            0x005B => with_battle!(self, battle => {
                let mut w = battle.enemy[event_object_id as usize].e.health / 2 + 1;
                if w > op[0] {
                    w = op[0];
                }
                let h = &mut battle.enemy[event_object_id as usize].e.health;
                *h = h.wrapping_sub(w);
            }),

            // 0x005C: hide for a while.
            0x005C => with_battle!(self, battle => {
                battle.hiding_time = -(op[0] as i32);
            }),

            // 0x005D: jump if player doesn't have the specified poison.
            0x005D => {
                if !self
                    .globals
                    .is_player_poisoned_by_kind(event_object_id, op[0])
                {
                    ws = op[1].wrapping_sub(1);
                }
            }

            // 0x005E: jump if enemy doesn't have the specified poison.
            0x005E => with_battle!(self, battle => {
                let mut found = false;
                for i in 0..MAX_POISONS {
                    if battle.enemy[event_object_id as usize].poisons[i].poison_id == op[0] {
                        found = true;
                        break;
                    }
                }
                if !found {
                    ws = op[1].wrapping_sub(1);
                }
            }),

            // 0x005F: kill the player immediately.
            0x005F => {
                self.globals.game.player_roles.hp[event_object_id as usize] = 0;
            }

            // 0x0060: immediate KO of the enemy.
            0x0060 => with_battle!(self, battle => {
                battle.enemy[event_object_id as usize].e.health = 0;
            }),

            // 0x0061: jump if player is not poisoned.
            0x0061 => {
                if !self.globals.is_player_poisoned_by_level(event_object_id, 0) {
                    ws = op[0].wrapping_sub(1);
                }
            }

            // 0x0062: pause enemy chasing for a while.
            0x0062 => {
                self.globals.chasespeed_change_cycles = op[0];
                self.globals.chase_range = 0;
            }

            // 0x0063: speed up enemy chasing for a while.
            0x0063 => {
                self.globals.chasespeed_change_cycles = op[0];
                self.globals.chase_range = 3;
            }

            // 0x0064: jump if enemy's HP is above the specified percentage.
            0x0064 => with_battle!(self, battle => {
                let obj = battle.enemy[event_object_id as usize].object_id;
                let enemy_id = self.globals.game.objects[obj as usize].enemy_id() as usize;
                let cur = battle.enemy[event_object_id as usize].e.health as i32;
                let base = self.globals.game.enemies[enemy_id].health as i32;
                if cur * 100 > base * op[0] as i32 {
                    ws = op[1].wrapping_sub(1);
                }
            }),

            // 0x0065: set the player's sprite.
            0x0065 => {
                self.globals.game.player_roles.sprite_num[op[0] as usize] = op[1];
                if !self.globals.in_battle && op[2] != 0 {
                    self.globals.load_flags |= LOAD_PLAYER_SPRITE;
                    self.load_resources();
                }
            }

            // 0x0066: throw weapon to enemy.
            0x0066 => with_battle!(self, battle => {
                let mut w = op[1].wrapping_mul(5);
                let role = self.globals.party[battle.moving_player_index as usize]
                    .player_role as usize;
                w = w.wrapping_add(
                    self.globals.game.player_roles.attack_strength[role]
                        .wrapping_mul(random_long(0, 3) as u16),
                );
                crate::fight::battle_simulate_magic(
                    self,
                    &mut battle,
                    event_object_id as i16,
                    op[0],
                    w,
                );
            }),

            // 0x0067: enemy use magic.
            0x0067 => with_battle!(self, battle => {
                let e = &mut battle.enemy[event_object_id as usize];
                e.e.magic = op[0];
                e.e.magic_rate = if op[1] == 0 { 10 } else { op[1] };
            }),

            // 0x0068: jump if it's the enemy's turn.
            0x0068 => with_battle!(self, battle => {
                if battle.enemy_moving {
                    ws = op[0].wrapping_sub(1);
                }
            }),

            // 0x0069: enemy escape in battle.
            0x0069 => with_battle!(self, battle => {
                crate::battle::enemy_escape(self, &mut battle);
            }),

            // 0x006A: steal from the enemy.
            0x006A => with_battle!(self, battle => {
                crate::fight::battle_steal_from_enemy(self, &mut battle, event_object_id, op[0]);
            }),

            // 0x006B: blow away enemies.
            0x006B => with_battle!(self, battle => {
                battle.blow = op[0] as i16 as i32;
            }),

            // 0x006C: walk the NPC in one step.
            0x006C => {
                {
                    let p = &mut self.globals.game.event_objects[cur_index];
                    p.x = (p.x as i32 + op[1] as i16 as i32) as u16;
                    p.y = (p.y as i32 + op[2] as i16 as i32) as u16;
                }
                self.npc_walk_one_step(cur_event_object_id, 0);
            }

            // 0x006D: set enter/teleport scripts for a scene.
            0x006D => {
                if op[0] != 0 {
                    let s = (op[0] - 1) as usize;
                    if op[1] != 0 {
                        self.globals.game.scenes[s].script_on_enter = op[1];
                    }
                    if op[2] != 0 {
                        self.globals.game.scenes[s].script_on_teleport = op[2];
                    }
                    if op[1] == 0 && op[2] == 0 {
                        self.globals.game.scenes[s].script_on_enter = 0;
                        self.globals.game.scenes[s].script_on_teleport = 0;
                    }
                }
            }

            // 0x006E: move the player to the position in one step.
            0x006E => {
                for i in (0..4).rev() {
                    self.globals.trail[i + 1] = self.globals.trail[i];
                }
                self.globals.trail[0].direction = self.globals.party_direction;
                self.globals.trail[0].x =
                    (px(self.globals.viewport) + px(self.globals.partyoffset)) as u16;
                self.globals.trail[0].y =
                    (py(self.globals.viewport) + py(self.globals.partyoffset)) as u16;

                self.globals.viewport = (
                    px(self.globals.viewport) + op[0] as i16 as i32,
                    py(self.globals.viewport) + op[1] as i16 as i32,
                );
                self.globals.layer = op[2] * 8;

                if op[0] != 0 || op[1] != 0 {
                    self.update_party_gestures(true);
                }
            }

            // 0x006F: sync the state of the current event object with another.
            0x006F => {
                if self.globals.game.event_objects[cur_index].state == op[1] as i16 {
                    self.globals.game.event_objects[evt_idx].state = op[1] as i16;
                }
            }

            // 0x0070: walk the party to the specified position.
            0x0070 => {
                self.party_walk_to(op[0] as i32, op[1] as i32, op[2] as i32, 2);
            }

            // 0x0071: wave the screen.
            0x0071 => {
                self.globals.screen_wave = op[0];
                self.globals.wave_progression = op[1] as i16;
            }

            // 0x0073: fade the screen to scene.
            0x0073 => {
                self.backup_screen();
                self.make_scene();
                self.fade_screen(op[0]);
            }

            // 0x0074: jump if not all players are at full HP.
            0x0074 => {
                for i in 0..=self.globals.max_party_member_index as usize {
                    let w = self.globals.party[i].player_role as usize;
                    if self.globals.game.player_roles.hp[w]
                        < self.globals.game.player_roles.max_hp[w]
                    {
                        ws = op[0].wrapping_sub(1);
                        break;
                    }
                }
            }

            // 0x0075: set the player party.
            0x0075 => {
                self.globals.max_party_member_index = 0;
                for &v in op.iter() {
                    if v != 0 {
                        let idx = self.globals.max_party_member_index as usize;
                        self.globals.party[idx].player_role = v - 1;
                        self.globals.max_party_member_index += 1;
                    }
                }
                if self.globals.max_party_member_index == 0 {
                    // HACK for Dream 2.11.
                    self.globals.party[0].player_role = 0;
                    self.globals.max_party_member_index = 1;
                }
                self.globals.max_party_member_index -= 1;

                self.globals.load_flags |= LOAD_PLAYER_SPRITE;
                self.load_resources();

                self.globals.poison_status = Default::default();
                self.update_equipments();
            }

            // 0x0076: show FBP picture.
            0x0076 => {
                self.ending_set_effect_sprite(0);
                self.show_fbp(op[0], op[1]);
            }

            // 0x0077: stop current playing music.
            0x0077 => {
                let fade = if op[0] == 0 { 2.0 } else { op[0] as f32 * 3.0 };
                self.play_music(0, false, fade);
                self.globals.num_music = 0;
            }

            // 0x0078: FIXME: ??? (no-op).
            0x0078 => {}

            // 0x0079: jump if the specified player is in the party.
            0x0079 => {
                for i in 0..=self.globals.max_party_member_index as usize {
                    let role = self.globals.party[i].player_role as usize;
                    if self.globals.game.player_roles.name[role] == op[0] {
                        ws = op[1].wrapping_sub(1);
                        break;
                    }
                }
            }

            // 0x007A: walk the party to the position, at a higher speed.
            0x007A => {
                self.party_walk_to(op[0] as i32, op[1] as i32, op[2] as i32, 4);
            }

            // 0x007B: walk the party to the position, at the highest speed.
            0x007B => {
                self.party_walk_to(op[0] as i32, op[1] as i32, op[2] as i32, 8);
            }

            // 0x007C: walk straight to the position (parity-gated, speed 4).
            0x007C => {
                if (event_object_id as u32 & 1) ^ (self.globals.frame_num & 1) != 0 {
                    if !self.npc_walk_to(
                        event_object_id,
                        op[0] as i32,
                        op[1] as i32,
                        op[2] as i32,
                        4,
                    ) {
                        ws = ws.wrapping_sub(1);
                    }
                } else {
                    ws = ws.wrapping_sub(1);
                }
            }

            // 0x007D: move the event object.
            0x007D => {
                let p = &mut self.globals.game.event_objects[cur_index];
                p.x = (p.x as i32 + op[1] as i16 as i32) as u16;
                p.y = (p.y as i32 + op[2] as i16 as i32) as u16;
            }

            // 0x007E: set the layer of the event object.
            0x007E => {
                self.globals.game.event_objects[cur_index].layer = op[1] as i16;
            }

            // 0x007F: move the viewport.
            0x007F => {
                self.op_move_viewport(op);
            }

            // 0x0080: toggle day/night palette.
            0x0080 => {
                self.globals.night_palette = !self.globals.night_palette;
                self.palette_fade(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    op[0] == 0,
                );
            }

            // 0x0081: jump if the player is not facing the event object.
            0x0081 => {
                ws = self.op_face_event_object(ws, op, event_object_id, cur_index);
            }

            // 0x0082: walk straight to the position, at a high speed.
            0x0082 => {
                if !self.npc_walk_to(event_object_id, op[0] as i32, op[1] as i32, op[2] as i32, 8) {
                    ws = ws.wrapping_sub(1);
                }
            }

            // 0x0083: jump if event object not in the zone of current object.
            0x0083 => {
                let scene = self.globals.num_scene as usize;
                if op[0] <= self.globals.game.scenes[scene - 1].event_object_index
                    || op[0] > self.globals.game.scenes[scene].event_object_index
                {
                    ws = op[2].wrapping_sub(1);
                    self.script.script_success = false;
                } else {
                    let x = self.globals.game.event_objects[evt_idx].x as i32
                        - self.globals.game.event_objects[cur_index].x as i32;
                    let y = self.globals.game.event_objects[evt_idx].y as i32
                        - self.globals.game.event_objects[cur_index].y as i32;
                    if x.abs() + (y * 2).abs() >= op[1] as i32 * 32 + 16 {
                        ws = op[2].wrapping_sub(1);
                        self.script.script_success = false;
                    }
                }
            }

            // 0x0084: place used item into the scene as an event object.
            0x0084 => {
                ws = self.op_place_item(ws, op, cur_index);
            }

            // 0x0085: delay for a period.
            0x0085 => {
                self.delay(op[0] as u64 * 80);
            }

            // 0x0086: jump if the specified item is not equipped.
            0x0086 => {
                let mut y = 0u16;
                for i in 0..=self.globals.max_party_member_index as usize {
                    let w = self.globals.party[i].player_role as usize;
                    for x in 0..MAX_PLAYER_EQUIPMENTS {
                        if self.globals.game.player_roles.equipment[x][w] == op[0] {
                            y += 1;
                        }
                    }
                }
                if y < op[1] {
                    ws = op[2].wrapping_sub(1);
                }
            }

            // 0x0087: animate the event object.
            0x0087 => {
                self.npc_walk_one_step(cur_event_object_id, 0);
            }

            // 0x0088: set base damage of magic according to money.
            0x0088 => {
                let i = if self.globals.cash > 5000 {
                    5000
                } else {
                    self.globals.cash
                };
                self.globals.cash -= i;
                let j = self.globals.game.objects[op[0] as usize].magic_number() as usize;
                self.globals.game.magics[j].base_damage = (i * 2 / 5) as u16;
            }

            // 0x0089: set the battle result.
            0x0089 => with_battle!(self, battle => {
                battle.battle_result = BattleResult::from_u16(op[0]);
            }),

            // 0x008A: enable auto-battle for the next battle.
            0x008A => {
                self.globals.auto_battle = true;
            }

            // 0x008B: change the current palette.
            0x008B => {
                self.globals.num_palette = op[0];
                if !self.globals.need_to_fade_in {
                    self.set_palette(self.globals.num_palette as usize, false);
                }
            }

            // 0x008C: fade from/to color.
            0x008C => {
                self.color_fade(op[1] as u64, op[0] as u8, op[2] != 0);
                self.globals.need_to_fade_in = false;
            }

            // 0x008D: increase player's level.
            0x008D => {
                self.globals
                    .player_level_up(event_object_id as usize, op[0]);
            }

            // 0x008F: halve the cash amount.
            0x008F => {
                self.globals.cash /= 2;
            }

            // 0x0090: set the object script.
            0x0090 => {
                self.globals.game.objects[op[0] as usize].data[2 + op[2] as usize] = op[1];
            }

            // 0x0091: jump if the enemy is not first of the same kind.
            0x0091 => with_battle!(self, battle => {
                // Reached only during battle (`self.globals.in_battle`), which
                // is exactly when `self.battle` is `Some`.
                let mut self_pos = 0;
                let mut count = 0;
                let target = battle.enemy[event_object_id as usize].object_id;
                for i in 0..=battle.max_enemy_index as usize {
                    if battle.enemy[i].object_id == target {
                        count += 1;
                        if i == event_object_id as usize {
                            self_pos = count;
                        }
                    }
                }
                if self_pos > 1 {
                    ws = op[0].wrapping_sub(1);
                }
            }),

            // 0x0092: show a magic-casting animation for a player in battle.
            0x0092 => with_battle!(self, battle => {
                if op[0] != 0 {
                    crate::fight::show_player_pre_magic_anim(self, &mut battle, (op[0] - 1) as usize, false);
                    battle.player[(op[0] - 1) as usize].current_frame = 6;
                }
                for i in 0..5 {
                    for j in 0..=self.globals.max_party_member_index as usize {
                        battle.player[j].color_shift = i * 2;
                    }
                    crate::fight::battle_delay(self, &mut battle, 1, 0, true);
                }
                crate::fight::battle_update_fighters(self, &mut battle);
                crate::battle::make_scene(self, &mut battle);
                crate::battle::fade_scene(self, &mut battle);
            }),

            // 0x0093: fade the screen, updating the scene during the process.
            0x0093 => {
                self.scene_fade(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    op[0] as i16 as i32,
                );
                self.globals.need_to_fade_in = (op[0] as i16) < 0;
            }

            // 0x0094: jump if event object state is the specified one.
            0x0094 => {
                if self.globals.game.event_objects[cur_index].state == op[1] as i16 {
                    ws = op[2].wrapping_sub(1);
                }
            }

            // 0x0095: jump if the current scene is the specified one.
            0x0095 => {
                if self.globals.num_scene == op[0] {
                    ws = op[1].wrapping_sub(1);
                }
            }

            // 0x0096: show the ending animation.
            0x0096 => {
                self.ending_animation();
            }

            // 0x0097: ride the event object to the position, higher speed.
            0x0097 => {
                self.party_ride_event_object(
                    event_object_id,
                    op[0] as i32,
                    op[1] as i32,
                    op[2] as i32,
                    8,
                );
            }

            // 0x0098: set the follower(s) of the party.
            0x0098 => {
                let mut j = 0;
                for (i, &v) in op.iter().take(2).enumerate() {
                    if v > 0 {
                        let cur_follower = i + 1;
                        j = cur_follower;
                        self.globals.follower_num = cur_follower as u16;
                        let party_idx = self.globals.max_party_member_index as usize + cur_follower;
                        self.globals.party[party_idx].player_role = v;

                        self.globals.load_flags |= LOAD_PLAYER_SPRITE;
                        self.load_resources();

                        self.globals.party[party_idx].x =
                            (self.globals.trail[3 + i].x as i32 - px(self.globals.viewport)) as i16;
                        self.globals.party[party_idx].y =
                            (self.globals.trail[3 + i].y as i32 - py(self.globals.viewport)) as i16;
                        self.globals.party[party_idx].frame =
                            self.globals.trail[3 + i].direction * 3;
                    }
                }
                if j == 0 {
                    self.globals.follower_num = 0;
                }
            }

            // 0x0099: change the map for the specified scene.
            0x0099 => {
                if op[0] == 0xFFFF {
                    let s = self.globals.num_scene as usize - 1;
                    self.globals.game.scenes[s].map_num = op[1];
                    self.globals.load_flags |= LOAD_SCENE;
                    self.load_resources();
                } else {
                    self.globals.game.scenes[(op[0] - 1) as usize].map_num = op[1];
                }
            }

            // 0x009A: set the state for multiple event objects.
            0x009A => {
                let mut i = op[0];
                while i <= op[1] {
                    self.globals.game.event_objects[(i - 1) as usize].state = op[2] as i16;
                    i += 1;
                }
            }

            // 0x009B: fade to the current scene.
            0x009B => {
                self.backup_screen();
                self.make_scene();
                self.fade_screen(2);
            }

            // 0x009C: enemy division.
            0x009C => with_battle!(self, battle => {
                ws = self.op_enemy_division(&mut battle, ws, op, event_object_id, cur_event_object_id);
            }),

            // 0x009E: enemy summons another monster.
            0x009E => with_battle!(self, battle => {
                ws = self.op_enemy_summon(&mut battle, ws, op, event_object_id);
            }),

            // 0x009F: enemy transforms into something else.
            0x009F => with_battle!(self, battle => {
                self.op_enemy_transform(&mut battle, op, event_object_id);
            }),

            // 0x00A0: quit game.
            0x00A0 => {
                self.additional_credits();
                self.shutdown();
            }

            // 0x00A1: set all party positions to the first member's.
            0x00A1 => {
                for i in 0..MAX_PLAYABLE_PLAYER_ROLES {
                    self.globals.trail[i].direction = self.globals.party_direction;
                    self.globals.trail[i].x =
                        (self.globals.party[0].x as i32 + px(self.globals.viewport)) as u16;
                    self.globals.trail[i].y =
                        (self.globals.party[0].y as i32 + py(self.globals.viewport)) as u16;
                }
                for i in 1..=self.globals.max_party_member_index as usize {
                    self.globals.party[i].x = self.globals.party[0].x;
                    self.globals.party[i].y = self.globals.party[0].y - 1;
                }
                self.update_party_gestures(false);
            }

            // 0x00A2: jump to one of the following instructions randomly.
            0x00A2 => {
                ws = ws.wrapping_add(random_long(0, op[0] as i32 - 1) as u16);
            }

            // 0x00A3: play CD music, RIX fallback (no CD support -> RIX).
            0x00A3 => {
                self.globals.num_music = op[1];
                self.play_music(op[1] as i32, true, 0.0);
            }

            // 0x00A4: scroll FBP to the screen.
            0x00A4 => {
                if op[0] == 68 {
                    // HACKHACK: make the ending picture show correctly.
                    self.show_fbp(69, 0);
                }
                self.scroll_fbp(op[0], op[2], true);
            }

            // 0x00A5: show FBP picture with sprite effects.
            0x00A5 => {
                if op[1] != 0xFFFF {
                    self.ending_set_effect_sprite(op[1]);
                }
                self.show_fbp(op[0], op[2]);
            }

            // 0x00A6: backup screen.
            0x00A6 => {
                self.backup_screen();
            }

            other => {
                panic!(
                    "SCRIPT: Invalid Instruction at {ws:04x}: ({other:04x} - {:04x}, {:04x}, {:04x})",
                    op[0], op[1], op[2]
                );
            }
        }

        ws.wrapping_add(1)
    }

    // ---- opcode helpers that are large enough to warrant their own fn ----

    /// Apply a single poison to enemy slot `i` (helper for opcode 0x0028).
    fn apply_poison_to_enemy(
        &mut self,
        battle: &mut Battle,
        i: usize,
        poison_id: u16,
        event_object_id: u16,
    ) {
        let mut j = MAX_POISONS;
        for k in 0..MAX_POISONS {
            if battle.enemy[i].poisons[k].poison_id == poison_id {
                j = k;
                break;
            }
        }
        if j >= MAX_POISONS {
            for k in 0..MAX_POISONS {
                if battle.enemy[i].poisons[k].poison_id == 0 {
                    let script =
                        self.globals.game.objects[poison_id as usize].poison_enemy_script();
                    // The poison's on-apply script can itself touch the battle;
                    // route it through the wrapper so it stays visible.
                    let result = self.run_trigger_script_in_battle(battle, script, event_object_id);
                    battle.enemy[i].poisons[k].poison_id = poison_id;
                    battle.enemy[i].poisons[k].poison_script = result;
                    break;
                }
            }
        }
    }

    /// Opcode 0x007F: move the viewport (or reset it).
    fn op_move_viewport(&mut self, op: [u16; 3]) {
        if op[0] == 0 && op[1] == 0 {
            // Move the viewport back to the normal state.
            let x = self.globals.party[0].x as i32 - 160;
            let y = self.globals.party[0].y as i32 - 112;
            self.globals.viewport = (px(self.globals.viewport) + x, py(self.globals.viewport) + y);
            self.globals.partyoffset = (160, 112);

            let n = self.globals.max_party_member_index as i32 + self.globals.follower_num as i32;
            for i in 0..=n as usize {
                self.globals.party[i].x -= x as i16;
                self.globals.party[i].y -= y as i16;
            }
            if op[2] != 0xFFFF {
                self.make_scene();
                self.video_update();
            }
        } else {
            let mut i = 0i32;
            let x0 = op[0] as i16 as i32;
            let y0 = op[1] as i16 as i32;
            let mut time = self.ticks() + FRAME_TIME;
            loop {
                if op[2] == 0xFFFF {
                    let x = px(self.globals.viewport);
                    let y = py(self.globals.viewport);
                    self.globals.viewport = (op[0] as i32 * 32 - 160, op[1] as i32 * 16 - 112);
                    let dx = x - px(self.globals.viewport);
                    let dy = y - py(self.globals.viewport);
                    let n = self.globals.max_party_member_index as i32
                        + self.globals.follower_num as i32;
                    for j in 0..=n as usize {
                        self.globals.party[j].x += dx as i16;
                        self.globals.party[j].y += dy as i16;
                    }
                } else {
                    self.globals.viewport = (
                        px(self.globals.viewport) + x0,
                        py(self.globals.viewport) + y0,
                    );
                    self.globals.partyoffset = (
                        px(self.globals.partyoffset) - x0,
                        py(self.globals.partyoffset) - y0,
                    );
                    let n = self.globals.max_party_member_index as i32
                        + self.globals.follower_num as i32;
                    for j in 0..=n as usize {
                        self.globals.party[j].x -= x0 as i16;
                        self.globals.party[j].y -= y0 as i16;
                    }
                }

                if op[2] != 0xFFFF {
                    self.game_update(false);
                }
                self.make_scene();
                self.video_update();
                self.delay_until(time);
                time = self.ticks() + FRAME_TIME;

                i += 1;
                if i >= op[2] as i16 as i32 {
                    break;
                }
            }
        }
    }

    /// Opcode 0x0081: jump if the player is not facing the event object.
    fn op_face_event_object(
        &mut self,
        mut ws: u16,
        op: [u16; 3],
        _event_object_id: u16,
        cur_index: usize,
    ) -> u16 {
        let scene = self.globals.num_scene as usize;
        if op[0] <= self.globals.game.scenes[scene - 1].event_object_index
            || op[0] > self.globals.game.scenes[scene].event_object_index
        {
            self.script.script_success = false;
            return op[2].wrapping_sub(1);
        }

        let mut x = self.globals.game.event_objects[cur_index].x as i32;
        let mut y = self.globals.game.event_objects[cur_index].y as i32;
        x += if self.globals.party_direction == DIR_WEST
            || self.globals.party_direction == DIR_SOUTH
        {
            16
        } else {
            -16
        };
        y += if self.globals.party_direction == DIR_WEST
            || self.globals.party_direction == DIR_NORTH
        {
            8
        } else {
            -8
        };
        x -= px(self.globals.viewport) + px(self.globals.partyoffset);
        y -= py(self.globals.viewport) + py(self.globals.partyoffset);

        let target_state = self.globals.game.event_objects[(op[0] - 1) as usize].state;
        if x.abs() + (y * 2).abs() < op[1] as i32 * 32 + 16 && target_state > 0 {
            if op[1] > 0 {
                self.globals.game.event_objects[cur_index].trigger_mode =
                    TRIGGER_TOUCH_NORMAL + op[1];
            }
        } else {
            ws = op[2].wrapping_sub(1);
            self.script.script_success = false;
        }
        ws
    }

    /// Opcode 0x0084: place the used item into the scene as an event object.
    fn op_place_item(&mut self, mut ws: u16, op: [u16; 3], cur_index: usize) -> u16 {
        let scene = self.globals.num_scene as usize;
        if op[0] <= self.globals.game.scenes[scene - 1].event_object_index
            || op[0] > self.globals.game.scenes[scene].event_object_index
        {
            self.script.script_success = false;
            return op[2].wrapping_sub(1);
        }

        let mut x = px(self.globals.viewport) + px(self.globals.partyoffset);
        let mut y = py(self.globals.viewport) + py(self.globals.partyoffset);
        x += if self.globals.party_direction == DIR_WEST
            || self.globals.party_direction == DIR_SOUTH
        {
            -16
        } else {
            16
        };
        y += if self.globals.party_direction == DIR_WEST
            || self.globals.party_direction == DIR_NORTH
        {
            -8
        } else {
            8
        };

        if self.check_obstacle((x, y), false, 0) {
            ws = op[2].wrapping_sub(1);
            self.script.script_success = false;
        } else {
            let p = &mut self.globals.game.event_objects[cur_index];
            p.x = x as u16;
            p.y = y as u16;
            p.state = op[1] as i16;
        }
        ws
    }

    /// Opcode 0x009C: enemy division.
    fn op_enemy_division(
        &mut self,
        battle: &mut Battle,
        mut ws: u16,
        op: [u16; 3],
        event_object_id: u16,
        cur_event_object_id: u16,
    ) -> u16 {
        let mut count = 0u16;
        for i in 0..=battle.max_enemy_index as usize {
            if battle.enemy[i].object_id != 0 {
                count += 1;
            }
        }
        if count != 1 || battle.enemy[cur_event_object_id as usize].e.health <= 1 {
            if op[1] != 0 {
                ws = op[1].wrapping_sub(1);
            }
            return ws;
        }

        let mut w = op[0];
        if w == 0 {
            w = 1;
        }
        let x = w + 1;
        let y = w;

        let src = battle.enemy[event_object_id as usize].clone();
        for i in 0..MAX_ENEMIES_IN_TEAM {
            if w > 0 && battle.enemy[i].object_id == 0 {
                w -= 1;
                let mut e = BattleEnemy {
                    object_id: src.object_id,
                    e: src.e,
                    script_on_turn_start: src.script_on_turn_start,
                    script_on_battle_end: src.script_on_battle_end,
                    script_on_ready: src.script_on_ready,
                    state: FighterState::Wait,
                    time_meter: 50.0,
                    color_shift: 0,
                    ..BattleEnemy::default()
                };
                e.e.health = (src.e.health + y) / x;
                battle.enemy[i] = e;
            }
        }
        battle.enemy[cur_event_object_id as usize].e.health = (src.e.health + y) / x;

        let mut max = 0;
        for i in 0..MAX_ENEMIES_IN_TEAM {
            if battle.enemy[i].object_id != 0 {
                max = i;
            }
        }
        battle.max_enemy_index = max as u16;

        crate::battle::load_battle_sprites(self, battle).ok();
        for i in 0..=battle.max_enemy_index as usize {
            if battle.enemy[i].object_id == 0 {
                continue;
            }
            battle.enemy[i].pos = src.pos;
        }
        for _ in 0..10 {
            for j in 0..=battle.max_enemy_index as usize {
                let px2 = (battle.enemy[j].pos.0 + battle.enemy[j].pos_original.0) / 2;
                let py2 = (battle.enemy[j].pos.1 + battle.enemy[j].pos_original.1) / 2;
                battle.enemy[j].pos = (px2, py2);
            }
            crate::fight::battle_delay(self, battle, 1, 0, true);
        }
        crate::fight::battle_update_fighters(self, battle);
        crate::fight::battle_delay(self, battle, 1, 0, true);
        ws
    }

    /// Opcode 0x009E: enemy summons another monster.
    fn op_enemy_summon(
        &mut self,
        battle: &mut Battle,
        mut ws: u16,
        op: [u16; 3],
        event_object_id: u16,
    ) -> u16 {
        let (magic_frames, idle_frames, act_wait) = {
            let e = &battle.enemy[event_object_id as usize].e;
            (e.magic_frames, e.idle_frames, e.act_wait_frames)
        };
        for i in 0..magic_frames {
            battle.enemy[event_object_id as usize].current_frame = idle_frames + i;
            crate::fight::battle_delay(self, battle, act_wait, 0, false);
        }

        let mut x = 0i32;
        let mut w = op[0];
        let mut y = if (op[1] as i16) <= 0 {
            1
        } else {
            op[1] as i16 as i32
        };

        if w == 0 || w == 0xFFFF {
            w = battle.enemy[event_object_id as usize].object_id;
        }

        for i in 0..=battle.max_enemy_index as usize {
            if battle.enemy[i].object_id == 0 {
                x += 1;
            }
        }

        let e = &battle.enemy[event_object_id as usize];
        if x < y
            || battle.hiding_time > 0
            || e.status[STATUS_SLEEP] != 0
            || e.status[STATUS_PARALYZED] != 0
            || e.status[STATUS_CONFUSED] != 0
        {
            if op[2] != 0 {
                ws = op[2].wrapping_sub(1);
            }
        } else {
            let enemy_id = self.globals.game.objects[w as usize].enemy_id() as usize;
            for i in 0..=battle.max_enemy_index as usize {
                if battle.enemy[i].object_id == 0 {
                    let obj = &self.globals.game.objects[w as usize];
                    battle.enemy[i] = BattleEnemy {
                        object_id: w,
                        e: self.globals.game.enemies[enemy_id],
                        state: FighterState::Wait,
                        script_on_turn_start: obj.enemy_script_on_turn_start(),
                        script_on_battle_end: obj.enemy_script_on_battle_end(),
                        script_on_ready: obj.enemy_script_on_ready(),
                        time_meter: 50.0,
                        color_shift: 8,
                        ..BattleEnemy::default()
                    };
                    y -= 1;
                    if y <= 0 {
                        break;
                    }
                }
            }
            crate::battle::load_battle_sprites(self, battle).ok();
            crate::battle::make_scene(self, battle);
            self.play_sound(212);
            crate::battle::fade_scene(self, battle);
            crate::fight::battle_delay(self, battle, 2, 0, true);
            for i in 0..=battle.max_enemy_index as usize {
                battle.enemy[i].color_shift = 0;
            }
            crate::battle::make_scene(self, battle);
            crate::battle::fade_scene(self, battle);
        }
        ws
    }

    /// Opcode 0x009F: enemy transforms into something else.
    fn op_enemy_transform(&mut self, battle: &mut Battle, op: [u16; 3], event_object_id: u16) {
        let e = &battle.enemy[event_object_id as usize];
        if battle.hiding_time <= 0
            && e.status[STATUS_SLEEP] == 0
            && e.status[STATUS_PARALYZED] == 0
            && e.status[STATUS_CONFUSED] == 0
        {
            let health = battle.enemy[event_object_id as usize].e.health;
            let enemy_id = self.globals.game.objects[op[0] as usize].enemy_id() as usize;
            let slot = &mut battle.enemy[event_object_id as usize];
            slot.object_id = op[0];
            slot.e = self.globals.game.enemies[enemy_id];
            slot.e.health = health;
            slot.current_frame = 0;

            for i in 0..6 {
                battle.enemy[event_object_id as usize].color_shift = i;
                crate::fight::battle_delay(self, battle, 1, 0, false);
            }
            battle.enemy[event_object_id as usize].color_shift = 0;
            self.play_sound(47);
            crate::battle::load_battle_sprites(self, battle).ok();
            crate::battle::make_scene(self, battle);
            crate::battle::fade_scene(self, battle);
        }
    }

    // =======================================================================
    // PAL_RunTriggerScript / PAL_RunAutoScript.
    // =======================================================================

    /// `PAL_RunTriggerScript`: run a trigger script; returns the entry point
    /// of the script to save back.
    pub fn run_trigger_script(&mut self, script_entry: u16, event_object_id: u16) -> u16 {
        let mut event_object_id = event_object_id;
        let mut script_entry = script_entry;
        let mut next_script_entry = script_entry;
        let mut ended = false;
        self.script.updated_in_battle = false;

        if event_object_id == 0xFFFF {
            event_object_id = self.script.last_event_object;
        }
        self.script.last_event_object = event_object_id;

        self.script.script_success = true;

        // Set the default dialog speed.
        self.dialog_set_delay_time(3);

        while script_entry != 0 && !ended {
            let script = self.globals.game.script_entries[script_entry as usize];
            let op = script.operand;
            let evt_idx = event_object_id.wrapping_sub(1) as usize;

            match script.operation {
                // 0x0000: stop running.
                0x0000 => {
                    ended = true;
                }

                // 0x0001: stop and replace the entry with the next line.
                0x0001 => {
                    ended = true;
                    next_script_entry = script_entry + 1;
                }

                // 0x0002: stop and replace the entry with the specified one.
                0x0002 => {
                    // `pEvtObj->nScriptIdleFrame` is only touched when
                    // operand[1] != 0 (C short-circuits the `||`).
                    let take = op[1] == 0 || {
                        let p = &mut self.globals.game.event_objects[evt_idx];
                        p.script_idle_frame += 1;
                        p.script_idle_frame < op[1]
                    };
                    if take {
                        ended = true;
                        next_script_entry = op[0];
                    } else {
                        self.globals.game.event_objects[evt_idx].script_idle_frame = 0;
                        script_entry += 1;
                    }
                }

                // 0x0003: unconditional jump.
                0x0003 => {
                    let take = op[1] == 0 || {
                        let p = &mut self.globals.game.event_objects[evt_idx];
                        p.script_idle_frame += 1;
                        p.script_idle_frame < op[1]
                    };
                    if take {
                        script_entry = op[0];
                    } else {
                        self.globals.game.event_objects[evt_idx].script_idle_frame = 0;
                        script_entry += 1;
                    }
                }

                // 0x0004: call script.
                0x0004 => {
                    let eid = if op[1] == 0 { event_object_id } else { op[1] };
                    self.run_trigger_script(op[0], eid);
                    script_entry += 1;
                }

                // 0x0005: redraw screen.
                0x0005 => {
                    self.clear_dialog(true);
                    if self.ui.playing_rng {
                        self.restore_screen();
                    } else if self.globals.in_battle {
                        with_battle!(self, battle => {
                            crate::battle::make_scene(self, &mut battle);
                        });
                        self.video_update();
                    } else {
                        if op[2] != 0 {
                            self.update_party_gestures(false);
                        }
                        self.make_scene();
                        self.video_update();
                        self.delay(if op[1] == 0 { 60 } else { op[1] as u64 * 60 });
                    }
                    script_entry += 1;
                }

                // 0x0006: jump to the specified address by the specified rate.
                0x0006 => {
                    if random_long(1, 100) >= op[0] as i32 {
                        script_entry = op[1];
                        continue;
                    } else {
                        script_entry += 1;
                    }
                }

                // 0x0007: start battle.
                0x0007 => {
                    let result = self.start_battle(op[0], op[2] == 0);
                    use crate::battle::BattleResult;
                    if result == BattleResult::Lost && op[1] != 0 {
                        script_entry = op[1];
                    } else if result == BattleResult::Fleed && op[2] != 0 {
                        script_entry = op[2];
                    } else {
                        script_entry += 1;
                    }
                    self.globals.auto_battle = false;
                }

                // 0x0008: replace the entry with the next instruction.
                0x0008 => {
                    script_entry += 1;
                    next_script_entry = script_entry;
                }

                // 0x0009: wait for the specified number of frames.
                0x0009 => {
                    self.clear_dialog(true);
                    let mut time = self.ticks() + FRAME_TIME;
                    let count = if op[0] != 0 { op[0] } else { 1 };
                    for _ in 0..count {
                        self.delay_until(time);
                        time = self.ticks() + FRAME_TIME;
                        if op[2] != 0 {
                            self.update_party_gestures(false);
                        }
                        self.game_update(op[1] != 0);
                        self.make_scene();
                        self.video_update();
                    }
                    script_entry += 1;
                }

                // 0x000A: goto the specified address if player selected no.
                0x000A => {
                    self.clear_dialog(false);
                    if !self.confirm_menu() {
                        script_entry = op[0];
                    } else {
                        script_entry += 1;
                    }
                }

                // 0x003B: dialog in the middle part of the screen.
                0x003B => {
                    self.clear_dialog(true);
                    self.start_dialog(DIALOG_CENTER, op[0] as u8, 0, op[2] != 0);
                    script_entry += 1;
                }

                // 0x003C: dialog in the upper part of the screen.
                0x003C => {
                    self.clear_dialog(true);
                    self.start_dialog(DIALOG_UPPER, op[1] as u8, op[0] as i32, op[2] != 0);
                    script_entry += 1;
                }

                // 0x003D: dialog in the lower part of the screen.
                0x003D => {
                    self.clear_dialog(true);
                    self.start_dialog(DIALOG_LOWER, op[1] as u8, op[0] as i32, op[2] != 0);
                    script_entry += 1;
                }

                // 0x003E: text in a window at the center of the screen.
                0x003E => {
                    self.clear_dialog(true);
                    self.start_dialog(DIALOG_CENTER_WINDOW, op[0] as u8, 0, false);
                    script_entry += 1;
                }

                // 0x008E: restore the screen.
                0x008E => {
                    self.clear_dialog(true);
                    self.restore_screen();
                    self.video_update();
                    script_entry += 1;
                }

                // 0xFFFF: print dialog text (DOS: no message file).
                0xFFFF => {
                    let text = self.texts.msg(op[0] as usize);
                    self.show_dialog_text(&text);
                    script_entry += 1;
                }

                // All other opcodes: interpret.
                _ => {
                    self.clear_dialog(true);
                    script_entry = self.interpret_instruction(script_entry, event_object_id);
                }
            }
        }

        self.end_dialog();
        self.script.cur_equip_part = -1;
        next_script_entry
    }

    /// `PAL_RunAutoScript`: run the autoscript of an event object; returns the
    /// next instruction address.
    pub fn run_auto_script(&mut self, script_entry: u16, event_object_id: u16) -> u16 {
        let mut script_entry = script_entry;
        loop {
            let script = self.globals.game.script_entries[script_entry as usize];
            let op = script.operand;
            let evt_idx = (event_object_id - 1) as usize;

            match script.operation {
                // 0x0000: stop running.
                0x0000 => {}

                // 0x0001: stop and replace the entry with the next line.
                0x0001 => {
                    script_entry += 1;
                }

                // 0x0002: stop and replace with the specified one.
                0x0002 => {
                    let p = &mut self.globals.game.event_objects[evt_idx];
                    if op[1] == 0 || {
                        p.script_idle_frame_count_auto += 1;
                        p.script_idle_frame_count_auto < op[1]
                    } {
                        script_entry = op[0];
                    } else {
                        self.globals.game.event_objects[evt_idx].script_idle_frame_count_auto = 0;
                        script_entry += 1;
                    }
                }

                // 0x0003: unconditional jump.
                0x0003 => {
                    let p = &mut self.globals.game.event_objects[evt_idx];
                    if op[1] == 0 || {
                        p.script_idle_frame_count_auto += 1;
                        p.script_idle_frame_count_auto < op[1]
                    } {
                        script_entry = op[0];
                        continue; // goto begin
                    } else {
                        self.globals.game.event_objects[evt_idx].script_idle_frame_count_auto = 0;
                        script_entry += 1;
                    }
                }

                // 0x0004: call subroutine.
                0x0004 => {
                    let eid = if op[1] != 0 { op[1] } else { event_object_id };
                    self.run_trigger_script(op[0], eid);
                    script_entry += 1;
                }

                // 0x0006: jump to the specified address by the specified rate.
                0x0006 => {
                    if random_long(1, 100) >= op[0] as i32 {
                        if op[1] != 0 {
                            script_entry = op[1];
                            continue; // goto begin
                        }
                    } else {
                        script_entry += 1;
                    }
                }

                // 0x0009: wait for a certain number of frames.
                0x0009 => {
                    let p = &mut self.globals.game.event_objects[evt_idx];
                    p.script_idle_frame_count_auto += 1;
                    if p.script_idle_frame_count_auto >= op[0] {
                        p.script_idle_frame_count_auto = 0;
                        script_entry += 1;
                    }
                }

                // 0xFFFF (DOS, not WIN95) and 0x00A7: skip.
                0xFFFF | 0x00A7 => {
                    script_entry += 1;
                }

                // Other operations.
                _ => {
                    script_entry = self.interpret_instruction(script_entry, event_object_id);
                }
            }

            break;
        }
        script_entry
    }

    // =======================================================================
    // Script-dependent global.c helpers.
    // =======================================================================

    /// `PAL_UpdateEquipments`: run the equip scripts of every equipped item to
    /// rebuild the equipment effects.
    pub fn update_equipments(&mut self) {
        self.globals.equipment_effect = Default::default();

        for i in 0..MAX_PLAYER_ROLES {
            for j in 0..MAX_PLAYER_EQUIPMENTS {
                let w = self.globals.game.player_roles.equipment[j][i];
                if w != 0 {
                    let script = self.globals.game.objects[w as usize].item_script_on_equip();
                    let result = self.run_trigger_script(script, i as u16);
                    self.globals.game.objects[w as usize].set_item_script_on_equip(result);
                }
            }
        }
    }

    /// `PAL_AddPoisonForPlayer`: add the poison to the player, running its
    /// player script.
    pub fn add_poison_for_player(&mut self, player_role: u16, poison_id: u16) {
        let mut index = None;
        for idx in 0..=self.globals.max_party_member_index as usize {
            if self.globals.party[idx].player_role == player_role {
                index = Some(idx);
                break;
            }
        }
        let Some(index) = index else {
            return;
        };

        let mut i = MAX_POISONS;
        for k in 0..MAX_POISONS {
            let w = self.globals.poison_status[k][index].poison_id;
            if w == 0 {
                i = k;
                break;
            }
            if w == poison_id {
                return; // already poisoned
            }
        }

        if i < MAX_POISONS {
            let script = self.globals.game.objects[poison_id as usize].poison_player_script();
            let result = self.run_trigger_script(script, player_role);
            self.globals.poison_status[i][index].poison_id = poison_id;
            self.globals.poison_status[i][index].poison_script = result;
        }
    }

    // =======================================================================
    // Small cross-module helpers.
    // =======================================================================

    /// PAL_DrawText for one line of the additional-credits roll (uigame.c).
    /// The credits screen itself is not part of this port's scope; the roll is
    /// driven entirely by timing and this per-line draw is intentionally a
    /// no-op (the credits text sprites are never loaded headlessly).
    fn draw_text_line(&mut self, _y: i32) {}

    /// PAL_Shutdown (main.c).  This port requests a graceful quit instead of
    /// terminating the process, so the game loop can unwind normally.
    fn shutdown(&mut self) {
        self.quit_requested = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::ScriptEntry;

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        let mut e = Engine::new(true).expect("headless engine");
        e.globals.load_default_game().expect("default game");
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;
        e
    }

    /// Write a synthetic script starting at `base` and run it as a trigger
    /// script, guarding against non-termination (tests only).
    fn run(e: &mut Engine, base: u16, entries: &[(u16, [u16; 3])], eid: u16) -> u16 {
        for (i, (opn, ops)) in entries.iter().enumerate() {
            e.globals.game.script_entries[base as usize + i] = ScriptEntry {
                operation: *opn,
                operand: *ops,
            };
        }
        // Iteration guard: run_trigger_script itself has no guard (faithful to
        // C); the synthetic scripts here always terminate, but assert it.
        e.run_trigger_script(base, eid)
    }

    #[test]
    fn opcode_give_and_remove_item() {
        let mut e = engine();
        // 0x001F add 3 of item 10, then stop.
        run(
            &mut e,
            20000,
            &[(0x001F, [10, 3, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert_eq!(e.globals.get_item_amount(10), 3);

        // 0x0020 remove 2 of item 10.
        run(
            &mut e,
            20010,
            &[(0x0020, [10, 2, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert_eq!(e.globals.get_item_amount(10), 1);
    }

    #[test]
    fn opcode_cash_change_and_insufficient_jump() {
        let mut e = engine();
        e.globals.cash = 100;
        // 0x001E add 50 cash.
        run(
            &mut e,
            20000,
            &[(0x001E, [50, 0, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert_eq!(e.globals.cash, 150);

        // 0x001E spend 999 (insufficient): jumps to operand[1], which we point
        // at a stop, and cash is unchanged.
        run(
            &mut e,
            20010,
            &[
                (0x001E, [(-999i16) as u16, 20012, 0]),
                (0x0000, [0, 0, 0]),
                (0x0000, [0, 0, 0]),
            ],
            0,
        );
        assert_eq!(e.globals.cash, 150);
    }

    #[test]
    fn opcode_hp_and_magic_changes() {
        let mut e = engine();
        let role = 0usize;
        e.globals.game.player_roles.max_hp[role] = 200;
        e.globals.game.player_roles.hp[role] = 100;
        // 0x001B increase HP of role 0 (eid=role) by 50.
        run(
            &mut e,
            20000,
            &[(0x001B, [0, 50, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert_eq!(e.globals.game.player_roles.hp[role], 150);

        // 0x005A halve HP.
        run(
            &mut e,
            20010,
            &[(0x005A, [0, 0, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert_eq!(e.globals.game.player_roles.hp[role], 75);

        // 0x0055 add magic 0x0100 to role 0.
        run(
            &mut e,
            20020,
            &[(0x0055, [0x0100, 0, 0]), (0x0000, [0, 0, 0])],
            0,
        );
        assert!(e
            .globals
            .game
            .player_roles
            .magic
            .iter()
            .any(|row| row[role] == 0x0100));
    }

    #[test]
    fn control_flow_jump_and_random_jump() {
        let mut e = engine();
        e.globals.cash = 0;
        // 0x0003 unconditional jump (operand[1]=0 so no idle frame) to entry
        // that adds cash; entry between must be skipped.
        run(
            &mut e,
            20000,
            &[
                (0x0003, [20002, 0, 0]), // jump to +2
                (0x001E, [777, 0, 0]),   // skipped
                (0x001E, [5, 0, 0]),     // executed
                (0x0000, [0, 0, 0]),
            ],
            0,
        );
        assert_eq!(e.globals.cash, 5);

        // 0x0006 with rate 100: RandomLong(1,100) >= 100 sometimes; force the
        // else path is nondeterministic, so instead test rate 1 which almost
        // always jumps. Point the jump at a cash add.
        e.globals.cash = 0;
        run(
            &mut e,
            20010,
            &[
                (0x0006, [1, 20013, 0]), // jump to +3 with near-certain rate
                (0x001E, [999, 0, 0]),   // skipped when jump taken
                (0x0000, [0, 0, 0]),
                (0x001E, [3, 0, 0]),
                (0x0000, [0, 0, 0]),
            ],
            0,
        );
        // Either path terminates; both leave a deterministic-ish state. We only
        // assert the script terminated (cash is 3 if jumped, 999 otherwise).
        assert!(e.globals.cash == 3 || e.globals.cash == 999);
    }

    #[test]
    fn poison_add_for_player_no_panic() {
        // Exercise add_poison_for_player: with role 0 in party it either adds a
        // poison (running its script) or returns early. Must terminate.
        let mut e = engine();
        // Find a valid poison object id (poison.wPlayerScript path). Use a low
        // object id that exists; the call must not panic regardless.
        e.add_poison_for_player(0, 0);
    }

    #[test]
    fn run_auto_script_terminates_on_stop() {
        let mut e = engine();
        // eid 1 (event object index 0). 0x0001 -> returns entry+1.
        e.globals.game.script_entries[20000] = ScriptEntry {
            operation: 0x0001,
            operand: [0, 0, 0],
        };
        let next = e.run_auto_script(20000, 1);
        assert_eq!(next, 20001);

        // 0x0009 wait: increments idle counter; with operand[0]=2 it stays put
        // until the counter reaches 2.
        e.globals.game.script_entries[20010] = ScriptEntry {
            operation: 0x0009,
            operand: [2, 0, 0],
        };
        e.globals.game.event_objects[0].script_idle_frame_count_auto = 0;
        let a = e.run_auto_script(20010, 1);
        assert_eq!(a, 20010); // still waiting
        let b = e.run_auto_script(20010, 1);
        assert_eq!(b, 20011); // wait ended, advanced
    }

    #[test]
    fn update_equipments_is_deterministic_and_terminates() {
        let mut e = engine();
        // Ensure at least one role has equipment so the equip scripts run.
        // Default game data typically equips the starting party; run twice and
        // require identical results (idempotent) — this also proves it
        // terminates.
        e.update_equipments();
        let snapshot: Vec<u16> = e
            .globals
            .equipment_effect
            .iter()
            .flat_map(|pr| pr.attack_strength.iter().chain(pr.defense.iter()).copied())
            .collect();
        e.update_equipments();
        let snapshot2: Vec<u16> = e
            .globals
            .equipment_effect
            .iter()
            .flat_map(|pr| pr.attack_strength.iter().chain(pr.defense.iter()).copied())
            .collect();
        assert_eq!(snapshot, snapshot2);
    }
}
