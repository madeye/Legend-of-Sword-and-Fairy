//! Ending sequences and FBP full-screen picture helpers (port of SDLPAL
//! ending.c, DOS paths — no AVI / CD audio).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::game_loop::Engine;
use crate::input::{KEY_MENU, KEY_SEARCH};
use crate::surface::{self, Surface, SCREEN_H, SCREEN_W};

/// Local port of PAL_ApplyWave (scene.c) used by the ending helpers.
/// XXX consolidate with scene.rs's version once the scene port lands.
fn apply_wave(engine: &mut Engine) {
    static INDEX: AtomicUsize = AtomicUsize::new(0);

    engine.globals.screen_wave = engine
        .globals
        .screen_wave
        .wrapping_add(engine.globals.wave_progression as u16);

    if engine.globals.screen_wave == 0 || engine.globals.screen_wave >= 256 {
        engine.globals.screen_wave = 0;
        engine.globals.wave_progression = 0;
        return;
    }

    let mut wave = [0usize; 32];
    let mut a = 0i32;
    let mut b = 60 + 8;
    for i in 0..16 {
        b -= 8;
        a += b;
        wave[i] = (a * engine.globals.screen_wave as i32 / 256) as usize;
        wave[i + 16] = SCREEN_W - wave[i];
    }

    let mut idx = INDEX.load(Ordering::Relaxed);
    let start = idx;
    let mut buf = [0u8; SCREEN_W];
    for y in 0..SCREEN_H {
        let shift = wave[idx];
        if shift > 0 && shift < SCREEN_W {
            let line = &mut engine.screen.pixels[y * SCREEN_W..(y + 1) * SCREEN_W];
            buf[..shift].copy_from_slice(&line[..shift]);
            line.copy_within(shift..SCREEN_W, 0);
            line[SCREEN_W - shift..].copy_from_slice(&buf[..shift]);
        }
        idx = (idx + 1) % 32;
    }
    INDEX.store((start + 1) % 32, Ordering::Relaxed);
}

/// Local port of PAL_WaitForKey (script.c) for the ending flow.
/// XXX consolidate with script.rs's version once the script port lands.
fn wait_for_key(engine: &mut Engine, timeout_ms: u64) {
    let deadline = if timeout_ms == 0 {
        u64::MAX
    } else {
        engine.ticks() + timeout_ms
    };
    engine.input.clear_key_state();
    loop {
        engine.process_event();
        if engine.input.pressed(KEY_SEARCH | KEY_MENU) || engine.quit_requested {
            break;
        }
        if engine.ticks() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    engine.input.clear_key_state();
}

impl Engine {
    /// PAL_EndingSetEffectSprite.
    pub fn ending_set_effect_sprite(&mut self, sprite_num: u16) {
        self.ending_effect_sprite = sprite_num;
    }

    fn load_effect_sprite(&self) -> Option<Vec<u8>> {
        let n = self.ending_effect_sprite;
        if n == 0 {
            return None;
        }
        self.globals.files.mgo.chunk_decompressed(n as usize).ok()
    }

    /// PAL_ShowFBP: draw FBP.MKF chunk `chunk_num` to the screen with the
    /// interleaved nibble fade effect.
    pub fn show_fbp(&mut self, chunk_num: u16, fade: u16) {
        const RG_INDEX: [usize; 6] = [0, 3, 1, 5, 2, 4];

        let buf = self
            .globals
            .files
            .fbp
            .chunk_decompressed(chunk_num as usize)
            .unwrap_or_else(|_| vec![0; SCREEN_W * SCREEN_H]);
        let sprite = self.load_effect_sprite();

        if fade != 0 {
            let fade = (fade as u64 + 1) * 10;
            let mut p = Surface::screen();
            p.blit_fbp(&buf);
            self.backup_screen();

            for i in 0..16 {
                for &start in RG_INDEX.iter() {
                    // Blend pixels of the two buffers into the backup.
                    let mut k = start;
                    while k < SCREEN_W * SCREEN_H {
                        let a = p.pixels[k];
                        let mut b = self.screen_bak.pixels[k];
                        if i > 0 {
                            if (a & 0x0F) > (b & 0x0F) {
                                b = b.wrapping_add(1);
                            } else if (a & 0x0F) < (b & 0x0F) {
                                b = b.wrapping_sub(1);
                            }
                        }
                        self.screen_bak.pixels[k] = (a & 0xF0) | (b & 0x0F);
                        k += 6;
                    }

                    self.restore_screen();

                    if let Some(sp) = sprite.as_ref() {
                        let count = surface::sprite_frame_count(sp).max(1);
                        let f = (self.ticks() / 150) as usize % count;
                        if let Some(frame) = surface::sprite_frame(sp, f) {
                            self.screen.blit_rle(frame, 0, 0);
                        }
                    }

                    self.video_update();
                    self.delay(fade);
                }
            }
        }

        // HACKHACK from the C: to make the ending show correctly (DOS
        // chunk 49 stays as blended).
        if chunk_num != 49 {
            self.screen.blit_fbp(&buf);
        }
        self.video_update();
    }

    /// PAL_ScrollFBP.
    pub fn scroll_fbp(&mut self, chunk_num: u16, scroll_speed: u16, scroll_down: bool) {
        let Ok(buf) = self
            .globals
            .files
            .fbp
            .chunk_decompressed(chunk_num as usize)
        else {
            return;
        };
        let sprite = self.load_effect_sprite();

        self.backup_screen();
        let mut p = Surface::screen();
        p.blit_fbp(&buf);

        let scroll_speed = scroll_speed.max(1) as u64;

        for l in 0..220usize {
            let i = l.min(200);

            // Copy the still-visible part of the old screen and the
            // incoming part of the new picture.
            if scroll_down {
                copy_rows(&self.screen_bak.pixels, 0, &mut self.screen, i, 200 - i);
                copy_rows(&p.pixels, 200 - i, &mut self.screen, 0, i);
            } else {
                copy_rows(&self.screen_bak.pixels, i, &mut self.screen, 0, 200 - i);
                copy_rows(&p.pixels, 0, &mut self.screen, 200 - i, i);
            }

            apply_wave(self);

            if let Some(sp) = sprite.as_ref() {
                let count = surface::sprite_frame_count(sp).max(1);
                let f = (self.ticks() / 150) as usize % count;
                if let Some(frame) = surface::sprite_frame(sp, f) {
                    self.screen.blit_rle(frame, 0, 0);
                }
            }

            self.video_update();

            if self.globals.need_to_fade_in {
                self.fade_in(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    1,
                );
                self.globals.need_to_fade_in = false;
            }

            self.delay(800 / scroll_speed);
        }

        self.screen.pixels.copy_from_slice(&p.pixels);
        self.video_update();
    }

    /// PAL_EndingAnimation: the scrolling two-background beast/girl scene.
    pub fn ending_animation(&mut self) {
        let Ok(upper_buf) = self.globals.files.fbp.chunk_decompressed(61) else {
            return;
        };
        let Ok(lower_buf) = self.globals.files.fbp.chunk_decompressed(62) else {
            return;
        };
        let Ok(beast) = self.globals.files.mgo.chunk_decompressed(571) else {
            return;
        };
        let Ok(girl) = self.globals.files.mgo.chunk_decompressed(572) else {
            return;
        };

        let mut upper = Surface::screen();
        upper.blit_fbp(&upper_buf);
        let mut lower = Surface::screen();
        lower.blit_fbp(&lower_buf);

        let mut y_pos_girl: i32 = 180;
        self.globals.screen_wave = 2;

        for i in 0..400usize {
            // Background: lower shifted down, upper scrolling in from top.
            copy_rows(&lower.pixels, 0, &mut self.screen, i / 2, 200 - i / 2);
            copy_rows(&upper.pixels, 200 - i / 2, &mut self.screen, 0, i / 2);

            apply_wave(self);

            // The beast.
            if let Some(f) = surface::sprite_frame(&beast, 0) {
                self.screen.blit_rle(f, 0, -400 + i as i32);
            }
            if let Some(f) = surface::sprite_frame(&beast, 1) {
                self.screen.blit_rle(f, 0, -200 + i as i32);
            }

            // The girl.
            y_pos_girl -= (i & 1) as i32;
            if y_pos_girl < 80 {
                y_pos_girl = 80;
            }
            let count = surface::sprite_frame_count(&girl).max(1);
            let gf = (self.ticks() / 50) as usize % count.min(4);
            if let Some(f) = surface::sprite_frame(&girl, gf) {
                self.screen.blit_rle(f, 220, y_pos_girl);
            }

            self.video_update();
            if self.globals.need_to_fade_in {
                self.fade_in(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    1,
                );
                self.globals.need_to_fade_in = false;
            }

            self.delay(50);
        }

        self.globals.screen_wave = 0;
    }

    /// PAL_EndingScreen (DOS simulation path: RIX music, no AVI/CD).
    pub fn ending_screen(&mut self) {
        self.play_music(0x1a, true, 0.0);
        let rng = self.globals.cur_playing_rng as u16;
        self.rng_play(rng, 110, 150, 7);
        self.rng_play(rng, 151, -1, 9);

        self.fade_out(2);

        self.play_music(0x19, true, 0.0);

        self.show_fbp(75, 0);
        self.fade_in(5, false, 1);
        self.scroll_fbp(74, 0xf, true);

        self.fade_out(1);

        self.screen.clear(0);
        self.globals.num_palette = 4;
        self.globals.need_to_fade_in = true;
        self.ending_animation();

        self.play_music(0, false, 2.0);
        self.color_fade(7, 15, false);

        self.play_music(0x11, true, 0.0);

        self.screen.clear(0);
        self.set_palette(0, false);
        self.rng_play(0xb, 0, -1, 7);

        self.fade_out(2);

        self.screen.clear(0);
        self.globals.num_palette = 8;
        self.globals.need_to_fade_in = true;
        self.rng_play(10, 0, -1, 6);

        self.ending_set_effect_sprite(0);
        self.show_fbp(77, 10);

        self.backup_screen();

        self.ending_set_effect_sprite(0x27b);
        self.show_fbp(76, 7);

        self.set_palette(5, false);
        self.show_fbp(73, 7);
        self.scroll_fbp(72, 0xf, true);

        self.show_fbp(71, 7);
        self.show_fbp(68, 7);

        self.ending_set_effect_sprite(0);
        self.show_fbp(68, 6);

        wait_for_key(self, 0);
        self.play_music(0, false, 1.0);
        self.delay(500);

        // Final credits scroll.
        self.play_music(9, true, 0.0);
        for chunk in (59..=67).rev() {
            self.scroll_fbp(chunk, 0xf, true);
        }
        self.play_music(0, false, 6.0);
        self.fade_out(3);
    }
}

/// Copy `rows` full scanlines from src (starting at src_y) to dst surface
/// (starting at dst_y).
fn copy_rows(src: &[u8], src_y: usize, dst: &mut Surface, dst_y: usize, rows: usize) {
    for r in 0..rows {
        let so = (src_y + r) * SCREEN_W;
        let do_ = (dst_y + r) * SCREEN_W;
        if so + SCREEN_W <= src.len() && do_ + SCREEN_W <= dst.pixels.len() {
            dst.pixels[do_..do_ + SCREEN_W].copy_from_slice(&src[so..so + SCREEN_W]);
        }
    }
}
