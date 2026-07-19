//! 8-bit indexed pixel surfaces, FBP bitmaps and RLE sprite decoding.
//! Port of the blitting primitives in SDLPAL `palcommon.c`.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 200;

/// 8-bit indexed surface (pixels are palette indices).
pub struct Surface {
    pub w: usize,
    pub h: usize,
    pub pixels: Vec<u8>,
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Surface {
        Surface {
            w,
            h,
            pixels: vec![0; w * h],
        }
    }

    /// The main 320x200 screen surface.
    pub fn screen() -> Surface {
        Surface::new(SCREEN_W, SCREEN_H)
    }

    pub fn clear(&mut self, color: u8) {
        self.pixels.fill(color);
    }

    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, c: u8) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.pixels[y as usize * self.w + x as usize] = c;
        }
    }

    #[inline]
    pub fn get_pixel(&self, x: i32, y: i32) -> u8 {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.pixels[y as usize * self.w + x as usize]
        } else {
            0
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                self.pixels[yy as usize * self.w + xx as usize] = c;
            }
        }
    }

    /// Blit an uncompressed 320x200 FBP bitmap onto this surface.
    pub fn blit_fbp(&mut self, fbp: &[u8]) {
        let n = (self.w * self.h).min(fbp.len());
        self.pixels[..n].copy_from_slice(&fbp[..n]);
    }

    /// Blit an RLE bitmap with transparency (port of PAL_RLEBlitToSurface).
    pub fn blit_rle(&mut self, rle: &[u8], x: i32, y: i32) {
        self.blit_rle_impl(rle, x, y, RleBlitMode::Normal);
    }

    /// Blit only a shadow (darken destination) shaped like the bitmap.
    pub fn blit_rle_shadow(&mut self, rle: &[u8], x: i32, y: i32) {
        self.blit_rle_impl(rle, x, y, RleBlitMode::Shadow);
    }

    /// Blit in mono-color form (port of PAL_RLEBlitMonoColor): every pixel is
    /// drawn in `color`'s hue with its brightness nibble shifted by `shift`.
    pub fn blit_rle_mono_color(&mut self, rle: &[u8], x: i32, y: i32, color: u8, shift: i32) {
        self.blit_rle_impl(rle, x, y, RleBlitMode::Mono { color, shift });
    }

    fn blit_rle_impl(&mut self, rle: &[u8], dx: i32, dy: i32, mode: RleBlitMode) {
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
        let mut i = 0usize; // decoded pixels so far
        let mut src_x = 0usize; // x inside source bitmap
        let mut dst_y = dy;
        while i < total && p < rle.len() {
            let t = rle[p];
            p += 1;
            if (t & 0x80) != 0 && (t as usize) <= 0x80 + w {
                // transparent run
                let n = (t as usize) - 0x80;
                i += n;
                src_x += n;
                if src_x >= w {
                    src_x -= w;
                    dst_y += 1;
                }
            } else {
                // literal run of t pixels
                let n = t as usize;
                if p + n > rle.len() {
                    return; // corrupt data; stop
                }
                for k in 0..n {
                    let dst_x = dx + src_x as i32;
                    if dst_x >= 0
                        && (dst_x as usize) < self.w
                        && dst_y >= 0
                        && (dst_y as usize) < self.h
                    {
                        let idx = dst_y as usize * self.w + dst_x as usize;
                        self.pixels[idx] = match mode {
                            RleBlitMode::Normal => rle[p + k],
                            RleBlitMode::Shadow => calc_shadow_color(self.pixels[idx]),
                            RleBlitMode::Mono { color, shift } => {
                                let b = (rle[p + k] & 0x0F) as i32 + shift;
                                b.clamp(0, 0x0F) as u8 | (color & 0xF0)
                            }
                        };
                    }
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

    /// Convert to RGBA for presentation. `out` must be w*h*4 bytes.
    pub fn to_rgba(&self, palette: &crate::palette::Palette, out: &mut [u8]) {
        for (i, &px) in self.pixels.iter().enumerate() {
            let c = palette.colors[px as usize];
            let o = i * 4;
            out[o] = c[0];
            out[o + 1] = c[1];
            out[o + 2] = c[2];
            out[o + 3] = 0xff;
        }
    }
}

/// Copy `rows` full 320-wide scanlines from `src` (starting at row `src_y`)
/// into `dst` (starting at row `dst_y`). Out-of-range rows are clipped.
pub fn copy_rows(src: &[u8], src_y: usize, dst: &mut Surface, dst_y: usize, rows: usize) {
    for r in 0..rows {
        let so = (src_y + r) * SCREEN_W;
        let do_ = (dst_y + r) * SCREEN_W;
        if so + SCREEN_W <= src.len() && do_ + SCREEN_W <= dst.pixels.len() {
            dst.pixels[do_..do_ + SCREEN_W].copy_from_slice(&src[so..so + SCREEN_W]);
        }
    }
}

/// Pixel treatment for `blit_rle_impl`.
#[derive(Clone, Copy)]
enum RleBlitMode {
    Normal,
    Shadow,
    Mono { color: u8, shift: i32 },
}

/// Darken a palette index (used for shadows). From PAL_CalcShadowColor.
#[inline]
pub fn calc_shadow_color(c: u8) -> u8 {
    (c & 0xF0) | ((c & 0x0F) >> 1)
}

fn u16_le(b: &[u8], off: usize) -> u16 {
    b[off] as u16 | ((b[off + 1] as u16) << 8)
}

/// Skip the optional 0x00000002 RLE file header.
fn skip_rle_header(rle: &[u8]) -> &[u8] {
    if rle.len() >= 4 && rle[0] == 0x02 && rle[1] == 0 && rle[2] == 0 && rle[3] == 0 {
        &rle[4..]
    } else {
        rle
    }
}

/// Width of an RLE bitmap.
pub fn rle_width(rle: &[u8]) -> usize {
    let rle = skip_rle_header(rle);
    if rle.len() < 4 {
        0
    } else {
        u16_le(rle, 0) as usize
    }
}

/// Height of an RLE bitmap.
pub fn rle_height(rle: &[u8]) -> usize {
    let rle = skip_rle_header(rle);
    if rle.len() < 4 {
        0
    } else {
        u16_le(rle, 2) as usize
    }
}

/// PAL_SpriteGetNumFrames: the real number of frames in a sprite. The first
/// header word doubles as both the frame-0 offset (in words) and the header
/// length, so a sprite with header word N has N-1 frames — the last header
/// word is an end marker (usually 0), not a frame offset.
pub fn sprite_num_frames(sprite: &[u8]) -> usize {
    sprite_frame_count(sprite).saturating_sub(1)
}

/// The raw first header word of a sprite: the upper bound PAL_SpriteGetFrame
/// uses for frame indexes. One more than `sprite_num_frames` because a few
/// broken sprites (the "Bloody-Mouth Bug" hack in the C code) store a valid
/// frame in the end-marker slot.
pub fn sprite_frame_count(sprite: &[u8]) -> usize {
    if sprite.len() < 2 {
        0
    } else {
        u16_le(sprite, 0) as usize
    }
}

/// Get the RLE data of frame `n` of a sprite.
pub fn sprite_frame(sprite: &[u8], n: usize) -> Option<&[u8]> {
    let count = sprite_frame_count(sprite);
    if n >= count {
        return None;
    }
    let idx = n * 2;
    if sprite.len() < idx + 2 {
        return None;
    }
    let mut offset = (u16_le(sprite, idx) as usize) << 1;
    // C: `if (offset == 0x18444) offset = (WORD)offset;` — hack for broken
    // sprites like the Bloody-Mouth Bug, where the DOS engine's 16-bit
    // arithmetic wrapped this offset.
    if offset == 0x18444 {
        offset &= 0xFFFF;
    }
    if offset >= sprite.len() {
        return None;
    }
    Some(&sprite[offset..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_basics() {
        let mut s = Surface::new(320, 200);
        s.clear(7);
        assert_eq!(s.get_pixel(10, 10), 7);
        s.put_pixel(5, 6, 200);
        assert_eq!(s.get_pixel(5, 6), 200);
        s.put_pixel(-1, 0, 9); // clipped, must not panic
        s.fill_rect(0, 0, 4, 4, 3);
        assert_eq!(s.get_pixel(3, 3), 3);
    }

    #[test]
    fn rle_roundtrip_literals_and_skips() {
        // Build a tiny RLE bitmap 4x2: pixels 1..8 with row wrap.
        // Header: w=4, h=2, then one literal run of 8 (max run 127 ok).
        let mut rle = vec![4, 0, 2, 0, 8];
        rle.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut s = Surface::new(8, 4);
        s.blit_rle(&rle, 0, 0);
        assert_eq!(s.get_pixel(0, 0), 1);
        assert_eq!(s.get_pixel(3, 0), 4);
        assert_eq!(s.get_pixel(0, 1), 5);
        assert_eq!(s.get_pixel(3, 1), 8);
        // skip run: skip 2, draw 1 pixel, skip rest
        let rle2 = vec![4, 0, 2, 0, 0x82, 1, 9];
        let mut s2 = Surface::new(8, 4);
        s2.blit_rle(&rle2, 1, 1);
        assert_eq!(s2.get_pixel(3, 1), 9);
        assert_eq!(s2.get_pixel(1, 1), 0);
    }

    #[test]
    fn sprite_frames() {
        // Sprite layout: [count_lo, count_hi, table...]. Frame n's table entry
        // is the u16 at byte offset n*2 (frame 0 shares the count field), and
        // the frame data offset is entry<<1.
        let mut sp = vec![2, 0, 6, 0]; // count=2 (frame0 at 2<<1=4), frame1 at 6<<1=12
        sp.extend_from_slice(&[1, 0, 1, 0, 1, 42, 0, 0]); // frame0 at 4: 1x1, pixel 42
        sp.extend_from_slice(&[1, 0, 1, 0, 1, 43]); // frame1 at 12: 1x1, pixel 43
        assert_eq!(sprite_frame_count(&sp), 2);
        let f0 = sprite_frame(&sp, 0).unwrap();
        assert_eq!(rle_width(f0), 1);
        let mut s = Surface::new(4, 4);
        s.blit_rle(f0, 0, 0);
        assert_eq!(s.get_pixel(0, 0), 42);
        let f1 = sprite_frame(&sp, 1).unwrap();
        let mut s2 = Surface::new(4, 4);
        s2.blit_rle(f1, 0, 0);
        assert_eq!(s2.get_pixel(0, 0), 43);
        assert!(sprite_frame(&sp, 2).is_none());

        // PAL_SpriteGetNumFrames: the last table slot is an end marker, not a
        // frame — the real frame count is word[0]-1. sprite_frame still allows
        // reading the marker slot (the C "Bloody-Mouth Bug" hack), but
        // animation moduli must use sprite_num_frames.
        assert_eq!(sprite_num_frames(&sp), 1);
        assert_eq!(sprite_num_frames(&[]), 0);
        assert_eq!(sprite_num_frames(&[0, 0]), 0);
    }

    #[test]
    fn sprite_frame_broken_offset_hack() {
        // C: `if (offset == 0x18444) offset = (WORD)offset;` — the offset
        // word 0xC222 (<<1 = 0x18444) wraps to 0x8444 like the DOS engine's
        // 16-bit arithmetic did.
        let mut sp = vec![0u8; 0x8444 + 6];
        sp[0] = 2; // count = 2
        sp[2] = 0x22; // frame 1 offset word = 0xC222
        sp[3] = 0xC2;
        sp[0x8444] = 1; // 1x1 frame: w=1, h=1, run of 1 pixel with value 7
        sp[0x8446] = 1;
        sp[0x8448] = 1;
        sp[0x8449] = 7;
        let f = sprite_frame(&sp, 1).expect("hack offset resolves");
        assert_eq!(rle_width(f), 1);
        let mut s = Surface::new(2, 2);
        s.blit_rle(f, 0, 0);
        assert_eq!(s.get_pixel(0, 0), 7);

        // Any other out-of-range offset still yields None.
        let mut sp2 = vec![2, 0, 0xFF, 0xFF];
        sp2.extend_from_slice(&[1, 0, 1, 0, 1, 9]);
        assert!(sprite_frame(&sp2, 1).is_none());
    }
}
