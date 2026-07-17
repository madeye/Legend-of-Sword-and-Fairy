//! VGA palettes from PAT.MKF (port of PAL_GetPalette in palette.c).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;

use crate::mkf::Mkf;

pub struct Palette {
    /// 8-bit RGB triples.
    pub colors: [[u8; 3]; 256],
}

impl Palette {
    /// Load palette `num` from pat.mkf. Chunks hold 256 RGB triples in VGA
    /// 6-bit format, optionally followed by a second (night) palette.
    pub fn from_mkf(mkf: &Mkf, num: usize, night: bool) -> io::Result<Palette> {
        let buf = mkf.chunk(num)?;
        let mut colors = [[0xffu8; 3]; 256];
        let has_night = buf.len() > 256 * 3;
        let base = if night && has_night { 256 * 3 } else { 0 };
        for (i, color) in colors.iter_mut().enumerate() {
            let o = base + i * 3;
            if o + 2 >= buf.len() {
                break;
            }
            *color = [buf[o] << 2, buf[o + 1] << 2, buf[o + 2] << 2];
        }
        Ok(Palette { colors })
    }

    pub fn black() -> Palette {
        Palette {
            colors: [[0; 3]; 256],
        }
    }
}
