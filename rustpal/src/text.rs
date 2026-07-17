//! Game text resources: WORD.DAT (word list) and M.MSG (dialogue messages).
//! STUB - to be implemented (see SDLPAL text.c).
//!
//! All text stays in the game's native byte encoding (GB2312 for this data
//! set), ready for `font::Font::draw_text`.
#![allow(dead_code)]

use std::io;

use crate::data::DataDir;

pub struct Texts;

impl Texts {
    /// Load WORD.DAT and M.MSG from the data dir.
    pub fn load(_dir: &DataDir) -> io::Result<Texts> {
        unimplemented!()
    }

    /// Word number `n` (1-based as in the scripts), control codes stripped.
    pub fn word(&self, _n: usize) -> Vec<u8> {
        unimplemented!()
    }

    /// Message number `n`, decrypted and with control codes stripped.
    pub fn msg(&self, _n: usize) -> Vec<u8> {
        unimplemented!()
    }
}
