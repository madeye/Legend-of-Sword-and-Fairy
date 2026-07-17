//! Audio mixer: RIX music + VOC sound effects -> stereo i16 output.
//! STUB - to be implemented.
#![allow(dead_code)]

use crate::rix::RixPlayer;
use crate::voc::VocSound;

/// Software mixer. Everything is rendered at the output device's sample rate.
pub struct Mixer;

impl Mixer {
    pub fn new(_out_rate: u32) -> Mixer {
        unimplemented!()
    }

    /// Start playing a RIX song (replaces any current song).
    pub fn play_music(&mut self, _rix: RixPlayer) {
        unimplemented!()
    }

    pub fn stop_music(&mut self) {
        unimplemented!()
    }

    /// Fire-and-forget playback of a decoded sound effect.
    pub fn play_sound(&mut self, _voc: VocSound) {
        unimplemented!()
    }

    /// Render `out.len()` interleaved stereo i16 samples (L,R,L,R,...).
    /// Called from the audio callback; must be allocation-free and fast.
    pub fn render(&mut self, _out: &mut [i16]) {
        unimplemented!()
    }
}
