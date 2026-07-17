//! Scene engine (port of SDLPAL scene.c, DOS/PAL_CLASSIC paths): drawing the
//! map + sprites for the current frame, obstacle detection, and party/NPC
//! movement and walking-gesture animation.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::global::{self, Globals, Trail};
use crate::input;
use crate::map::Rect;
use crate::res::Resources;
use crate::surface::{self, SCREEN_W};

/// MAX_SPRITE_TO_DRAW.
const MAX_SPRITE_TO_DRAW: usize = 2048;

/// SPRITE_TO_DRAW. `frame` borrows either a player/event-object sprite frame
/// or a map tile bitmap (added by `calc_cover_tiles`), all ultimately owned
/// by `Resources`.
struct SpriteToDraw<'a> {
    frame: &'a [u8],
    x: i32,
    y: i32,
    layer: i32,
}

/// PAL_AddSpriteToDraw.
fn add_sprite_to_draw<'a>(
    queue: &mut Vec<SpriteToDraw<'a>>,
    frame: &'a [u8],
    x: i32,
    y: i32,
    layer: i32,
) {
    debug_assert!(
        queue.len() < MAX_SPRITE_TO_DRAW,
        "sprite-to-draw queue overflow (C: assert(g_nSpriteToDraw < MAX_SPRITE_TO_DRAW))"
    );
    if queue.len() >= MAX_SPRITE_TO_DRAW {
        return;
    }
    queue.push(SpriteToDraw { frame, x, y, layer });
}

/// PAL_CalcCoverTiles: find the map tiles which may cover the sprite just
/// added at (`pos`, `layer`) with bitmap `frame`, and queue them too.
fn calc_cover_tiles<'a>(
    queue: &mut Vec<SpriteToDraw<'a>>,
    res: &'a Resources,
    viewport: (i32, i32),
    pos: (i32, i32),
    layer: i32,
    frame: &[u8],
) {
    let Some(map) = res.map.as_ref() else {
        return;
    };

    let sx = viewport.0 + pos.0 - layer / 2;
    let sy = viewport.1 + pos.1 - layer;
    let sh = if sx % 32 != 0 { 1 } else { 0 };

    let width = surface::rle_width(frame) as i32;
    let height = surface::rle_height(frame) as i32;

    // dx/dy/dh persist across the whole nested loop below (not reset per
    // iteration), exactly like the C locals of the same name: case 1 of the
    // switch only updates dx, intentionally reusing dy/dh from case 0. The
    // initial values are always overwritten (case 0 always runs first, for
    // i_start == 0 on the first x of every y) before being read.
    #[allow(unused_assignments)]
    let mut dx = 0i32;
    let mut dy = 0i32;
    let mut dh = 0i32;

    let y0 = (sy - height - 15) / 16;
    let y1 = sy / 16;
    for y in y0..=y1 {
        let x0 = (sx - width / 2) / 32;
        let x1 = (sx + width / 2) / 32;
        for x in x0..=x1 {
            let i_start = if x == x0 { 0 } else { 3 };
            for i in i_start..5 {
                match i {
                    0 => {
                        dx = x;
                        dy = y;
                        dh = sh;
                    }
                    1 => {
                        dx = x - 1;
                    }
                    2 => {
                        dx = if sh != 0 { x } else { x - 1 };
                        dy = if sh != 0 { y + 1 } else { y };
                        dh = 1 - sh;
                    }
                    3 => {
                        dx = x + 1;
                        dy = y;
                        dh = sh;
                    }
                    4 => {
                        dx = if sh != 0 { x + 1 } else { x };
                        dy = if sh != 0 { y + 1 } else { y };
                        dh = 1 - sh;
                    }
                    _ => unreachable!(),
                }

                for l in 0..2i32 {
                    let tile = map.get_tile_bitmap(dx as u8, dy as u8, dh as u8, l as u8);
                    let Some(bitmap) = tile else { continue };
                    // (signed char) cast in C is a no-op here: the height
                    // value is always in 0..=15.
                    let tile_height =
                        map.get_tile_height(dx as u8, dy as u8, dh as u8, l as u8) as i32;
                    if tile_height > 0 && (dy + tile_height) * 16 + dh * 8 >= sy {
                        add_sprite_to_draw(
                            queue,
                            bitmap,
                            dx * 32 + dh * 16 - 16 - viewport.0,
                            dy * 16 + dh * 8 + 7 + l + tile_height * 8 - viewport.1,
                            tile_height * 8 + l,
                        );
                    }
                }
            }
        }
    }
}

/// PAL_SceneDrawSprites (queue-building half): gather every player and
/// event-object sprite plus their covering map tiles, then sort by Y like
/// the C bubble sort (which — because it only swaps strictly-greater
/// adjacent pairs — is a stable sort, so `sort_by_key` reproduces it).
fn calc_sprites_to_draw<'a>(res: &'a Resources, globals: &Globals) -> Vec<SpriteToDraw<'a>> {
    let mut queue: Vec<SpriteToDraw<'a>> = Vec::new();
    let viewport = globals.viewport;

    // Players (party members + followers).
    let last = globals.max_party_member_index as usize + globals.follower_num as usize;
    for i in 0..=last {
        let Some(sprite) = res.player_sprite(i) else {
            continue;
        };
        let frame_num = globals.party[i].frame as usize;
        let Some(bitmap) = surface::sprite_frame(sprite, frame_num) else {
            continue;
        };
        let width = surface::rle_width(bitmap) as i32;
        let x = globals.party[i].x as i32 - width / 2;
        let y = globals.party[i].y as i32 + globals.layer as i32 + 10;
        let layer = globals.layer as i32 + 6;
        add_sprite_to_draw(&mut queue, bitmap, x, y, layer);
        calc_cover_tiles(&mut queue, res, viewport, (x, y), layer, bitmap);
    }

    // Event objects (monsters/NPCs/others) of the current scene.
    let num_scene = globals.num_scene as usize;
    if num_scene >= 1 && num_scene < globals.game.scenes.len() {
        let start = globals.game.scenes[num_scene - 1].event_object_index as usize;
        let end = globals.game.scenes[num_scene].event_object_index as usize;
        for i in start..end {
            let Some(eo) = globals.game.event_objects.get(i) else {
                continue;
            };
            if eo.state == global::OBJSTATE_HIDDEN || eo.vanish_time > 0 || eo.state < 0 {
                continue;
            }
            let Some(sprite) = res.event_object_sprite(i + 1) else {
                continue;
            };

            let mut frame_idx = eo.current_frame_num;
            if eo.sprite_frames == 3 {
                // walking character
                if frame_idx == 2 {
                    frame_idx = 0;
                }
                if frame_idx == 3 {
                    frame_idx = 2;
                }
            }
            let n = eo.direction as usize * eo.sprite_frames as usize + frame_idx as usize;
            let Some(bitmap) = surface::sprite_frame(sprite, n) else {
                continue;
            };
            let width = surface::rle_width(bitmap) as i32;
            let height = surface::rle_height(bitmap) as i32;

            let mut x = eo.x as i16 as i32 - viewport.0;
            x -= width / 2;
            if x >= 320 || x < -width {
                continue;
            }

            let mut y = eo.y as i16 as i32 - viewport.1;
            y += eo.layer as i32 * 8 + 9;

            let vy = y - height - eo.layer as i32 * 8 + 2;
            if vy >= 200 || vy < -height {
                continue;
            }

            let layer = eo.layer as i32 * 8 + 2;
            add_sprite_to_draw(&mut queue, bitmap, x, y, layer);
            calc_cover_tiles(&mut queue, res, viewport, (x, y), layer, bitmap);
        }
    }

    queue.sort_by_key(|s| s.y);
    queue
}

/// Module-private state for the scene engine (the C statics of scene.c).
#[derive(Default)]
pub struct SceneState {
    /// PAL_UpdatePartyGestures's `s_iThisStepFrame`.
    pub this_step_frame: i32,
    /// PAL_ApplyWave's `index`.
    pub wave_index: usize,
}

impl Engine {
    // =======================================================================
    // Screen waving (PAL_ApplyWave). The C signature takes an SDL_Surface*
    // but the engine only ever calls it on `gpScreen`, so this operates on
    // `self.screen` directly.
    // =======================================================================

    /// PAL_ApplyWave.
    pub fn apply_wave(&mut self) {
        self.globals.screen_wave = self
            .globals
            .screen_wave
            .wrapping_add(self.globals.wave_progression as u16);

        if self.globals.screen_wave == 0 || self.globals.screen_wave >= 256 {
            self.globals.screen_wave = 0;
            self.globals.wave_progression = 0;
            return;
        }

        let mut wave = [0i32; 32];
        let mut a = 0i32;
        let mut b = 60 + 8;
        for i in 0..16usize {
            b -= 8;
            a += b;
            wave[i] = a * self.globals.screen_wave as i32 / 256;
            wave[i + 16] = 320 - wave[i];
        }

        let mut idx = self.scene.wave_index;
        for y in 0..self.screen.h {
            let shift = wave[idx];
            if shift > 0 {
                let shift = shift as usize;
                let row_start = y * SCREEN_W;
                let row = &mut self.screen.pixels[row_start..row_start + SCREEN_W];
                let mut buf = [0u8; SCREEN_W];
                buf[..shift].copy_from_slice(&row[..shift]);
                row.copy_within(shift.., 0);
                row[SCREEN_W - shift..].copy_from_slice(&buf[..shift]);
            }
            idx = (idx + 1) % 32;
        }
        self.scene.wave_index = (self.scene.wave_index + 1) % 32;
    }

    // =======================================================================
    // Scene drawing.
    // =======================================================================

    /// PAL_SceneDrawSprites.
    fn scene_draw_sprites(&mut self) {
        let res = &self.res;
        let globals = &self.globals;
        let queue = calc_sprites_to_draw(res, globals);
        for s in &queue {
            let y = s.y - surface::rle_height(s.frame) as i32 - s.layer;
            self.screen.blit_rle(s.frame, s.x, y);
        }
    }

    /// PAL_MakeScene: draw the entire scene (map + sprites) to self.screen.
    pub fn make_scene(&mut self) {
        let rect = Rect {
            x: self.globals.viewport.0,
            y: self.globals.viewport.1,
            w: 320,
            h: 200,
        };

        // Step 1: draw the complete map, for both of the layers.
        if let Some(map) = self.res.map.as_ref() {
            map.blit_to_surface(&mut self.screen, rect, 0);
            map.blit_to_surface(&mut self.screen, rect, 1);
        }

        // Step 2: apply screen waving effects.
        self.apply_wave();

        // Step 3: draw all the sprites.
        self.scene_draw_sprites();

        // Check if we need to fade in.
        if self.globals.need_to_fade_in {
            self.video_update();
            self.fade_in(
                self.globals.num_palette as usize,
                self.globals.night_palette,
                1,
            );
            self.globals.need_to_fade_in = false;
        }
    }

    // =======================================================================
    // Obstacle checking.
    // =======================================================================

    /// PAL_CheckObstacle.
    pub fn check_obstacle(
        &self,
        pos: (i32, i32),
        check_event_objects: bool,
        self_object: u16,
    ) -> bool {
        self.check_obstacle_with_range(pos, check_event_objects, self_object, false)
    }

    /// PAL_CheckObstacleWithRange.
    pub fn check_obstacle_with_range(
        &self,
        pos: (i32, i32),
        check_event_objects: bool,
        self_object: u16,
        check_range: bool,
    ) -> bool {
        let block_x = self.globals.partyoffset.0 / 32;
        let block_y = self.globals.partyoffset.1 / 16;

        let mut x = pos.0 / 32;
        let mut y = pos.1 / 16;
        let mut h = 0i32;

        // Avoid walk out of range, look out of map.
        if check_range && (x < block_x || x >= 2048 || y < block_y || y >= 2048) {
            return true;
        }

        let xr = pos.0 % 32;
        let yr = pos.1 % 16;

        if xr + yr * 2 >= 16 {
            if xr + yr * 2 >= 48 {
                x += 1;
                y += 1;
            } else if 32 - xr + yr * 2 < 16 {
                x += 1;
            } else if 32 - xr + yr * 2 < 48 {
                h = 1;
            } else {
                y += 1;
            }
        }

        let blocked = match self.res.map.as_ref() {
            Some(map) => map.tile_is_blocked(x as u8, y as u8, h as u8),
            None => true,
        };
        if blocked {
            return true;
        }

        if check_event_objects {
            let num_scene = self.globals.num_scene as usize;
            if num_scene >= 1 && num_scene < self.globals.game.scenes.len() {
                let start = self.globals.game.scenes[num_scene - 1].event_object_index as usize;
                let end = self.globals.game.scenes[num_scene].event_object_index as usize;
                for i in start..end {
                    // Skip myself (wSelfObject == 0 means "no self", and in
                    // the C code `i == wSelfObject - 1` can never match for
                    // wSelfObject == 0 since that underflows to 0xFFFF).
                    if self_object != 0 && i + 1 == self_object as usize {
                        continue;
                    }
                    let p = &self.globals.game.event_objects[i];
                    if p.state >= global::OBJSTATE_BLOCKER
                        && (p.x as i32 - pos.0).abs() + (p.y as i32 - pos.1).abs() * 2 < 16
                    {
                        return true;
                    }
                }
            }
        }

        false
    }

    // =======================================================================
    // Party movement and gestures.
    // =======================================================================

    /// PAL_UpdatePartyGestures.
    pub fn update_party_gestures(&mut self, walking: bool) {
        if walking {
            self.scene.this_step_frame = (self.scene.this_step_frame + 1) % 4;
            let (step_leader, step_follower) = if self.scene.this_step_frame & 1 != 0 {
                let leader = (self.scene.this_step_frame + 1) / 2;
                (leader, 3 - leader)
            } else {
                (0, 0)
            };

            self.globals.party[0].x = self.globals.partyoffset.0 as i16;
            self.globals.party[0].y = self.globals.partyoffset.1 as i16;

            let role0 = self.globals.party[0].player_role as usize;
            if self.globals.game.player_roles.walk_frames[role0] == 4 {
                self.globals.party[0].frame =
                    self.globals.party_direction * 4 + self.scene.this_step_frame as u16;
            } else {
                self.globals.party[0].frame = self.globals.party_direction * 3 + step_leader as u16;
            }

            // Update the gestures and positions for other party members.
            for i in 1..=self.globals.max_party_member_index as usize {
                let base_x = self.globals.trail[1].x as i32 - self.globals.viewport.0;
                let base_y = self.globals.trail[1].y as i32 - self.globals.viewport.1;
                let dir1 = self.globals.trail[1].direction;

                let (dx, dy): (i32, i32) = if i == 2 {
                    let dx = if dir1 == global::DIR_EAST || dir1 == global::DIR_WEST {
                        -16
                    } else {
                        16
                    };
                    (dx, 8)
                } else {
                    let dx = if dir1 == global::DIR_WEST || dir1 == global::DIR_SOUTH {
                        16
                    } else {
                        -16
                    };
                    let dy = if dir1 == global::DIR_WEST || dir1 == global::DIR_NORTH {
                        8
                    } else {
                        -8
                    };
                    (dx, dy)
                };

                self.globals.party[i].x = (base_x + dx) as i16;
                self.globals.party[i].y = (base_y + dy) as i16;

                // Adjust the position if there is obstacle.
                let check_pos = (
                    self.globals.party[i].x as i32 + self.globals.viewport.0,
                    self.globals.party[i].y as i32 + self.globals.viewport.1,
                );
                if self.check_obstacle_with_range(check_pos, true, 0, true) {
                    self.globals.party[i].x = base_x as i16;
                    self.globals.party[i].y = base_y as i16;
                }

                // Update gesture for this party member.
                let role_i = self.globals.party[i].player_role as usize;
                let dir2 = self.globals.trail[2].direction;
                if self.globals.game.player_roles.walk_frames[role_i] == 4 {
                    self.globals.party[i].frame = dir2 * 4 + self.scene.this_step_frame as u16;
                } else {
                    self.globals.party[i].frame = dir2 * 3 + step_leader as u16;
                }
            }

            for i in 1..=self.globals.follower_num as usize {
                let idx = self.globals.max_party_member_index as usize + i;
                let trail_idx = 2 + i;
                self.globals.party[idx].x =
                    (self.globals.trail[trail_idx].x as i32 - self.globals.viewport.0) as i16;
                self.globals.party[idx].y =
                    (self.globals.trail[trail_idx].y as i32 - self.globals.viewport.1) as i16;
                self.globals.party[idx].frame =
                    self.globals.trail[trail_idx].direction * 3 + step_follower as u16;
            }
        } else {
            // Player is not moved. Use the "standing" gesture instead of
            // the "walking" one.
            let role0 = self.globals.party[0].player_role as usize;
            let mut f0 = self.globals.game.player_roles.walk_frames[role0];
            if f0 == 0 {
                f0 = 3;
            }
            self.globals.party[0].frame = self.globals.party_direction * f0;

            for i in 1..=self.globals.max_party_member_index as usize {
                let role_i = self.globals.party[i].player_role as usize;
                let mut f = self.globals.game.player_roles.walk_frames[role_i];
                if f == 0 {
                    f = 3;
                }
                self.globals.party[i].frame = self.globals.trail[2].direction * f;
            }

            for i in 1..=self.globals.follower_num as usize {
                let idx = self.globals.max_party_member_index as usize + i;
                self.globals.party[idx].frame = self.globals.trail[2 + i].direction * 3;
            }

            self.scene.this_step_frame &= 2;
            self.scene.this_step_frame ^= 2;
        }
    }

    /// PAL_UpdateParty: walk the party according to input.
    pub fn update_party(&mut self) {
        if self.input.dir != input::DIR_UNKNOWN {
            let dir = self.input.dir;
            let x_offset = if dir == input::DIR_WEST || dir == input::DIR_SOUTH {
                -16
            } else {
                16
            };
            let y_offset = if dir == input::DIR_WEST || dir == input::DIR_NORTH {
                -8
            } else {
                8
            };

            let x_source = self.globals.viewport.0 + self.globals.partyoffset.0;
            let y_source = self.globals.viewport.1 + self.globals.partyoffset.1;
            let x_target = x_source + x_offset;
            let y_target = y_source + y_offset;

            self.globals.party_direction = dir as u16;

            // Check for obstacles on the destination location.
            if !self.check_obstacle_with_range((x_target, y_target), true, 0, true) {
                // Player will actually be moved. Store trail.
                for i in (0..=3).rev() {
                    self.globals.trail[i + 1] = self.globals.trail[i];
                }
                self.globals.trail[0] = Trail {
                    x: x_source as u16,
                    y: y_source as u16,
                    direction: dir as u16,
                };

                // Move the viewport.
                self.globals.viewport = (
                    self.globals.viewport.0 + x_offset,
                    self.globals.viewport.1 + y_offset,
                );

                self.update_party_gestures(true);
                return; // don't go further
            }
        }

        self.update_party_gestures(false);
    }

    /// PAL_NPCWalkOneStep: move and animate the specified event object (NPC).
    pub fn npc_walk_one_step(&mut self, event_object_id: u16, speed: i32) {
        if event_object_id == 0 || event_object_id as usize > self.globals.game.event_objects.len()
        {
            return;
        }

        let p = &mut self.globals.game.event_objects[event_object_id as usize - 1];

        let dx = if p.direction == global::DIR_WEST || p.direction == global::DIR_SOUTH {
            -2
        } else {
            2
        };
        let dy = if p.direction == global::DIR_WEST || p.direction == global::DIR_NORTH {
            -1
        } else {
            1
        };
        p.x = (p.x as i32 + dx * speed) as u16;
        p.y = (p.y as i32 + dy * speed) as u16;

        // Update the gesture.
        if p.sprite_frames > 0 {
            p.current_frame_num += 1;
            p.current_frame_num %= if p.sprite_frames == 3 {
                4
            } else {
                p.sprite_frames
            };
        } else if p.sprite_frames_auto > 0 {
            p.current_frame_num += 1;
            p.current_frame_num %= p.sprite_frames_auto;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::{EventObject, LOAD_PLAYER_SPRITE, LOAD_SCENE};

    fn engine() -> Engine {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        Engine::new(true).expect("engine")
    }

    // ---------------------------------------------------------------------
    // PAL_MakeScene
    // ---------------------------------------------------------------------

    #[test]
    fn make_scene_draws_nonempty_and_deterministic_frame() {
        fn build(viewport: (i32, i32)) -> Engine {
            let mut eng = engine();
            eng.globals.load_default_game().unwrap();
            eng.globals.max_party_member_index = 0;
            eng.globals.party[0].player_role = 0;
            eng.globals.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;
            eng.res.load_resources(&mut eng.globals).unwrap();
            eng.globals.viewport = viewport;
            eng.globals.party[0].x = eng.globals.partyoffset.0 as i16;
            eng.globals.party[0].y = eng.globals.partyoffset.1 as i16;
            eng
        }

        // Scene 1's map isn't necessarily "full" right at world (0, 0) (map
        // edges/borders can be blank), so scan a coarse grid of candidate
        // viewports across the map and keep the best-populated one — this
        // is only test setup, not something make_scene itself has to do
        // (the real engine's initial viewport comes from play.c/script.c,
        // out of scene.c's scope).
        let probe = build((0, 0));
        let map = probe.res.map.as_ref().expect("map loaded");
        let rect = Rect {
            x: 0,
            y: 0,
            w: 320,
            h: 200,
        };
        let mut best = ((0i32, 0i32), 0usize);
        for ty in 0..16i32 {
            for tx in 0..16i32 {
                let vp = (tx * 128, ty * 128);
                let mut s = crate::surface::Surface::new(320, 200);
                let r = Rect {
                    x: vp.0,
                    y: vp.1,
                    ..rect
                };
                map.blit_to_surface(&mut s, r, 0);
                let nz = s.pixels.iter().filter(|&&p| p != 0).count();
                if nz > best.1 {
                    best = (vp, nz);
                }
            }
        }
        let viewport = best.0;
        assert!(
            best.1 as f64 / (320.0 * 200.0) > 0.1,
            "could not find a well-populated viewport on map {} (best frac {})",
            map.num,
            best.1 as f64 / (320.0 * 200.0)
        );

        let mut a = build(viewport);
        let mut b = build(viewport);
        a.make_scene();
        b.make_scene();

        let nonzero = a.screen.pixels.iter().filter(|&&p| p != 0).count();
        let frac = nonzero as f64 / a.screen.pixels.len() as f64;
        assert!(frac > 0.1, "expected a well-populated frame, got {frac}");

        assert_eq!(
            a.screen.pixels, b.screen.pixels,
            "make_scene should be deterministic for identical state"
        );
    }

    // ---------------------------------------------------------------------
    // PAL_ApplyWave
    // ---------------------------------------------------------------------

    #[test]
    fn apply_wave_progresses_and_resets() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();

        eng.globals.screen_wave = 0;
        eng.globals.wave_progression = 8;
        eng.apply_wave();
        assert_eq!(eng.globals.screen_wave, 8);
        assert_eq!(eng.scene.wave_index, 1);

        // Push wScreenWave to >= 256: resets both fields and does not
        // advance the wave index (the C code returns before that point).
        eng.globals.screen_wave = 250;
        eng.globals.wave_progression = 10;
        eng.apply_wave();
        assert_eq!(eng.globals.screen_wave, 0);
        assert_eq!(eng.globals.wave_progression, 0);
        assert_eq!(eng.scene.wave_index, 1);
    }

    // ---------------------------------------------------------------------
    // PAL_CheckObstacle / PAL_CheckObstacleWithRange
    // ---------------------------------------------------------------------

    #[test]
    fn check_obstacle_matches_map_tile_lookup() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.load_flags = LOAD_SCENE;
        eng.res.load_resources(&mut eng.globals).unwrap();
        eng.globals.viewport = (0, 0);

        let (blocked_tile, open_tile) = {
            let map = eng.res.map.as_ref().expect("map loaded");
            let mut blocked_tile = None;
            let mut open_tile = None;
            'scan: for y in 0..128u8 {
                for x in 0..64u8 {
                    let b = map.tile_is_blocked(x, y, 0);
                    if b && blocked_tile.is_none() {
                        blocked_tile = Some((x, y));
                    }
                    if !b && open_tile.is_none() {
                        open_tile = Some((x, y));
                    }
                    if blocked_tile.is_some() && open_tile.is_some() {
                        break 'scan;
                    }
                }
            }
            (
                blocked_tile.expect("map has a blocked tile"),
                open_tile.expect("map has an open tile"),
            )
        };

        let (bx, by) = blocked_tile;
        let (ox, oy) = open_tile;
        // (x*32, y*16) has xr = yr = 0, so the diamond adjustment in
        // check_obstacle leaves (x, y, h) unchanged and this exercises
        // tile_is_blocked(x, y, 0) directly.
        assert!(eng.check_obstacle((bx as i32 * 32, by as i32 * 16), false, 0));
        assert!(!eng.check_obstacle((ox as i32 * 32, oy as i32 * 16), false, 0));
    }

    #[test]
    fn check_obstacle_event_object_blocks_and_self_skip_works() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.load_flags = LOAD_SCENE;
        eng.res.load_resources(&mut eng.globals).unwrap();
        eng.globals.viewport = (0, 0);

        let (ox, oy) = {
            let map = eng.res.map.as_ref().unwrap();
            let mut open = None;
            'scan: for y in 0..128u8 {
                for x in 0..64u8 {
                    if !map.tile_is_blocked(x, y, 0) {
                        open = Some((x, y));
                        break 'scan;
                    }
                }
            }
            open.expect("map has an open tile")
        };
        let pos = (ox as i32 * 32, oy as i32 * 16);
        assert!(!eng.check_obstacle(pos, true, 0));

        let scene_idx = eng.globals.num_scene as usize - 1;
        let obj_index = eng.globals.game.scenes[scene_idx].event_object_index as usize;
        assert!(
            obj_index < eng.globals.game.scenes[scene_idx + 1].event_object_index as usize,
            "scene 1 should have at least one event object"
        );
        eng.globals.game.event_objects[obj_index] = EventObject {
            state: global::OBJSTATE_BLOCKER,
            x: pos.0 as u16,
            y: pos.1 as u16,
            ..Default::default()
        };

        assert!(eng.check_obstacle(pos, true, 0));
        // Skipping "myself" removes the block again.
        assert!(!eng.check_obstacle(pos, true, (obj_index + 1) as u16));
        // Not checking event objects at all also leaves it unblocked.
        assert!(!eng.check_obstacle(pos, false, 0));
    }

    // ---------------------------------------------------------------------
    // PAL_UpdatePartyGestures
    // ---------------------------------------------------------------------

    #[test]
    fn update_party_gestures_standing_uses_walk_frames_table() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.max_party_member_index = 0;
        eng.globals.party[0].player_role = 0;
        eng.globals.party_direction = global::DIR_EAST;
        eng.globals.game.player_roles.walk_frames[0] = 3;

        eng.update_party_gestures(false);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3);

        // walk_frames == 0 falls back to 3 frames per direction.
        eng.globals.game.player_roles.walk_frames[0] = 0;
        eng.update_party_gestures(false);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3);
    }

    #[test]
    fn update_party_gestures_walking_cycles_this_step_frame() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.max_party_member_index = 0;
        eng.globals.party[0].player_role = 0;
        eng.globals.party_direction = global::DIR_EAST;
        eng.globals.partyoffset = (160, 112);
        eng.globals.game.player_roles.walk_frames[0] = 3; // 3-frame table

        // this_step_frame: 0 -> 1 (odd, leader = 1)
        eng.update_party_gestures(true);
        assert_eq!(eng.scene.this_step_frame, 1);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3 + 1);
        assert_eq!(eng.globals.party[0].x, 160);
        assert_eq!(eng.globals.party[0].y, 112);

        // 1 -> 2 (even, leader = 0)
        eng.update_party_gestures(true);
        assert_eq!(eng.scene.this_step_frame, 2);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3);

        // 2 -> 3 (odd, leader = (3+1)/2 = 2)
        eng.update_party_gestures(true);
        assert_eq!(eng.scene.this_step_frame, 3);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3 + 2);

        // 3 -> 0 (even, leader = 0)
        eng.update_party_gestures(true);
        assert_eq!(eng.scene.this_step_frame, 0);
        assert_eq!(eng.globals.party[0].frame, global::DIR_EAST * 3);

        // Standing afterwards toggles this_step_frame's bit 2 (0 <-> 2).
        eng.update_party_gestures(false);
        assert_eq!(eng.scene.this_step_frame, 2);
        eng.update_party_gestures(false);
        assert_eq!(eng.scene.this_step_frame, 0);
    }

    #[test]
    fn update_party_gestures_walking_four_frame_table() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.max_party_member_index = 0;
        eng.globals.party[0].player_role = 0;
        eng.globals.party_direction = global::DIR_NORTH;
        eng.globals.partyoffset = (160, 112);
        eng.globals.game.player_roles.walk_frames[0] = 4;

        eng.update_party_gestures(true); // this_step_frame -> 1
        assert_eq!(eng.globals.party[0].frame, global::DIR_NORTH * 4 + 1);
    }

    #[test]
    fn update_party_gestures_second_member_reverts_without_map() {
        // With no map loaded, check_obstacle_with_range always reports
        // "blocked" (matching PAL_MapTileIsBlocked's NULL-map TRUE default),
        // so the second party member must always fall back to trail[1]'s
        // position (minus viewport), never the offset position.
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.max_party_member_index = 1;
        eng.globals.party[0].player_role = 0;
        eng.globals.party[1].player_role = 0;
        eng.globals.viewport = (0, 0);
        eng.globals.partyoffset = (160, 112);
        eng.globals.trail[1] = Trail {
            x: 200,
            y: 150,
            direction: global::DIR_EAST,
        };
        eng.globals.trail[2] = Trail {
            x: 200,
            y: 150,
            direction: global::DIR_SOUTH,
        };

        assert!(eng.res.map.is_none());
        eng.update_party_gestures(true);

        assert_eq!(eng.globals.party[1].x, 200);
        assert_eq!(eng.globals.party[1].y, 150);
    }

    // ---------------------------------------------------------------------
    // PAL_UpdateParty
    // ---------------------------------------------------------------------

    #[test]
    fn update_party_no_input_does_not_move() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.party_direction = global::DIR_NORTH;
        let viewport_before = eng.globals.viewport;
        eng.input.dir = input::DIR_UNKNOWN;

        eng.update_party();

        assert_eq!(eng.globals.viewport, viewport_before);
        // party_direction is only touched when dir != Unknown.
        assert_eq!(eng.globals.party_direction, global::DIR_NORTH);
    }

    #[test]
    fn update_party_without_map_never_moves() {
        // No map loaded => check_obstacle_with_range always blocks, so the
        // party must never actually move, regardless of requested direction.
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.party[0].x = eng.globals.partyoffset.0 as i16;
        eng.globals.party[0].y = eng.globals.partyoffset.1 as i16;
        let viewport_before = eng.globals.viewport;

        eng.input.dir = input::DIR_EAST;
        eng.update_party();

        assert_eq!(eng.globals.party_direction, global::DIR_EAST);
        assert_eq!(eng.globals.viewport, viewport_before);
    }

    #[test]
    fn update_party_moves_or_stays_consistently_with_obstacle_check() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();
        eng.globals.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;
        eng.res.load_resources(&mut eng.globals).unwrap();
        eng.globals.party[0].x = eng.globals.partyoffset.0 as i16;
        eng.globals.party[0].y = eng.globals.partyoffset.1 as i16;

        eng.input.dir = input::DIR_EAST;
        let x_source = eng.globals.viewport.0 + eng.globals.partyoffset.0;
        let y_source = eng.globals.viewport.1 + eng.globals.partyoffset.1;
        let target = (x_source + 16, y_source + 8);
        let blocked = eng.check_obstacle_with_range(target, true, 0, true);
        let viewport_before = eng.globals.viewport;

        eng.update_party();

        assert_eq!(eng.globals.party_direction, global::DIR_EAST);
        if blocked {
            assert_eq!(eng.globals.viewport, viewport_before);
        } else {
            assert_eq!(
                eng.globals.viewport,
                (viewport_before.0 + 16, viewport_before.1 + 8)
            );
            assert_eq!(eng.globals.trail[0].x, x_source as u16);
            assert_eq!(eng.globals.trail[0].y, y_source as u16);
            assert_eq!(eng.globals.trail[0].direction, global::DIR_EAST);
        }
    }

    // ---------------------------------------------------------------------
    // PAL_NPCWalkOneStep
    // ---------------------------------------------------------------------

    #[test]
    fn npc_walk_one_step_moves_and_cycles_frames() {
        let mut eng = engine();
        eng.globals.load_default_game().unwrap();

        // East: dx = +2*speed, dy = +1*speed. nSpriteFrames == 3 cycles mod 4.
        eng.globals.game.event_objects[0] = EventObject {
            direction: global::DIR_EAST,
            sprite_frames: 3,
            x: 100,
            y: 100,
            ..Default::default()
        };
        eng.npc_walk_one_step(1, 2);
        let p0 = eng.globals.game.event_objects[0];
        assert_eq!(p0.x, 104);
        assert_eq!(p0.y, 102);
        assert_eq!(p0.current_frame_num, 1);

        // West: dx = -2*speed, dy = -1*speed. Frame wraps 3 -> 0 (mod 4).
        eng.globals.game.event_objects[1] = EventObject {
            direction: global::DIR_WEST,
            sprite_frames: 3,
            current_frame_num: 3,
            x: 200,
            y: 200,
            ..Default::default()
        };
        eng.npc_walk_one_step(2, 3);
        let p1 = eng.globals.game.event_objects[1];
        assert_eq!(p1.x, 194);
        assert_eq!(p1.y, 197);
        assert_eq!(p1.current_frame_num, 0);

        // sprite_frames == 0 falls back to sprite_frames_auto's modulus.
        eng.globals.game.event_objects[2] = EventObject {
            direction: global::DIR_SOUTH,
            sprite_frames: 0,
            sprite_frames_auto: 5,
            current_frame_num: 4,
            ..Default::default()
        };
        eng.npc_walk_one_step(3, 1);
        assert_eq!(eng.globals.game.event_objects[2].current_frame_num, 0);

        // Invalid IDs are a no-op.
        let before = eng.globals.game.event_objects[0];
        eng.npc_walk_one_step(0, 5);
        eng.npc_walk_one_step(u16::MAX, 5);
        assert_eq!(eng.globals.game.event_objects[0].x, before.x);
        assert_eq!(eng.globals.game.event_objects[0].y, before.y);
    }
}
