//! OPL3 (YMF262) emulator. STUB - to be implemented as a Rust port of the
//! DOSBox DBOPL core (SDLPAL adplug/dosbox/dbopl.h + dbopl.cpp.h).
#![allow(dead_code)]

/// OPL3 chip emulator producing stereo 16-bit samples.
pub struct Opl;

impl Opl {
    /// Create a chip generating audio at `rate` Hz (e.g. 44100/49716).
    pub fn new(_rate: u32) -> Opl {
        unimplemented!()
    }

    /// Write `val` to register `addr` (0x000..=0x1FF, both register banks).
    pub fn write(&mut self, _addr: u16, _val: u8) {
        unimplemented!()
    }

    /// Generate `out.len()` stereo samples.
    pub fn generate(&mut self, _out: &mut [[i16; 2]]) {
        unimplemented!()
    }
}
