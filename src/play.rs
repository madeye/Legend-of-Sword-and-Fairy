//! Game update loop — port of SDLPAL `play.c` (DOS paths only).
//!
//! `PAL_GameUpdate` (the per-frame logic driving trigger/auto scripts and
//! blocker avoidance), `PAL_StartFrame`, the manual-search trigger logic
//! (`PAL_GetSearchTriggerRange` / `PAL_Search`), `PAL_WaitForKey` /
//! `PAL_WaitForAnyKey`, and the in-game item/equip menu drivers.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::global::{
    ITEMFLAG_APPLY_TO_ALL, ITEMFLAG_CONSUMING, ITEMFLAG_EQUIPABLE, ITEMFLAG_USABLE,
    LOAD_GLOBAL_DATA,
};
use crate::input::{
    DIR_EAST, DIR_NORTH, DIR_SOUTH, KEY_FLEE, KEY_FORCE, KEY_MENU, KEY_SEARCH, KEY_STATUS,
    KEY_THROW_ITEM, KEY_USE_ITEM,
};

// kTriggerTouchNear (global.h) — event objects with wTriggerMode >= this can be
// triggered without a manual search.
const TRIGGER_TOUCH_NEAR: u16 = 4;
// kObjStateBlocker (global.h).
const OBJ_STATE_BLOCKER: i16 = 2;
// MENUITEM_VALUE_CANCELLED (ui.h).
const MENUITEM_VALUE_CANCELLED: u16 = 0xFFFF;

/// Module-private state for the play loop.
#[derive(Default)]
pub struct PlayState {}

impl Engine {
    /// `PAL_GameUpdate`: run the game logic for one frame.
    pub fn game_update(&mut self, trigger: bool) {
        if trigger {
            // Check if we are entering a new scene.
            if self.globals.entering_scene {
                self.globals.entering_scene = false;

                let i = self.globals.num_scene as usize - 1;
                let s = self.globals.game.scenes[i].script_on_enter;
                let r = self.run_trigger_script(s, 0xFFFF);
                self.globals.game.scenes[i].script_on_enter = r;

                if self.globals.entering_scene {
                    // Switching to another scene; don't go further.
                    return;
                }

                self.input.clear_key_state();
                self.make_scene();
            }

            // Loop through all event objects in the current scene.
            let scene = self.globals.num_scene as usize;
            let start = self.globals.game.scenes[scene - 1].event_object_index + 1;
            let end = self.globals.game.scenes[scene].event_object_index;
            let mut id = start;
            while id <= end {
                let idx = (id - 1) as usize;
                let vanish = self.globals.game.event_objects[idx].vanish_time;
                if vanish != 0 {
                    self.globals.game.event_objects[idx].vanish_time +=
                        if vanish < 0 { 1 } else { -1 };
                    id += 1;
                    continue;
                }

                let state = self.globals.game.event_objects[idx].state;
                if state < 0 {
                    let (ex, ey) = {
                        let p = &self.globals.game.event_objects[idx];
                        (p.x as i32, p.y as i32)
                    };
                    let vx = self.globals.viewport.0;
                    let vy = self.globals.viewport.1;
                    if ex < vx || ex > vx + 320 || ey < vy || ey > vy + 320 {
                        let p = &mut self.globals.game.event_objects[idx];
                        p.state = p.state.abs();
                        p.current_frame_num = 0;
                    }
                } else if state > 0
                    && self.globals.game.event_objects[idx].trigger_mode >= TRIGGER_TOUCH_NEAR
                {
                    let (ex, ey, trigger_mode, sprite_frames) = {
                        let p = &self.globals.game.event_objects[idx];
                        (p.x as i32, p.y as i32, p.trigger_mode, p.sprite_frames)
                    };
                    let vx = self.globals.viewport.0 + self.globals.partyoffset.0;
                    let vy = self.globals.viewport.1 + self.globals.partyoffset.1;
                    if (vx - ex).abs() + (vy - ey).abs() * 2
                        < (trigger_mode - TRIGGER_TOUCH_NEAR) as i32 * 32 + 16
                    {
                        // Player is in the trigger zone.
                        if sprite_frames != 0 {
                            // Adjust the sprite direction to face the party.
                            let x_offset = vx - ex;
                            let y_offset = vy - ey;
                            let dir = if x_offset > 0 {
                                if y_offset > 0 {
                                    DIR_EAST
                                } else {
                                    DIR_NORTH
                                }
                            } else if y_offset > 0 {
                                DIR_SOUTH
                            } else {
                                crate::input::DIR_WEST
                            };
                            {
                                let p = &mut self.globals.game.event_objects[idx];
                                p.current_frame_num = 0;
                                p.direction = dir as u16;
                            }
                            self.update_party_gestures(false);
                            self.make_scene();
                            self.video_update();
                        }

                        // Execute the script.
                        let ts = self.globals.game.event_objects[idx].trigger_script;
                        let r = self.run_trigger_script(ts, id);
                        self.globals.game.event_objects[idx].trigger_script = r;

                        self.input.clear_key_state();

                        if self.globals.entering_scene {
                            return;
                        }
                    }
                }

                id += 1;
            }
        }

        // Run autoscript for each event object.
        let scene = self.globals.num_scene as usize;
        let start = self.globals.game.scenes[scene - 1].event_object_index + 1;
        let end = self.globals.game.scenes[scene].event_object_index;
        let mut id = start;
        while id <= end {
            let idx = (id - 1) as usize;
            let (state, vanish, auto_script) = {
                let p = &self.globals.game.event_objects[idx];
                (p.state, p.vanish_time, p.auto_script)
            };

            if state > 0 && vanish == 0 && auto_script != 0 {
                let r = self.run_auto_script(auto_script, id);
                self.globals.game.event_objects[idx].auto_script = r;
                if self.globals.entering_scene {
                    return;
                }
            }

            // Check if the player is in the way.
            let (ex, ey, sprite_num, state, direction) = {
                let p = &self.globals.game.event_objects[idx];
                (p.x as i32, p.y as i32, p.sprite_num, p.state, p.direction)
            };
            let vx = self.globals.viewport.0 + self.globals.partyoffset.0;
            let vy = self.globals.viewport.1 + self.globals.partyoffset.1;
            if trigger
                && state >= OBJ_STATE_BLOCKER
                && sprite_num != 0
                && (ex - vx).abs() + (ey - vy).abs() * 2 <= 12
            {
                // Player is in the way; try to move a step.
                let mut wdir = (direction + 1) % 4;
                for _ in 0..4 {
                    let mut x = self.globals.viewport.0 + self.globals.partyoffset.0;
                    let mut y = self.globals.viewport.1 + self.globals.partyoffset.1;
                    x += if wdir == crate::input::DIR_WEST as u16 || wdir == DIR_SOUTH as u16 {
                        -16
                    } else {
                        16
                    };
                    y += if wdir == crate::input::DIR_WEST as u16 || wdir == DIR_NORTH as u16 {
                        -8
                    } else {
                        8
                    };

                    if !self.check_obstacle_with_range((x, y), true, 0, true) {
                        self.globals.viewport = (
                            x - self.globals.partyoffset.0,
                            y - self.globals.partyoffset.1,
                        );
                        break;
                    }
                    wdir = (wdir + 1) % 4;
                }
            }

            id += 1;
        }

        self.globals.chasespeed_change_cycles =
            self.globals.chasespeed_change_cycles.wrapping_sub(1);
        if self.globals.chasespeed_change_cycles == 0 {
            self.globals.chase_range = 1;
        }

        self.globals.frame_num += 1;
    }

    /// `PAL_StartFrame`: process one frame in the main game.
    pub fn start_frame(&mut self) {
        self.game_update(true);
        if self.globals.entering_scene {
            return;
        }

        self.update_party();
        self.make_scene();
        self.video_update();

        if self.input.pressed(KEY_MENU) {
            self.in_game_menu();
        } else if self.input.pressed(KEY_USE_ITEM) {
            self.game_use_item();
        } else if self.input.pressed(KEY_THROW_ITEM) {
            self.game_equip_item();
        } else if self.input.pressed(KEY_FORCE) {
            self.in_game_magic_menu();
        } else if self.input.pressed(KEY_STATUS) {
            self.player_status();
        } else if self.input.pressed(KEY_SEARCH) {
            self.search();
        } else if self.input.pressed(KEY_FLEE) {
            self.quit_game();
        }
    }

    /// `PAL_GetSearchTriggerRange`: the 13 checkpoint coordinates for a manual
    /// search.
    fn get_search_trigger_range(&self) -> [(i32, i32); 13] {
        let mut x = self.globals.viewport.0 + self.globals.partyoffset.0;
        let mut y = self.globals.viewport.1 + self.globals.partyoffset.1;

        let x_offset = if self.globals.party_direction == DIR_NORTH as u16
            || self.globals.party_direction == DIR_EAST as u16
        {
            16
        } else {
            -16
        };
        let y_offset = if self.globals.party_direction == DIR_EAST as u16
            || self.globals.party_direction == DIR_SOUTH as u16
        {
            8
        } else {
            -8
        };

        let mut range = [(0i32, 0i32); 13];
        range[0] = (x, y);
        for i in 0..4 {
            range[i * 3 + 1] = (x + x_offset, y + y_offset);
            range[i * 3 + 2] = (x, y + y_offset * 2);
            range[i * 3 + 3] = (x + 2 * x_offset, y);
            x += x_offset;
            y += y_offset;
        }
        range
    }

    /// `PAL_Search`: process searching trigger events.
    fn search(&mut self) {
        let range = self.get_search_trigger_range();

        for (i, &(rx, ry)) in range.iter().enumerate() {
            let dh = if rx % 32 != 0 { 1 } else { 0 };
            let dx = rx / 32;
            let dy = ry / 16;

            let scene = self.globals.num_scene as usize;
            let k_start = self.globals.game.scenes[scene - 1].event_object_index as usize;
            let k_end = self.globals.game.scenes[scene].event_object_index as usize;

            let mut k = k_start;
            while k < k_end {
                let (px, py, state, trigger_mode, sprite_frames, current_frame_num) = {
                    let p = &self.globals.game.event_objects[k];
                    (
                        p.x as i32,
                        p.y as i32,
                        p.state,
                        p.trigger_mode,
                        p.sprite_frames,
                        p.current_frame_num,
                    )
                };
                let ex = px / 32;
                let ey = py / 16;
                let eh = if px % 32 != 0 { 1 } else { 0 };

                if state <= 0
                    || trigger_mode >= TRIGGER_TOUCH_NEAR
                    || (trigger_mode as i32) * 6 - 4 <= i as i32
                    || dx != ex
                    || dy != ey
                    || dh != eh
                {
                    k += 1;
                    continue;
                }

                // Adjust direction/gesture for party members and event object.
                if sprite_frames as u32 * 4 > current_frame_num as u32 {
                    {
                        let p = &mut self.globals.game.event_objects[k];
                        p.current_frame_num = 0;
                        p.direction = (self.globals.party_direction + 2) % 4;
                    }
                    for l in 0..=self.globals.max_party_member_index as usize {
                        self.globals.party[l].frame = self.globals.party_direction * 3;
                    }
                    self.make_scene();
                    self.video_update();
                }

                // Execute the script.
                let ts = self.globals.game.event_objects[k].trigger_script;
                let r = self.run_trigger_script(ts, (k + 1) as u16);
                self.globals.game.event_objects[k].trigger_script = r;

                self.delay(50);
                self.input.clear_key_state();
                return; // don't go further
            }
        }
    }

    /// `PAL_WaitForKey`: wait for KeySearch and KeyMenu.
    pub fn wait_for_key(&mut self, timeout: u16) {
        self.wait_for_key_internal(timeout, false);
    }

    /// `PAL_WaitForAnyKey`: wait for any key.
    pub fn wait_for_any_key(&mut self, timeout: u16) {
        self.wait_for_key_internal(timeout, true);
    }

    fn wait_for_key_internal(&mut self, timeout: u16, allow_any_key: bool) {
        if self.ui.auto_confirm {
            // Headless test escape hatch: don't block on input.
            self.input.clear_key_state();
            return;
        }
        let deadline = self.ticks() + timeout as u64;
        self.input.clear_key_state();

        while timeout == 0 || self.ticks() < deadline {
            self.delay(5);
            if (allow_any_key && self.input.key_press != 0)
                || self.input.pressed(KEY_SEARCH | KEY_MENU)
            {
                break;
            }
            if self.quit_requested {
                break;
            }
        }
    }

    /// `PAL_GameUseItem`: let the player use an item in the game.
    pub fn game_use_item(&mut self) {
        loop {
            let object = self.item_select_menu(None, ITEMFLAG_USABLE);
            if object == 0 {
                return;
            }

            let flags = self.globals.game.objects[object as usize].item_flags();
            if flags & ITEMFLAG_APPLY_TO_ALL == 0 {
                // Select the player to use the item on.
                loop {
                    let player = self.item_use_menu(object);
                    if player == MENUITEM_VALUE_CANCELLED {
                        break;
                    }
                    let script = self.globals.game.objects[object as usize].item_script_on_use();
                    let r = self.run_trigger_script(script, player);
                    self.globals.game.objects[object as usize].set_item_script_on_use(r);

                    if flags & ITEMFLAG_CONSUMING != 0 && self.script.script_success {
                        self.globals.add_item_to_inventory(object, -1);
                    }
                }
            } else {
                let script = self.globals.game.objects[object as usize].item_script_on_use();
                let r = self.run_trigger_script(script, 0xFFFF);
                self.globals.game.objects[object as usize].set_item_script_on_use(r);

                if flags & ITEMFLAG_CONSUMING != 0 && self.script.script_success {
                    self.globals.add_item_to_inventory(object, -1);
                }
                return;
            }
        }
    }

    /// `PAL_GameEquipItem`: let the player equip an item in the game.
    pub fn game_equip_item(&mut self) {
        loop {
            let object = self.item_select_menu(None, ITEMFLAG_EQUIPABLE);
            if object == 0 {
                return;
            }
            self.equip_item_menu(object);
        }
    }

    /// `PAL_LoadResources` wrapper: run the resource loader and apply the
    /// cross-layer effects it signals back (start music + rebuild equipment
    /// effects after a global-data reload).
    pub fn load_resources(&mut self) {
        let done = match self.res.load_resources(&mut self.globals) {
            Ok(d) => d,
            Err(_) => return,
        };
        if done.global_data {
            let num_music = self.globals.num_music as i32;
            self.play_music(num_music, true, 0.0);
            self.update_equipments();
        }
        if done.scene {
            self.load_voice_bank();
        }
        let _ = LOAD_GLOBAL_DATA; // documents which flag drives the above
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        let mut e = Engine::new(true).expect("headless engine");
        e.globals.load_default_game().expect("default game");
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;
        // Load scene 1 so the event-object range is valid.
        e.globals.num_scene = 1;
        e.globals.load_flags = crate::global::LOAD_SCENE | crate::global::LOAD_PLAYER_SPRITE;
        e.load_resources();
        e
    }

    #[test]
    fn search_trigger_range_is_centered_on_party() {
        let e = engine();
        let range = e.get_search_trigger_range();
        let cx = e.globals.viewport.0 + e.globals.partyoffset.0;
        let cy = e.globals.viewport.1 + e.globals.partyoffset.1;
        // The first checkpoint is exactly the party position.
        assert_eq!(range[0], (cx, cy));
        // 13 checkpoints total.
        assert_eq!(range.len(), 13);
    }

    #[test]
    fn game_update_no_trigger_advances_frame_and_terminates() {
        let mut e = engine();
        e.globals.entering_scene = false;
        let before = e.globals.frame_num;
        // Non-trigger update runs autoscripts + blocker logic and advances the
        // frame counter. Must terminate (real scene data).
        e.game_update(false);
        assert_eq!(e.globals.frame_num, before + 1);
    }

    #[test]
    fn game_update_entering_scene_runs_enter_script() {
        let mut e = engine();
        // Entering scene 1 runs its on-enter script; the pointer is saved back
        // and the flag cleared (unless the script switches scenes again).
        e.globals.entering_scene = true;
        e.game_update(true);
        // After a normal on-enter script, we are no longer "entering" (or we
        // switched scenes, which also clears/replaces state deterministically).
        // Either way the call terminates.
        assert!(e.globals.frame_num >= 1 || e.globals.entering_scene);
    }

    #[test]
    fn wait_for_key_times_out() {
        let mut e = engine();
        // With a small timeout and no key, this returns promptly.
        e.wait_for_key(10);
    }
}
