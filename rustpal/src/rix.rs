//! RIX (Softstar AdLib) music player. STUB - to be implemented as a Rust port
//! of SDLPAL rixplay.cpp on top of `opl::Opl`.
#![allow(dead_code)]

pub struct RixPlayer;

impl RixPlayer {
    /// Load a RIX song (a raw chunk from mus.mkf). Returns None if the data
    /// is not a valid RIX song. `opl_rate` is the audio sample rate the
    /// internal OPL chip runs at (must match the mixer output rate).
    pub fn new(_song: &[u8], _opl_rate: u32) -> Option<RixPlayer> {
        unimplemented!()
    }

    /// Render `out.len()` stereo samples, advancing the song.
    pub fn render(&mut self, _out: &mut [[i16; 2]]) {
        unimplemented!()
    }
}
