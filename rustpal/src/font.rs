//! Font rendering (wor16.asc ASCII glyphs + wor16.fon CJK glyphs).
//! STUB - to be implemented (see SDLPAL font.c / text.c).
//!
//! Text is handled in the game's native byte encoding (GB2312 for the
//! simplified-Chinese data set): 1-byte ASCII chars, 2-byte CJK chars.
#![allow(dead_code)]

use std::io;

use crate::data::DataDir;
use crate::surface::Surface;

pub struct Font;

impl Font {
    /// Load wor16.asc / wor16.fon from the data dir.
    pub fn load(_dir: &DataDir) -> io::Result<Font> {
        unimplemented!()
    }

    /// Draw raw game-encoded text at (x, y) with the given palette index.
    /// Returns the x coordinate just past the drawn text.
    pub fn draw_text(
        &self,
        _surf: &mut Surface,
        _text: &[u8],
        _x: i32,
        _y: i32,
        _color: u8,
        _shadow: bool,
    ) -> i32 {
        unimplemented!()
    }

    /// Pixel width of a text string as it would be drawn.
    pub fn text_width(&self, _text: &[u8]) -> i32 {
        unimplemented!()
    }
}
