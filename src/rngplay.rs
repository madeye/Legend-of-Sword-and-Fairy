//! RNG animated cutscene playback (port of SDLPAL rngplay.c).
//!
//! An RNG chunk in RNG.MKF is itself a nested MKF-style archive whose
//! sub-chunks are YJ_1-compressed delta frames over a 320x200 surface.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::surface::{Surface, SCREEN_H, SCREEN_W};
use crate::yj;

/// PAL_RNGReadFrame: extract the raw (still compressed) frame `frame_num`
/// of animation `rng_num` from the RNG.MKF archive bytes.
fn rng_read_frame(rng_mkf: &crate::mkf::Mkf, rng_num: usize, frame_num: usize) -> Option<Vec<u8>> {
    let chunk = rng_mkf.chunk(rng_num).ok()?;
    if chunk.len() < 4 {
        return None;
    }
    let u32_at = |off: usize| -> Option<usize> {
        Some(u32::from_le_bytes(chunk.get(off..off + 4)?.try_into().ok()?) as usize)
    };
    let count = (u32_at(0)?.checked_sub(4)?) / 4;
    if frame_num >= count {
        return None;
    }
    let sub = u32_at(4 * frame_num)?;
    let next = u32_at(4 * frame_num + 4)?;
    if next <= sub {
        return None;
    }
    chunk.get(sub..next).map(|s| s.to_vec())
}

/// PAL_RNGBlitToSurface: apply one delta frame onto the surface. The switch
/// in the C code deliberately falls through from case 0x0a down to 0x06 —
/// opcode 0x06+n writes n+1 pixel pairs from the stream.
fn rng_blit_to_surface(rng: &[u8], surf: &mut Surface) {
    let total = SCREEN_W * SCREEN_H;
    let mut ptr = 0usize;
    let mut dst = 0usize;

    // Write one pixel pair from the stream at dst.
    macro_rules! put_pair {
        ($a:expr, $b:expr) => {
            if dst + 1 < total {
                surf.pixels[dst] = $a;
                surf.pixels[dst + 1] = $b;
            }
        };
    }

    while ptr < rng.len() {
        let data = rng[ptr];
        ptr += 1;
        match data {
            0x00 | 0x13 => return, // end
            0x02 => dst += 2,
            0x03 => {
                let Some(&d) = rng.get(ptr) else { return };
                ptr += 1;
                dst += (d as usize + 1) * 2;
            }
            0x04 => {
                if ptr + 1 >= rng.len() {
                    return;
                }
                let w = rng[ptr] as usize | ((rng[ptr + 1] as usize) << 8);
                ptr += 2;
                dst += (w + 1) * 2;
            }
            0x06..=0x0a => {
                // Fall-through chain: 0x0a writes 5 pairs, 0x09 -> 4, ... 0x06 -> 1.
                let pairs = data as usize - 0x05;
                for _ in 0..pairs {
                    if ptr + 1 >= rng.len() {
                        return;
                    }
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    ptr += 2;
                    dst += 2;
                }
            }
            0x0b => {
                let Some(&d) = rng.get(ptr) else { return };
                ptr += 1;
                for _ in 0..=d as usize {
                    if ptr + 1 >= rng.len() {
                        return;
                    }
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    ptr += 2;
                    dst += 2;
                }
            }
            0x0c => {
                if ptr + 1 >= rng.len() {
                    return;
                }
                let w = rng[ptr] as usize | ((rng[ptr + 1] as usize) << 8);
                ptr += 2;
                for _ in 0..=w {
                    if ptr + 1 >= rng.len() {
                        return;
                    }
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    ptr += 2;
                    dst += 2;
                }
            }
            0x0d..=0x10 => {
                // Repeat the same pair (data - 0x0b) times.
                if ptr + 1 >= rng.len() {
                    return;
                }
                let n = data as usize - 0x0b;
                for _ in 0..n {
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    dst += 2;
                }
                ptr += 2;
            }
            0x11 => {
                let Some(&d) = rng.get(ptr) else { return };
                ptr += 1;
                if ptr + 1 >= rng.len() {
                    return;
                }
                for _ in 0..=d as usize {
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    dst += 2;
                }
                ptr += 2;
            }
            0x12 => {
                if ptr + 3 >= rng.len() {
                    return;
                }
                let n = (rng[ptr] as usize | ((rng[ptr + 1] as usize) << 8)) + 1;
                ptr += 2;
                for _ in 0..n {
                    put_pair!(rng[ptr], rng[ptr + 1]);
                    dst += 2;
                }
                ptr += 2;
            }
            _ => {
                // Unknown opcodes are ignored exactly like the C switch
                // (no default case): continue with the next byte.
            }
        }
    }
}

impl Engine {
    /// PAL_RNGPlay: play frames [start_frame, end_frame] of RNG animation
    /// `rng_num` at the given speed (frames per second; 0 = 16 fps).
    pub fn rng_play(&mut self, rng_num: u16, start_frame: i32, end_frame: i32, speed: i32) {
        let delay_ms = 1000f64 / if speed == 0 { 16.0 } else { speed as f64 };
        let Ok(rng_mkf) = self.globals.data_dir.mkf("rng.mkf") else {
            return;
        };

        // Avoid losing the last frame.
        let end_frame = if end_frame > 0 {
            end_frame + 1
        } else {
            end_frame
        };

        let mut time = self.ticks() as f64;
        let mut frame = start_frame;
        while frame != end_frame {
            time += delay_ms;

            let Some(compressed) = rng_read_frame(&rng_mkf, rng_num as usize, frame as usize)
            else {
                break;
            };
            let Ok(data) = yj::decompress(&compressed) else {
                break;
            };
            rng_blit_to_surface(&data, &mut self.screen);
            self.video_update();

            if self.globals.need_to_fade_in {
                self.fade_in(
                    self.globals.num_palette as usize,
                    self.globals.night_palette,
                    1,
                );
                self.globals.need_to_fade_in = false;
                time = self.ticks() as f64 + delay_ms / 2.0;
            }

            let deadline = time as u64;
            self.delay_until(deadline);
            frame += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDir;

    fn rng_mkf() -> crate::mkf::Mkf {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        DataDir::new().unwrap().mkf("rng.mkf").unwrap()
    }

    #[test]
    fn decodes_real_rng_frames() {
        let mkf = rng_mkf();
        assert!(mkf.chunk_count() > 0);
        let mut decoded_any = false;
        // Decode the first animation fully; the surface must accumulate
        // nonzero pixels.
        let mut surf = Surface::screen();
        let mut frame = 0usize;
        while let Some(compressed) = rng_read_frame(&mkf, 0, frame) {
            let data = crate::yj::decompress(&compressed).expect("frame decompresses");
            rng_blit_to_surface(&data, &mut surf);
            decoded_any = true;
            frame += 1;
            if frame > 500 {
                break;
            }
        }
        assert!(decoded_any, "no frames decoded from RNG 0");
        let nonzero = surf.pixels.iter().filter(|&&p| p != 0).count();
        assert!(
            nonzero > 1000,
            "RNG frame left surface mostly empty ({nonzero} nonzero)"
        );
    }

    #[test]
    fn all_animations_decode_without_panic() {
        let mkf = rng_mkf();
        for anim in 0..mkf.chunk_count() {
            let mut surf = Surface::screen();
            let mut frame = 0usize;
            while let Some(compressed) = rng_read_frame(&mkf, anim, frame) {
                if let Ok(data) = crate::yj::decompress(&compressed) {
                    rng_blit_to_surface(&data, &mut surf);
                }
                frame += 1;
                if frame > 2000 {
                    break;
                }
            }
        }
    }
}
