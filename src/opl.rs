//! OPL2/OPL3 (YM3812 / YMF262) emulator. A faithful Rust port of the DOSBox
//! DBOPL core (SDLPAL `adplug/dosbox/dbopl.h` + `dbopl.cpp.h`), using the
//! `WAVE_TABLEMUL` wave generator like the SDLPAL build.
//!
//! The public [`Opl`] wraps a [`Chip`] in the OPL2 configuration used by PAL:
//! `GenerateBlock2` produces one 32-bit mono stream, each sample is clipped to
//! `i16` and duplicated to both stereo channels. This mirrors SDLPAL's
//! `DBINTOPL2::Generate` followed by `CConvertopl::update_16m_16s`.
#![allow(dead_code)]
// used incrementally as engine bring-up proceeds
// This is a deliberately literal port of the DOSBox C++ source: constant
// expressions, C-style indexed loops and bit-twiddling are kept verbatim so the
// output is bit-exact, at the cost of some idiom lints.
#![allow(
    clippy::identity_op,
    clippy::eq_op,
    clippy::needless_range_loop,
    clippy::manual_div_ceil,
    clippy::erasing_op,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::implicit_saturating_sub
)]

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Constants (mirrors the #defines in dbopl.cpp.h with DBOPL_WAVE == TABLEMUL)
// ---------------------------------------------------------------------------

const OPLRATE: f64 = 14318180.0 / 288.0;
const TREMOLO_TABLE: usize = 52;

const WAVE_BITS: u32 = 10;
const WAVE_SH: u32 = 32 - WAVE_BITS;
const WAVE_MASK: u32 = (1 << WAVE_SH) - 1;

const LFO_SH: u32 = WAVE_SH - 10;
const LFO_MAX: u32 = 256 << LFO_SH;

const ENV_BITS: u32 = 9;
const ENV_EXTRA: u32 = ENV_BITS - 9; // == 0
const ENV_MIN: i32 = 0;
const ENV_MAX: i32 = 511 << ENV_EXTRA; // == 511
const ENV_LIMIT: u32 = (12 * 256) >> (3 - ENV_EXTRA); // == 384

const RATE_SH: u32 = 24;
const RATE_MASK: u32 = (1 << RATE_SH) - 1;
const MUL_SH: u32 = 16;

// Operator register 0x20 masks
const MASK_KSR: u8 = 0x10;
const MASK_SUSTAIN: u8 = 0x20;
const MASK_VIBRATO: u8 = 0x40;
#[allow(dead_code)]
const MASK_TREMOLO: u8 = 0x80;

// Envelope states
const OFF: u8 = 0;
const RELEASE: u8 = 1;
const SUSTAIN: u8 = 2;
const DECAY: u8 = 3;
const ATTACK: u8 = 4;

// chandata shifts
const SHIFT_KSLBASE: u32 = 16;
const SHIFT_KEYCODE: u32 = 24;

// Synth modes (order matters for the `> sm4Start` / `> sm6Start` comparisons)
const SM2AM: u8 = 0;
const SM2FM: u8 = 1;
const SM3AM: u8 = 2;
const SM3FM: u8 = 3;
const SM4START: u8 = 4;
const SM3FMFM: u8 = 5;
const SM3AMFM: u8 = 6;
const SM3FMAM: u8 = 7;
const SM3AMAM: u8 = 8;
const SM6START: u8 = 9;
const SM2PERCUSSION: u8 = 10;
const SM3PERCUSSION: u8 = 11;

#[inline]
fn env_silent(x: i32) -> bool {
    x >= ENV_LIMIT as i32
}

// ---------------------------------------------------------------------------
// Const generator tables
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const KSL_CREATE_TABLE: [u8; 16] = [
    64, 32, 24, 19,
    16, 12, 11, 10,
     8,  6,  5,  4,
     3,  2,  1,  0,
];

#[rustfmt::skip]
const FREQ_CREATE_TABLE: [u8; 16] = [
    // M(x) == (u8)(x * 2)
    (0.5 * 2.0) as u8, (1.0 * 2.0) as u8, (2.0 * 2.0) as u8, (3.0 * 2.0) as u8,
    (4.0 * 2.0) as u8, (5.0 * 2.0) as u8, (6.0 * 2.0) as u8, (7.0 * 2.0) as u8,
    (8.0 * 2.0) as u8, (9.0 * 2.0) as u8, (10.0 * 2.0) as u8, (10.0 * 2.0) as u8,
    (12.0 * 2.0) as u8, (12.0 * 2.0) as u8, (15.0 * 2.0) as u8, (15.0 * 2.0) as u8,
];

#[rustfmt::skip]
const ATTACK_SAMPLES_TABLE: [u8; 13] = [
    69, 55, 46, 40,
    35, 29, 23, 20,
    19, 15, 11, 10,
    9,
];

#[rustfmt::skip]
const ENVELOPE_INCREASE_TABLE: [u8; 13] = [
    4,  5,  6,  7,
    8, 10, 12, 14,
    16, 20, 24, 28,
    32,
];

#[rustfmt::skip]
const WAVE_BASE_TABLE: [u16; 8] = [
    0x000, 0x200, 0x200, 0x800,
    0xa00, 0xc00, 0x100, 0x400,
];

#[rustfmt::skip]
const WAVE_MASK_TABLE: [u16; 8] = [
    1023, 1023, 511, 511,
    1023, 1023, 512, 1023,
];

#[rustfmt::skip]
const WAVE_START_TABLE: [u16; 8] = [
    512, 0, 0, 0,
    0, 512, 512, 256,
];

#[rustfmt::skip]
const VIBRATO_TABLE: [i8; 8] = [
    (1i16 - 0x00) as i8, (0i16 - 0x00) as i8, (1i16 - 0x00) as i8, (30i16 - 0x00) as i8,
    (1i16 - 0x80) as i8, (0i16 - 0x80) as i8, (1i16 - 0x80) as i8, (30i16 - 0x80) as i8,
];

const KSL_SHIFT_TABLE: [u8; 4] = [31, 1, 2, 0];

fn envelope_select(val: u8) -> (u8, u8) {
    // returns (index, shift)
    if val < 13 * 4 {
        (val & 3, 12 - (val >> 2))
    } else if val < 15 * 4 {
        (val - 12 * 4, 0)
    } else {
        (12, 0)
    }
}

// ---------------------------------------------------------------------------
// Globally-initialised tables (InitTables in dbopl.cpp.h)
// ---------------------------------------------------------------------------

struct Tables {
    wave_table: [i16; 8 * 512],
    mul_table: [u16; 384],
    ksl_table: [u8; 8 * 16],
    tremolo_table: [u8; TREMOLO_TABLE],
    // Register-index -> channel / operator resolution.
    chan_map: [i16; 32], // channel index, or -1 when absent
    op_chan: [i16; 64],  // operator's channel index, or -1
    op_sub: [u8; 64],    // operator index within the channel (0/1)
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(init_tables)
}

fn init_tables() -> Tables {
    let mut wave_table = [0i16; 8 * 512];
    let mut mul_table = [0u16; 384];
    let mut ksl_table = [0u8; 8 * 16];
    let mut tremolo_table = [0u8; TREMOLO_TABLE];

    // Multiplication based tables
    for i in 0..384 {
        let s = i as i32 * 8;
        let val =
            0.5 + 2f64.powf(-1.0 + (255 - s) as f64 * (1.0 / 256.0)) * (1u32 << MUL_SH) as f64;
        mul_table[i] = val as u16;
    }

    // Sine wave base
    for i in 0..512 {
        let v = (((i as f64 + 0.5) * (std::f64::consts::PI / 512.0)).sin() * 4084.0) as i16;
        wave_table[0x0200 + i] = v;
        wave_table[i] = -wave_table[0x200 + i];
    }
    // Exponential wave
    for i in 0..256 {
        let v =
            (0.5 + 2f64.powf(-1.0 + (255 - i as i32 * 8) as f64 * (1.0 / 256.0)) * 4085.0) as i16;
        wave_table[0x700 + i] = v;
        wave_table[0x6ff - i] = -wave_table[0x700 + i];
    }

    // Fill silence / replicate waves
    for i in 0..256 {
        wave_table[0x400 + i] = wave_table[0];
        wave_table[0x500 + i] = wave_table[0];
        wave_table[0x900 + i] = wave_table[0];
        wave_table[0xc00 + i] = wave_table[0];
        wave_table[0xd00 + i] = wave_table[0];
        wave_table[0x800 + i] = wave_table[0x200 + i];
        wave_table[0xa00 + i] = wave_table[0x200 + i * 2];
        wave_table[0xb00 + i] = wave_table[i * 2];
        wave_table[0xe00 + i] = wave_table[0x200 + i * 2];
        wave_table[0xf00 + i] = wave_table[0x200 + i * 2];
    }

    // KSL table
    for oct in 0..8 {
        let base = oct as i32 * 8;
        for i in 0..16 {
            let mut val = base - KSL_CREATE_TABLE[i] as i32;
            if val < 0 {
                val = 0;
            }
            ksl_table[oct * 16 + i] = (val * 4) as u8;
        }
    }

    // Tremolo table (triangle wave)
    for i in 0..(TREMOLO_TABLE / 2) {
        let val = (i as u8) << ENV_EXTRA;
        tremolo_table[i] = val;
        tremolo_table[TREMOLO_TABLE - 1 - i] = val;
    }

    // Channel offset resolution table
    let mut chan_map = [-1i16; 32];
    for i in 0..32usize {
        let mut index = i & 0xf;
        if index >= 9 {
            chan_map[i] = -1;
            continue;
        }
        if index < 6 {
            index = (index % 3) * 2 + (index / 3);
        }
        if i >= 16 {
            index += 9;
        }
        chan_map[i] = index as i16;
    }

    // Operator offset resolution table
    let mut op_chan = [-1i16; 64];
    let mut op_sub = [0u8; 64];
    for i in 0..64usize {
        if i % 8 >= 6 || (i / 8) % 4 == 3 {
            op_chan[i] = -1;
            continue;
        }
        let mut ch_num = (i / 8) * 3 + (i % 8) % 3;
        if ch_num >= 12 {
            ch_num += 16 - 12;
        }
        let op_num = (i % 8) / 3;
        op_chan[i] = chan_map[ch_num];
        op_sub[i] = op_num as u8;
    }

    Tables {
        wave_table,
        mul_table,
        ksl_table,
        tremolo_table,
        chan_map,
        op_chan,
        op_sub,
    }
}

// ---------------------------------------------------------------------------
// Operator
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Operator {
    wave_base: u32, // start offset into WaveTable
    wave_mask: u32,
    wave_start: u32,
    wave_index: u32,
    wave_add: u32,
    wave_current: u32,

    chan_data: u32,
    freq_mul: u32,
    vibrato: u32,
    sustain_level: i32,
    total_level: i32,
    current_level: u32,
    volume: i32,

    attack_add: u32,
    decay_add: u32,
    release_add: u32,
    rate_index: u32,

    rate_zero: u8,
    key_on: u8,
    reg20: u8,
    reg40: u8,
    reg60: u8,
    reg80: u8,
    reg_e0: u8,
    state: u8,
    tremolo_mask: u8,
    vib_strength: u8,
    ksr: u8,
}

impl Operator {
    fn new() -> Operator {
        Operator {
            wave_base: 0,
            wave_mask: 0,
            wave_start: 0,
            wave_index: 0,
            wave_add: 0,
            wave_current: 0,
            chan_data: 0,
            freq_mul: 0,
            vibrato: 0,
            sustain_level: ENV_MAX,
            total_level: ENV_MAX,
            current_level: ENV_MAX as u32,
            volume: ENV_MAX,
            attack_add: 0,
            decay_add: 0,
            release_add: 0,
            rate_index: 0,
            rate_zero: 1 << OFF,
            key_on: 0,
            reg20: 0,
            reg40: 0,
            reg60: 0,
            reg80: 0,
            reg_e0: 0,
            state: OFF,
            tremolo_mask: 0,
            vib_strength: 0,
            ksr: 0,
        }
    }

    #[inline]
    fn set_state(&mut self, s: u8) {
        self.state = s;
    }

    fn update_attack(&mut self, attack_rates: &[u32; 76]) {
        let rate = self.reg60 >> 4;
        if rate != 0 {
            let val = ((rate << 2) + self.ksr) as usize;
            self.attack_add = attack_rates[val];
            self.rate_zero &= !(1 << ATTACK);
        } else {
            self.attack_add = 0;
            self.rate_zero |= 1 << ATTACK;
        }
    }

    fn update_decay(&mut self, linear_rates: &[u32; 76]) {
        let rate = self.reg60 & 0xf;
        if rate != 0 {
            let val = ((rate << 2) + self.ksr) as usize;
            self.decay_add = linear_rates[val];
            self.rate_zero &= !(1 << DECAY);
        } else {
            self.decay_add = 0;
            self.rate_zero |= 1 << DECAY;
        }
    }

    fn update_release(&mut self, linear_rates: &[u32; 76]) {
        let rate = self.reg80 & 0xf;
        if rate != 0 {
            let val = ((rate << 2) + self.ksr) as usize;
            self.release_add = linear_rates[val];
            self.rate_zero &= !(1 << RELEASE);
            if self.reg20 & MASK_SUSTAIN == 0 {
                self.rate_zero &= !(1 << SUSTAIN);
            }
        } else {
            self.rate_zero |= 1 << RELEASE;
            self.release_add = 0;
            if self.reg20 & MASK_SUSTAIN == 0 {
                self.rate_zero |= 1 << SUSTAIN;
            }
        }
    }

    fn update_attenuation(&mut self) {
        let ksl_base = ((self.chan_data >> SHIFT_KSLBASE) & 0xff) as u8;
        let tl = self.reg40 & 0x3f;
        let ksl_shift = KSL_SHIFT_TABLE[(self.reg40 >> 6) as usize];
        self.total_level = (tl << (ENV_BITS - 7)) as i32;
        self.total_level += (((ksl_base as u32) << ENV_EXTRA) >> ksl_shift) as i32;
    }

    fn update_frequency(&mut self) {
        let freq = self.chan_data & ((1 << 10) - 1);
        let block = (self.chan_data >> 10) & 0xff;
        self.wave_add = (freq << block).wrapping_mul(self.freq_mul);
        if self.reg20 & MASK_VIBRATO != 0 {
            self.vib_strength = (freq >> 7) as u8;
            self.vibrato = ((self.vib_strength as u32) << block).wrapping_mul(self.freq_mul);
        } else {
            self.vib_strength = 0;
            self.vibrato = 0;
        }
    }

    fn update_rates(&mut self, attack_rates: &[u32; 76], linear_rates: &[u32; 76]) {
        let mut new_ksr = ((self.chan_data >> SHIFT_KEYCODE) & 0xff) as u8;
        if self.reg20 & MASK_KSR == 0 {
            new_ksr >>= 2;
        }
        if self.ksr == new_ksr {
            return;
        }
        self.ksr = new_ksr;
        self.update_attack(attack_rates);
        self.update_decay(linear_rates);
        self.update_release(linear_rates);
    }

    #[inline]
    fn rate_forward(&mut self, add: u32) -> i32 {
        self.rate_index = self.rate_index.wrapping_add(add);
        let ret = (self.rate_index >> RATE_SH) as i32;
        self.rate_index &= RATE_MASK;
        ret
    }

    fn template_volume(&mut self) -> i32 {
        let mut vol = self.volume;
        match self.state {
            OFF => ENV_MAX,
            ATTACK => {
                let change = self.rate_forward(self.attack_add);
                if change == 0 {
                    return vol;
                }
                vol += ((!vol) * change) >> 3;
                if vol < ENV_MIN {
                    self.volume = ENV_MIN;
                    self.rate_index = 0;
                    self.set_state(DECAY);
                    return ENV_MIN;
                }
                self.volume = vol;
                vol
            }
            DECAY => {
                vol += self.rate_forward(self.decay_add);
                if vol >= self.sustain_level {
                    if vol >= ENV_MAX {
                        self.volume = ENV_MAX;
                        self.set_state(OFF);
                        return ENV_MAX;
                    }
                    self.rate_index = 0;
                    self.set_state(SUSTAIN);
                }
                self.volume = vol;
                vol
            }
            SUSTAIN => {
                if self.reg20 & MASK_SUSTAIN != 0 {
                    return vol;
                }
                // fall through to release behaviour
                vol += self.rate_forward(self.release_add);
                if vol >= ENV_MAX {
                    self.volume = ENV_MAX;
                    self.set_state(OFF);
                    return ENV_MAX;
                }
                self.volume = vol;
                vol
            }
            RELEASE => {
                vol += self.rate_forward(self.release_add);
                if vol >= ENV_MAX {
                    self.volume = ENV_MAX;
                    self.set_state(OFF);
                    return ENV_MAX;
                }
                self.volume = vol;
                vol
            }
            _ => ENV_MAX,
        }
    }

    #[inline]
    fn forward_volume(&mut self) -> u32 {
        self.current_level
            .wrapping_add(self.template_volume() as u32)
    }

    #[inline]
    fn forward_wave(&mut self) -> u32 {
        self.wave_index = self.wave_index.wrapping_add(self.wave_current);
        self.wave_index >> WAVE_SH
    }

    fn write20(
        &mut self,
        val: u8,
        freq_mul: &[u32; 16],
        attack_rates: &[u32; 76],
        linear_rates: &[u32; 76],
    ) {
        let change = self.reg20 ^ val;
        if change == 0 {
            return;
        }
        self.reg20 = val;
        self.tremolo_mask = ((val as i8) >> 7) as u8;
        self.tremolo_mask &= !((1u8 << ENV_EXTRA) - 1);
        if change & MASK_KSR != 0 {
            self.update_rates(attack_rates, linear_rates);
        }
        if self.reg20 & MASK_SUSTAIN != 0 || self.release_add == 0 {
            self.rate_zero |= 1 << SUSTAIN;
        } else {
            self.rate_zero &= !(1 << SUSTAIN);
        }
        if change & (0xf | MASK_VIBRATO) != 0 {
            self.freq_mul = freq_mul[(val & 0xf) as usize];
            self.update_frequency();
        }
    }

    fn write40(&mut self, val: u8) {
        if (self.reg40 ^ val) == 0 {
            return;
        }
        self.reg40 = val;
        self.update_attenuation();
    }

    fn write60(&mut self, val: u8, attack_rates: &[u32; 76], linear_rates: &[u32; 76]) {
        let change = self.reg60 ^ val;
        self.reg60 = val;
        if change & 0x0f != 0 {
            self.update_decay(linear_rates);
        }
        if change & 0xf0 != 0 {
            self.update_attack(attack_rates);
        }
    }

    fn write80(&mut self, val: u8, linear_rates: &[u32; 76]) {
        let change = self.reg80 ^ val;
        if change == 0 {
            return;
        }
        self.reg80 = val;
        let mut sustain = val >> 4;
        sustain |= (sustain + 1) & 0x10;
        self.sustain_level = (sustain as i32) << (ENV_BITS - 5);
        if change & 0x0f != 0 {
            self.update_release(linear_rates);
        }
    }

    fn write_e0(&mut self, val: u8, wave_form_mask: u8, opl3_active: u8) {
        if self.reg_e0 ^ val == 0 {
            return;
        }
        let wave_form = val & ((0x3 & wave_form_mask) | (0x7 & opl3_active));
        self.reg_e0 = val;
        self.wave_base = WAVE_BASE_TABLE[wave_form as usize] as u32;
        self.wave_start = (WAVE_START_TABLE[wave_form as usize] as u32) << WAVE_SH;
        self.wave_mask = WAVE_MASK_TABLE[wave_form as usize] as u32;
    }

    #[inline]
    fn silent(&self) -> bool {
        if !env_silent(self.total_level + self.volume) {
            return false;
        }
        if self.rate_zero & (1 << self.state) == 0 {
            return false;
        }
        true
    }

    fn prepare(&mut self, tremolo_value: u8, vibrato_shift: u8, vibrato_sign: i8) {
        self.current_level =
            (self.total_level as u32).wrapping_add((tremolo_value & self.tremolo_mask) as u32);
        self.wave_current = self.wave_add;
        if (self.vib_strength as u32 >> vibrato_shift) != 0 {
            let add = (self.vibrato >> vibrato_shift) as i32;
            let neg = vibrato_sign as i32;
            let add = (add ^ neg) - neg;
            self.wave_current = self.wave_current.wrapping_add(add as u32);
        }
    }

    fn key_on(&mut self, mask: u8) {
        if self.key_on == 0 {
            self.wave_index = self.wave_start;
            self.rate_index = 0;
            self.set_state(ATTACK);
        }
        self.key_on |= mask;
    }

    fn key_off(&mut self, mask: u8) {
        self.key_on &= !mask;
        if self.key_on == 0 && self.state != OFF {
            self.set_state(RELEASE);
        }
    }

    #[inline]
    fn get_wave(&self, index: u32, vol: u32) -> i32 {
        let t = tables();
        let w = t.wave_table[(self.wave_base + (index & self.wave_mask)) as usize] as i32;
        let m = t.mul_table[(vol >> ENV_EXTRA) as usize] as i32;
        (w * m) >> MUL_SH
    }

    #[inline]
    fn get_sample(&mut self, modulation: i32) -> i32 {
        let vol = self.forward_volume();
        if env_silent(vol as i32) {
            self.wave_index = self.wave_index.wrapping_add(self.wave_current);
            0
        } else {
            let index = self.forward_wave().wrapping_add(modulation as u32);
            self.get_wave(index, vol)
        }
    }
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Channel {
    op: [Operator; 2],
    synth_mode: u8,
    chan_data: u32,
    old: [i32; 2],
    feedback: u8,
    reg_b0: u8,
    reg_c0: u8,
    four_mask: u8,
    mask_left: i32,
    mask_right: i32,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            op: [Operator::new(), Operator::new()],
            synth_mode: SM2FM,
            chan_data: 0,
            old: [0, 0],
            feedback: 31,
            reg_b0: 0,
            reg_c0: 0,
            four_mask: 0,
            mask_left: -1,
            mask_right: -1,
        }
    }
}

// ---------------------------------------------------------------------------
// Chip
// ---------------------------------------------------------------------------

struct Chip {
    lfo_counter: u32,
    lfo_add: u32,
    noise_counter: u32,
    noise_add: u32,
    noise_value: u32,

    freq_mul: [u32; 16],
    linear_rates: [u32; 76],
    attack_rates: [u32; 76],

    chan: Vec<Channel>, // 18 channels

    reg104: u8,
    reg08: u8,
    reg04: u8,
    reg_bd: u8,
    vibrato_index: u8,
    tremolo_index: u8,
    vibrato_sign: i8,
    vibrato_shift: u8,
    tremolo_value: u8,
    vibrato_strength: u8,
    tremolo_strength: u8,
    wave_form_mask: u8,
    opl3_active: u8, // 0x00 or 0xff
}

#[inline]
fn opref(ci: usize, index: usize) -> (usize, usize) {
    (ci + (index >> 1), index & 1)
}

impl Chip {
    fn new() -> Chip {
        Chip {
            lfo_counter: 0,
            lfo_add: 0,
            noise_counter: 0,
            noise_add: 0,
            noise_value: 0,
            freq_mul: [0; 16],
            linear_rates: [0; 76],
            attack_rates: [0; 76],
            chan: vec![Channel::new(); 18],
            reg104: 0,
            reg08: 0,
            reg04: 0,
            reg_bd: 0,
            vibrato_index: 0,
            tremolo_index: 0,
            vibrato_sign: 0,
            vibrato_shift: 0,
            tremolo_value: 0,
            vibrato_strength: 0,
            tremolo_strength: 0,
            wave_form_mask: 0,
            opl3_active: 0,
        }
    }

    // ------- operator helpers (resolve borrow-splitting) -------

    fn op_update_rates(&mut self, ci: usize, k: usize) {
        let Chip {
            chan,
            attack_rates,
            linear_rates,
            ..
        } = self;
        chan[ci].op[k].update_rates(attack_rates, linear_rates);
    }

    fn op_get_sample(&mut self, ci: usize, index: usize, modulation: i32) -> i32 {
        let (cc, k) = opref(ci, index);
        self.chan[cc].op[k].get_sample(modulation)
    }

    fn op_forward_wave(&mut self, ci: usize, index: usize) -> u32 {
        let (cc, k) = opref(ci, index);
        self.chan[cc].op[k].forward_wave()
    }

    fn op_forward_volume(&mut self, ci: usize, index: usize) -> u32 {
        let (cc, k) = opref(ci, index);
        self.chan[cc].op[k].forward_volume()
    }

    fn op_get_wave(&self, ci: usize, index: usize, wave_index: u32, vol: u32) -> i32 {
        let (cc, k) = opref(ci, index);
        self.chan[cc].op[k].get_wave(wave_index, vol)
    }

    fn op_silent(&self, ci: usize, index: usize) -> bool {
        let (cc, k) = opref(ci, index);
        self.chan[cc].op[k].silent()
    }

    fn op_prepare(&mut self, ci: usize, index: usize) {
        let (cc, k) = opref(ci, index);
        let tv = self.tremolo_value;
        let vsh = self.vibrato_shift;
        let vsi = self.vibrato_sign;
        self.chan[cc].op[k].prepare(tv, vsh, vsi);
    }

    // ------- channel helpers -------

    fn set_chan_data(&mut self, ci: usize, data: u32) {
        let change = self.chan[ci].chan_data ^ data;
        self.chan[ci].chan_data = data;
        self.chan[ci].op[0].chan_data = data;
        self.chan[ci].op[1].chan_data = data;
        self.chan[ci].op[0].update_frequency();
        self.chan[ci].op[1].update_frequency();
        if change & (0xff << SHIFT_KSLBASE) != 0 {
            self.chan[ci].op[0].update_attenuation();
            self.chan[ci].op[1].update_attenuation();
        }
        if change & (0xff << SHIFT_KEYCODE) != 0 {
            self.op_update_rates(ci, 0);
            self.op_update_rates(ci, 1);
        }
    }

    fn update_channel_frequency(&mut self, ci: usize, four_op: u8) {
        let data0 = self.chan[ci].chan_data & 0xffff;
        let ksl_base = tables().ksl_table[(data0 >> 6) as usize] as u32;
        let mut key_code = (data0 & 0x1c00) >> 9;
        if self.reg08 & 0x40 != 0 {
            key_code |= (data0 & 0x100) >> 8;
        } else {
            key_code |= (data0 & 0x200) >> 9;
        }
        let data = data0 | (key_code << SHIFT_KEYCODE) | (ksl_base << SHIFT_KSLBASE);
        self.set_chan_data(ci, data);
        if four_op & 0x3f != 0 {
            self.set_chan_data(ci + 1, data);
        }
    }

    fn write_a0(&mut self, ci: usize, val: u8) {
        let four_op = self.reg104 & self.opl3_active & self.chan[ci].four_mask;
        if four_op > 0x80 {
            return;
        }
        let change = (self.chan[ci].chan_data ^ val as u32) & 0xff;
        if change != 0 {
            self.chan[ci].chan_data ^= change;
            self.update_channel_frequency(ci, four_op);
        }
    }

    fn write_b0(&mut self, ci: usize, val: u8) {
        let four_op = self.reg104 & self.opl3_active & self.chan[ci].four_mask;
        if four_op > 0x80 {
            return;
        }
        let change = (self.chan[ci].chan_data ^ ((val as u32) << 8)) & 0x1f00;
        if change != 0 {
            self.chan[ci].chan_data ^= change;
            self.update_channel_frequency(ci, four_op);
        }
        if (val ^ self.chan[ci].reg_b0) & 0x20 == 0 {
            return;
        }
        self.chan[ci].reg_b0 = val;
        if val & 0x20 != 0 {
            self.chan[ci].op[0].key_on(0x1);
            self.chan[ci].op[1].key_on(0x1);
            if four_op & 0x3f != 0 {
                self.chan[ci + 1].op[0].key_on(1);
                self.chan[ci + 1].op[1].key_on(1);
            }
        } else {
            self.chan[ci].op[0].key_off(0x1);
            self.chan[ci].op[1].key_off(0x1);
            if four_op & 0x3f != 0 {
                self.chan[ci + 1].op[0].key_off(1);
                self.chan[ci + 1].op[1].key_off(1);
            }
        }
    }

    fn write_c0(&mut self, ci: usize, val: u8) {
        let change = val ^ self.chan[ci].reg_c0;
        if change == 0 {
            return;
        }
        self.chan[ci].reg_c0 = val;
        let mut feedback = (val >> 1) & 7;
        if feedback != 0 {
            feedback = 9 - feedback;
        } else {
            feedback = 31;
        }
        self.chan[ci].feedback = feedback;
        self.update_synth(ci);
    }

    fn update_synth(&mut self, ci: usize) {
        if self.opl3_active != 0 {
            if (self.reg104 & self.chan[ci].four_mask) & 0x3f != 0 {
                let (chan0, chan1) = if self.chan[ci].four_mask & 0x80 == 0 {
                    (ci, ci + 1)
                } else {
                    (ci - 1, ci)
                };
                let synth = (self.chan[chan0].reg_c0 & 1) | ((self.chan[chan1].reg_c0 & 1) << 1);
                self.chan[chan0].synth_mode = match synth {
                    0 => SM3FMFM,
                    1 => SM3AMFM,
                    2 => SM3FMAM,
                    _ => SM3AMAM,
                };
            } else if (self.chan[ci].four_mask & 0x40 != 0) && (self.reg_bd & 0x20 != 0) {
                // percussion channel; leave handler as-is
            } else if self.chan[ci].reg_c0 & 1 != 0 {
                self.chan[ci].synth_mode = SM3AM;
            } else {
                self.chan[ci].synth_mode = SM3FM;
            }
            self.chan[ci].mask_left = if self.chan[ci].reg_c0 & 0x10 != 0 {
                -1
            } else {
                0
            };
            self.chan[ci].mask_right = if self.chan[ci].reg_c0 & 0x20 != 0 {
                -1
            } else {
                0
            };
        } else if (self.chan[ci].four_mask & 0x40 != 0) && (self.reg_bd & 0x20 != 0) {
            // percussion channel; leave handler as-is
        } else if self.chan[ci].reg_c0 & 1 != 0 {
            self.chan[ci].synth_mode = SM2AM;
        } else {
            self.chan[ci].synth_mode = SM2FM;
        }
    }

    fn update_synths(&mut self) {
        for i in 0..18 {
            self.update_synth(i);
        }
    }

    // ------- LFO / noise -------

    #[inline]
    fn forward_noise(&mut self) -> u32 {
        self.noise_counter = self.noise_counter.wrapping_add(self.noise_add);
        let mut count = self.noise_counter >> LFO_SH;
        self.noise_counter &= WAVE_MASK;
        while count > 0 {
            self.noise_value ^= 0x800302 & (0u32.wrapping_sub(self.noise_value & 1));
            self.noise_value >>= 1;
            count -= 1;
        }
        self.noise_value
    }

    #[inline]
    fn forward_lfo(&mut self, samples: u32) -> u32 {
        self.vibrato_sign = VIBRATO_TABLE[(self.vibrato_index >> 2) as usize] >> 7;
        self.vibrato_shift =
            (VIBRATO_TABLE[(self.vibrato_index >> 2) as usize] as u8 & 7) + self.vibrato_strength;
        self.tremolo_value =
            tables().tremolo_table[self.tremolo_index as usize] >> self.tremolo_strength;

        let todo = LFO_MAX - self.lfo_counter;
        let mut count = (todo + self.lfo_add - 1) / self.lfo_add;
        if count > samples {
            count = samples;
            self.lfo_counter += count * self.lfo_add;
        } else {
            self.lfo_counter += count * self.lfo_add;
            self.lfo_counter &= LFO_MAX - 1;
            self.vibrato_index = (self.vibrato_index + 1) & 31;
            if (self.tremolo_index as usize + 1) < TREMOLO_TABLE {
                self.tremolo_index += 1;
            } else {
                self.tremolo_index = 0;
            }
        }
        count
    }

    // ------- register writes -------

    fn write_bd(&mut self, val: u8) {
        let change = self.reg_bd ^ val;
        if change == 0 {
            return;
        }
        self.reg_bd = val;
        self.vibrato_strength = if val & 0x40 != 0 { 0x00 } else { 0x01 };
        self.tremolo_strength = if val & 0x80 != 0 { 0x00 } else { 0x02 };
        if val & 0x20 != 0 {
            if change & 0x20 != 0 {
                self.chan[6].synth_mode = if self.opl3_active != 0 {
                    SM3PERCUSSION
                } else {
                    SM2PERCUSSION
                };
            }
            if val & 0x10 != 0 {
                self.chan[6].op[0].key_on(0x2);
                self.chan[6].op[1].key_on(0x2);
            } else {
                self.chan[6].op[0].key_off(0x2);
                self.chan[6].op[1].key_off(0x2);
            }
            if val & 0x1 != 0 {
                self.chan[7].op[0].key_on(0x2);
            } else {
                self.chan[7].op[0].key_off(0x2);
            }
            if val & 0x8 != 0 {
                self.chan[7].op[1].key_on(0x2);
            } else {
                self.chan[7].op[1].key_off(0x2);
            }
            if val & 0x4 != 0 {
                self.chan[8].op[0].key_on(0x2);
            } else {
                self.chan[8].op[0].key_off(0x2);
            }
            if val & 0x2 != 0 {
                self.chan[8].op[1].key_on(0x2);
            } else {
                self.chan[8].op[1].key_off(0x2);
            }
        } else if change & 0x20 != 0 {
            self.update_synth(6);
            self.chan[6].op[0].key_off(0x2);
            self.chan[6].op[1].key_off(0x2);
            self.chan[7].op[0].key_off(0x2);
            self.chan[7].op[1].key_off(0x2);
            self.chan[8].op[0].key_off(0x2);
            self.chan[8].op[1].key_off(0x2);
        }
    }

    fn write_reg(&mut self, reg: u32, val: u8) {
        match (reg & 0xf0) >> 4 {
            0x0 | 0x1 => {
                if reg == 0x01 {
                    self.wave_form_mask = if val & 0x20 != 0 { 0x7 } else { 0x0 };
                } else if reg == 0x104 {
                    if (self.reg104 ^ val) & 0x3f == 0 {
                        return;
                    }
                    self.reg104 = 0x80 | (val & 0x3f);
                    self.update_synths();
                } else if reg == 0x105 {
                    if (self.opl3_active ^ val) & 1 == 0 {
                        return;
                    }
                    self.opl3_active = if val & 1 != 0 { 0xff } else { 0 };
                    self.update_synths();
                } else if reg == 0x08 {
                    self.reg08 = val;
                }
            }
            0x2 | 0x3 => {
                let index = (((reg >> 3) & 0x20) | (reg & 0x1f)) as usize;
                let ci = tables().op_chan[index];
                if ci >= 0 {
                    let k = tables().op_sub[index] as usize;
                    let Chip {
                        chan,
                        freq_mul,
                        attack_rates,
                        linear_rates,
                        ..
                    } = self;
                    chan[ci as usize].op[k].write20(val, freq_mul, attack_rates, linear_rates);
                }
            }
            0x4 | 0x5 => {
                let index = (((reg >> 3) & 0x20) | (reg & 0x1f)) as usize;
                let ci = tables().op_chan[index];
                if ci >= 0 {
                    let k = tables().op_sub[index] as usize;
                    self.chan[ci as usize].op[k].write40(val);
                }
            }
            0x6 | 0x7 => {
                let index = (((reg >> 3) & 0x20) | (reg & 0x1f)) as usize;
                let ci = tables().op_chan[index];
                if ci >= 0 {
                    let k = tables().op_sub[index] as usize;
                    let Chip {
                        chan,
                        attack_rates,
                        linear_rates,
                        ..
                    } = self;
                    chan[ci as usize].op[k].write60(val, attack_rates, linear_rates);
                }
            }
            0x8 | 0x9 => {
                let index = (((reg >> 3) & 0x20) | (reg & 0x1f)) as usize;
                let ci = tables().op_chan[index];
                if ci >= 0 {
                    let k = tables().op_sub[index] as usize;
                    let Chip {
                        chan, linear_rates, ..
                    } = self;
                    chan[ci as usize].op[k].write80(val, linear_rates);
                }
            }
            0xa => {
                let index = (((reg >> 4) & 0x10) | (reg & 0xf)) as usize;
                let ci = tables().chan_map[index];
                if ci >= 0 {
                    self.write_a0(ci as usize, val);
                }
            }
            0xb => {
                if reg == 0xbd {
                    self.write_bd(val);
                } else {
                    let index = (((reg >> 4) & 0x10) | (reg & 0xf)) as usize;
                    let ci = tables().chan_map[index];
                    if ci >= 0 {
                        self.write_b0(ci as usize, val);
                    }
                }
            }
            0xc => {
                let index = (((reg >> 4) & 0x10) | (reg & 0xf)) as usize;
                let ci = tables().chan_map[index];
                if ci >= 0 {
                    self.write_c0(ci as usize, val);
                }
            }
            0xd => {}
            0xe | 0xf => {
                let index = (((reg >> 3) & 0x20) | (reg & 0x1f)) as usize;
                let ci = tables().op_chan[index];
                if ci >= 0 {
                    let k = tables().op_sub[index] as usize;
                    let (wfm, o3) = (self.wave_form_mask, self.opl3_active);
                    self.chan[ci as usize].op[k].write_e0(val, wfm, o3);
                }
            }
            _ => {}
        }
    }

    // ------- block generation -------

    fn generate_percussion(&mut self, ci: usize, opl3_mode: bool, output: &mut [i32], idx: usize) {
        // Bass drum
        let fb = self.chan[ci].feedback;
        let mut mod_ =
            ((self.chan[ci].old[0].wrapping_add(self.chan[ci].old[1])) as u32 >> fb) as i32;
        self.chan[ci].old[0] = self.chan[ci].old[1];
        let s = self.op_get_sample(ci, 0, mod_);
        self.chan[ci].old[1] = s;

        if self.chan[ci].reg_c0 & 1 != 0 {
            mod_ = 0;
        } else {
            mod_ = self.chan[ci].old[0];
        }
        let mut sample = self.op_get_sample(ci, 1, mod_);

        let noise_bit = self.forward_noise() & 0x1;
        let c2 = self.op_forward_wave(ci, 2);
        let c5 = self.op_forward_wave(ci, 5);
        let phase_bit: u32 =
            if (((c2 & 0x88) ^ ((c2 << 5) & 0x80)) | ((c5 ^ (c5 << 2)) & 0x20)) != 0 {
                0x02
            } else {
                0x00
            };

        // Hi-hat
        let hh_vol = self.op_forward_volume(ci, 2);
        if !env_silent(hh_vol as i32) {
            let hh_index = (phase_bit << 8) | (0x34u32 << (phase_bit ^ (noise_bit << 1)));
            sample += self.op_get_wave(ci, 2, hh_index, hh_vol);
        }
        // Snare drum
        let sd_vol = self.op_forward_volume(ci, 3);
        if !env_silent(sd_vol as i32) {
            let sd_index = (0x100 + (c2 & 0x100)) ^ (noise_bit << 8);
            sample += self.op_get_wave(ci, 3, sd_index, sd_vol);
        }
        // Tom-tom
        sample += self.op_get_sample(ci, 4, 0);
        // Top cymbal
        let tc_vol = self.op_forward_volume(ci, 5);
        if !env_silent(tc_vol as i32) {
            let tc_index = (1 + phase_bit) << 8;
            sample += self.op_get_wave(ci, 5, tc_index, tc_vol);
        }
        sample <<= 1;
        output[idx] += sample;
        if opl3_mode {
            output[idx + 1] += sample;
        }
    }

    /// Runs one channel's synth handler over `samples`, writing into `output`
    /// starting at `out_off` (mono index). Returns the next channel index.
    fn block_template(
        &mut self,
        ci: usize,
        mode: u8,
        samples: u32,
        output: &mut [i32],
        out_off: usize,
    ) -> usize {
        // Silent-channel early-out
        match mode {
            SM2AM | SM3AM => {
                if self.op_silent(ci, 0) && self.op_silent(ci, 1) {
                    self.chan[ci].old = [0, 0];
                    return ci + 1;
                }
            }
            SM2FM | SM3FM => {
                if self.op_silent(ci, 1) {
                    self.chan[ci].old = [0, 0];
                    return ci + 1;
                }
            }
            SM3FMFM => {
                if self.op_silent(ci, 3) {
                    self.chan[ci].old = [0, 0];
                    return ci + 2;
                }
            }
            SM3AMFM => {
                if self.op_silent(ci, 0) && self.op_silent(ci, 3) {
                    self.chan[ci].old = [0, 0];
                    return ci + 2;
                }
            }
            SM3FMAM => {
                if self.op_silent(ci, 1) && self.op_silent(ci, 3) {
                    self.chan[ci].old = [0, 0];
                    return ci + 2;
                }
            }
            SM3AMAM => {
                if self.op_silent(ci, 0) && self.op_silent(ci, 2) && self.op_silent(ci, 3) {
                    self.chan[ci].old = [0, 0];
                    return ci + 2;
                }
            }
            _ => {}
        }

        self.op_prepare(ci, 0);
        self.op_prepare(ci, 1);
        if mode > SM4START {
            self.op_prepare(ci, 2);
            self.op_prepare(ci, 3);
        }
        if mode > SM6START {
            self.op_prepare(ci, 4);
            self.op_prepare(ci, 5);
        }

        for i in 0..samples as usize {
            if mode == SM2PERCUSSION {
                self.generate_percussion(ci, false, output, out_off + i);
                continue;
            } else if mode == SM3PERCUSSION {
                self.generate_percussion(ci, true, output, out_off + i * 2);
                continue;
            }

            let fb = self.chan[ci].feedback;
            let mod_ =
                ((self.chan[ci].old[0].wrapping_add(self.chan[ci].old[1])) as u32 >> fb) as i32;
            self.chan[ci].old[0] = self.chan[ci].old[1];
            let s0 = self.op_get_sample(ci, 0, mod_);
            self.chan[ci].old[1] = s0;
            let out0 = self.chan[ci].old[0];

            let sample = match mode {
                SM2AM | SM3AM => out0 + self.op_get_sample(ci, 1, 0),
                SM2FM | SM3FM => self.op_get_sample(ci, 1, out0),
                SM3FMFM => {
                    let n = self.op_get_sample(ci, 1, out0);
                    let n = self.op_get_sample(ci, 2, n);
                    self.op_get_sample(ci, 3, n)
                }
                SM3AMFM => {
                    let mut s = out0;
                    let n = self.op_get_sample(ci, 1, 0);
                    let n = self.op_get_sample(ci, 2, n);
                    s += self.op_get_sample(ci, 3, n);
                    s
                }
                SM3FMAM => {
                    let mut s = self.op_get_sample(ci, 1, out0);
                    let n = self.op_get_sample(ci, 2, 0);
                    s += self.op_get_sample(ci, 3, n);
                    s
                }
                SM3AMAM => {
                    let mut s = out0;
                    let n = self.op_get_sample(ci, 1, 0);
                    s += self.op_get_sample(ci, 2, n);
                    s += self.op_get_sample(ci, 3, 0);
                    s
                }
                _ => 0,
            };

            match mode {
                SM2AM | SM2FM => output[out_off + i] += sample,
                _ => {
                    let ml = self.chan[ci].mask_left;
                    let mr = self.chan[ci].mask_right;
                    output[out_off + i * 2] += sample & ml;
                    output[out_off + i * 2 + 1] += sample & mr;
                }
            }
        }

        match mode {
            SM2AM | SM2FM | SM3AM | SM3FM => ci + 1,
            SM3FMFM | SM3AMFM | SM3FMAM | SM3AMAM => ci + 2,
            _ => ci + 3, // percussion
        }
    }

    fn generate_block2(&mut self, output: &mut [i32]) {
        let total = output.len();
        let mut done = 0usize;
        while done < total {
            let samples = self.forward_lfo((total - done) as u32) as usize;
            for s in output[done..done + samples].iter_mut() {
                *s = 0;
            }
            let mut ci = 0usize;
            while ci < 9 {
                let mode = self.chan[ci].synth_mode;
                ci = self.block_template(ci, mode, samples as u32, output, done);
            }
            done += samples;
        }
    }

    fn setup(&mut self, rate: u32) {
        let original = OPLRATE;
        let scale = original / rate as f64;

        self.noise_add = (0.5 + scale * (1u32 << LFO_SH) as f64) as u32;
        self.noise_counter = 0;
        self.noise_value = 1;
        self.lfo_add = (0.5 + scale * (1u32 << LFO_SH) as f64) as u32;
        self.lfo_counter = 0;
        self.vibrato_index = 0;
        self.tremolo_index = 0;

        let freq_scale = (0.5 + scale * (1u32 << (WAVE_SH - 1 - 10)) as f64) as u32;
        for i in 0..16 {
            self.freq_mul[i] = freq_scale.wrapping_mul(FREQ_CREATE_TABLE[i] as u32);
        }

        for i in 0..76u8 {
            let (index, shift) = envelope_select(i);
            self.linear_rates[i as usize] = (scale
                * ((ENVELOPE_INCREASE_TABLE[index as usize] as u32)
                    << (RATE_SH + ENV_EXTRA - shift as u32 - 3)) as f64)
                as u32;
        }

        for i in 0..62u8 {
            let (index, shift) = envelope_select(i);
            let original_samples =
                ((ATTACK_SAMPLES_TABLE[index as usize] as u32) << shift) as f64 / scale;
            let original_samples = original_samples as i32;
            let mut guess_add = ((scale
                * ((ENVELOPE_INCREASE_TABLE[index as usize] as u32) << (RATE_SH - shift as u32 - 3))
                    as f64) as u32) as i32;
            let mut best_add = guess_add;
            let mut best_diff: u32 = 1 << 30;
            for _pass in 0..16 {
                let mut volume = ENV_MAX;
                let mut samples = 0i32;
                let mut count: u32 = 0;
                while volume > 0 && samples < original_samples * 2 {
                    count = count.wrapping_add(guess_add as u32);
                    let change = (count >> RATE_SH) as i32;
                    count &= RATE_MASK;
                    if change != 0 {
                        volume += (!volume * change) >> 3;
                    }
                    samples += 1;
                }
                let diff = original_samples - samples;
                let l_diff = diff.unsigned_abs();
                if l_diff < best_diff {
                    best_diff = l_diff;
                    best_add = guess_add;
                    if best_diff == 0 {
                        break;
                    }
                }
                let correct = (original_samples - diff) as f64 / original_samples as f64;
                guess_add = ((guess_add as f64 * correct) as u32) as i32;
                if diff < 0 {
                    guess_add += 1;
                }
            }
            self.attack_rates[i as usize] = best_add as u32;
        }
        for i in 62..76 {
            self.attack_rates[i] = 8 << RATE_SH;
        }

        self.chan[0].four_mask = 0x00 | (1 << 0);
        self.chan[1].four_mask = 0x80 | (1 << 0);
        self.chan[2].four_mask = 0x00 | (1 << 1);
        self.chan[3].four_mask = 0x80 | (1 << 1);
        self.chan[4].four_mask = 0x00 | (1 << 2);
        self.chan[5].four_mask = 0x80 | (1 << 2);

        self.chan[9].four_mask = 0x00 | (1 << 3);
        self.chan[10].four_mask = 0x80 | (1 << 3);
        self.chan[11].four_mask = 0x00 | (1 << 4);
        self.chan[12].four_mask = 0x80 | (1 << 4);
        self.chan[13].four_mask = 0x00 | (1 << 5);
        self.chan[14].four_mask = 0x80 | (1 << 5);

        self.chan[6].four_mask = 0x40;
        self.chan[7].four_mask = 0x40;
        self.chan[8].four_mask = 0x40;

        // Clear everything in opl3 mode
        self.write_reg(0x105, 0x1);
        for i in 0..512 {
            if i == 0x105 {
                continue;
            }
            self.write_reg(i, 0xff);
            self.write_reg(i, 0x0);
        }
        self.write_reg(0x105, 0x0);
        // Clear everything in opl2 mode
        for i in 0..255 {
            self.write_reg(i, 0xff);
            self.write_reg(i, 0x0);
        }
    }
}

// ---------------------------------------------------------------------------
// Public wrapper
// ---------------------------------------------------------------------------

#[inline]
fn clip_sample(sample: i32) -> i16 {
    if sample > 32767 {
        32767
    } else if sample < -32768 {
        -32768
    } else {
        sample as i16
    }
}

/// OPL2 chip emulator producing stereo 16-bit samples (mono duplicated).
pub struct Opl {
    chip: Chip,
    scratch: Vec<i32>,
}

impl Opl {
    /// Create a chip generating audio at `rate` Hz (e.g. 44100/49716).
    pub fn new(rate: u32) -> Opl {
        // Force table initialisation up front.
        let _ = tables();
        let mut chip = Chip::new();
        chip.setup(rate);
        Opl {
            chip,
            scratch: Vec::new(),
        }
    }

    /// Write `val` to register `addr` (0x000..=0x1FF, both register banks).
    pub fn write(&mut self, addr: u16, val: u8) {
        self.chip.write_reg(addr as u32, val);
    }

    /// Generate `out.len()` stereo samples. OPL2 output is mono and duplicated
    /// to both channels, matching `DBINTOPL2::Generate` + `update_16m_16s`.
    pub fn generate(&mut self, out: &mut [[i16; 2]]) {
        let n = out.len();
        if self.scratch.len() < n {
            self.scratch.resize(n, 0);
        }
        let buf = &mut self.scratch[..n];
        self.chip.generate_block2(buf);
        for (o, &s) in out.iter_mut().zip(buf.iter()) {
            let v = clip_sample(s);
            o[0] = v;
            o[1] = v;
        }
    }
}
