//! RIX (Softstar AdLib) music player. A faithful Rust port of SDLPAL's
//! `adplug/rix.cpp` (`CrixPlayer`) driving [`crate::opl::Opl`].
//!
//! [`RixPlayer::new`] takes one raw RIX song chunk (as stored, uncompressed, in
//! `MUS.MKF`) and an OPL sample rate. [`RixPlayer::render`] runs the player at
//! its fixed 70 Hz tick rate and produces interleaved stereo `[i16; 2]`
//! samples, running the OPL directly at the output rate (no resampler), exactly
//! like `rixplay.cpp`'s `RIX_FillBuffer` when `iOPLSampleRate == iSampleRate`.
#![allow(dead_code)]
// used incrementally as engine bring-up proceeds
// A deliberately literal port of the C++ CrixPlayer: nested conditionals and
// index arithmetic mirror rix.cpp so register writes stay bit-identical.
#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::collapsible_else_if,
    clippy::needless_range_loop,
    clippy::identity_op,
    clippy::implicit_saturating_sub
)]

use crate::opl::Opl;

// ---------------------------------------------------------------------------
// Static data tables (CrixPlayer::adflag / reg_data / ... in rix.cpp)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const ADFLAG: [u8; 18] = [0,0,0,1,1,1,0,0,0,1,1,1,0,0,0,1,1,1];
#[rustfmt::skip]
const REG_DATA: [u8; 18] = [0,1,2,3,4,5,8,9,10,11,12,13,16,17,18,19,20,21];
#[rustfmt::skip]
const AD_C0_OFFS: [u8; 18] = [0,1,2,0,1,2,3,4,5,3,4,5,6,7,8,6,7,8];
#[rustfmt::skip]
const MODIFY: [u8; 28] = [
    0,3,1,4,2,5,6,9,7,10,8,11,12,15,13,16,14,17,12,15,16,0,14,0,17,0,13,0,
];
#[rustfmt::skip]
const BD_REG_DATA: [u8; 124] = [
    0x00,0x00,0x00,0x00,0x00,0x00,0x10,0x08,0x04,0x02,0x01,
    0x00,0x01,0x01,0x03,0x0F,0x05,0x00,0x01,0x03,0x0F,0x00,
    0x00,0x00,0x01,0x00,0x00,0x01,0x01,0x0F,0x07,0x00,0x02,
    0x04,0x00,0x00,0x00,0x01,0x00,0x00,0x00,0x00,0x00,0x0A,
    0x04,0x00,0x08,0x0C,0x0B,0x00,0x00,0x00,0x01,0x00,0x00,
    0x00,0x00,0x0D,0x04,0x00,0x06,0x0F,0x00,0x00,0x00,0x00,
    0x01,0x00,0x00,0x0C,0x00,0x0F,0x0B,0x00,0x08,0x05,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x04,0x00,0x0F,0x0B,0x00,
    0x07,0x05,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x01,0x00,
    0x0F,0x0B,0x00,0x05,0x05,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x01,0x00,0x0F,0x0B,0x00,0x07,0x05,0x00,0x00,0x00,
    0x00,0x00,0x00,
];

/// A RIX (Softstar AdLib) song player wrapping an OPL2 chip.
pub struct RixPlayer {
    opl: Opl,

    rix_buf: Vec<u8>,
    length: u32,
    pos: u32,

    f_buffer: [u16; 300],
    a0b0_data2: [u16; 11],
    a0b0_data3: [u8; 18],
    a0b0_data4: [u8; 18],
    a0b0_data5: [u8; 96],
    addrs_head: [u8; 96],
    insbuf: [u16; 28],
    displace: [u16; 11],
    reg_bufs: [[u8; 14]; 18],
    for40reg: [u8; 18],

    i_reg: u32,
    t_reg: u32,
    mus_block: u16,
    ins_block: u16,
    rhythm: u8,
    music_on: u8,
    pause_flag: u8,
    band: u16,
    band_low: u8,
    e0_reg_flag: u16,
    bd_modify: u8,
    sustain: i32,
    play_end: i32,

    // Playback / resampling bookkeeping (mirrors rixplay.cpp's tick buffer).
    samples_per_tick: usize,
    tick_buf: Vec<[i16; 2]>,
    tick_pos: usize,
}

impl RixPlayer {
    /// Load a RIX song (a raw chunk from `mus.mkf`). Returns `None` if the data
    /// is not a valid RIX song. `opl_rate` is the audio sample rate the internal
    /// OPL chip runs at (must match the mixer output rate).
    pub fn new(song: &[u8], opl_rate: u32) -> Option<RixPlayer> {
        // Signature check (rix.cpp reads a u16 == 0x55aa at the chunk start).
        if song.len() < 14 || song[0] != 0xAA || song[1] != 0x55 {
            return None;
        }
        if opl_rate < 70 {
            return None;
        }
        let samples_per_tick = (opl_rate / 70) as usize;

        let mut p = RixPlayer {
            opl: Opl::new(opl_rate),
            rix_buf: song.to_vec(),
            length: song.len() as u32,
            pos: 0,
            f_buffer: [0; 300],
            a0b0_data2: [0; 11],
            a0b0_data3: [0; 18],
            a0b0_data4: [0; 18],
            a0b0_data5: [0; 96],
            addrs_head: [0; 96],
            insbuf: [0; 28],
            displace: [0; 11],
            reg_bufs: [[0; 14]; 18],
            for40reg: [0x7F; 18],
            i_reg: 0,
            t_reg: 0,
            mus_block: 0,
            ins_block: 0,
            rhythm: 0,
            music_on: 0,
            pause_flag: 0,
            band: 0,
            band_low: 0,
            e0_reg_flag: 0,
            bd_modify: 0,
            sustain: 0,
            play_end: 0,
            samples_per_tick,
            tick_buf: vec![[0i16; 2]; samples_per_tick],
            tick_pos: samples_per_tick, // empty; forces a tick on first render
        };

        // rewindReInit(subsong, true): the OPL was already reset by Opl::new
        // (equivalent to opl->init()), so continue with the register bootstrap.
        p.opl.write(1, 32); // go to OPL2 mode
        p.ad_initial(); // set_new_int
        p.data_initial();

        Some(p)
    }

    /// Render `out.len()` stereo samples, advancing the song (looping forever,
    /// as PAL background music does).
    pub fn render(&mut self, out: &mut [[i16; 2]]) {
        for o in out.iter_mut() {
            if self.tick_pos >= self.samples_per_tick {
                self.produce_tick();
                self.tick_pos = 0;
            }
            *o = self.tick_buf[self.tick_pos];
            self.tick_pos += 1;
        }
    }

    fn produce_tick(&mut self) {
        if !self.update() {
            // Song ended: loop. Mirrors rixplay.cpp's rewindReInit(_, false)
            // which only clears play_end/pos; rix_proc already reset the read
            // cursor to `mus_block + 1`.
            self.play_end = 0;
            self.pos = 0;
            let _ = self.update();
        }
        let mut buf = std::mem::take(&mut self.tick_buf);
        self.opl.generate(&mut buf);
        self.tick_buf = buf;
    }

    // ------- ad_bop: combined register + data write to the OPL -------

    #[inline]
    fn ad_bop(&mut self, reg: u16, value: u16) {
        self.opl.write(reg & 0xff, (value & 0xff) as u8);
    }

    #[inline]
    fn ad_08_reg(&mut self) {
        self.ad_bop(8, 0);
    }

    // ------- update / tick logic -------

    fn update(&mut self) -> bool {
        self.int_08h_entry();
        self.play_end == 0
    }

    fn int_08h_entry(&mut self) {
        let mut band_sus: u16 = 1;
        while band_sus != 0 {
            if self.sustain <= 0 {
                band_sus = self.rix_proc();
                if band_sus != 0 {
                    self.sustain += band_sus as i32;
                } else {
                    self.play_end = 1;
                    break;
                }
            } else {
                // band_sus is non-zero here, so age the sustain and stop.
                self.sustain -= 14;
                break;
            }
        }
    }

    fn rix_proc(&mut self) -> u16 {
        if self.music_on == 0 || self.pause_flag == 1 {
            return 0;
        }
        self.band = 0;
        while self.rix_buf[self.i_reg as usize] != 0x80 && self.i_reg < self.length - 1 {
            self.band_low = self.rix_buf[(self.i_reg - 1) as usize];
            let ctrl = self.rix_buf[self.i_reg as usize];
            self.i_reg += 2;
            match ctrl & 0xF0 {
                0x90 => {
                    self.rix_get_ins();
                    self.rix_90_pro((ctrl & 0x0F) as u16);
                }
                0xA0 => {
                    self.rix_a0_pro((ctrl & 0x0F) as u16, (self.band_low as u16) << 6);
                }
                0xB0 => {
                    self.rix_b0_pro((ctrl & 0x0F) as u16, self.band_low as u16);
                }
                0xC0 => {
                    self.switch_ad_bd((ctrl & 0x0F) as u16);
                    if self.band_low != 0 {
                        self.rix_c0_pro((ctrl & 0x0F) as u16, self.band_low as u16);
                    }
                }
                _ => {
                    self.band = ((ctrl as u16) << 8) + self.band_low as u16;
                }
            }
            if self.band != 0 {
                return self.band;
            }
        }
        self.music_ctrl();
        self.i_reg = self.mus_block as u32 + 1;
        self.band = 0;
        self.music_on = 1;
        0
    }

    // ------- initialisation -------

    fn ad_initial(&mut self) {
        for i in 0..25usize {
            let mut res: u32 =
                ((i as u32).wrapping_mul(24).wrapping_add(10000)).wrapping_mul(52088) / 250000;
            res = res.wrapping_mul(0x24000) / 0x1B503;
            self.f_buffer[i * 12] = ((res as u16).wrapping_add(4)) >> 3;
            for t in 1..12usize {
                res = (res as f64 * 1.06) as u32;
                self.f_buffer[i * 12 + t] = ((res as u16).wrapping_add(4)) >> 3;
            }
        }
        let mut k = 0usize;
        for i in 0..8u8 {
            for j in 0..12u8 {
                self.a0b0_data5[k] = i;
                self.addrs_head[k] = j;
                k += 1;
            }
        }
        self.ad_bd_reg();
        self.ad_08_reg();
        for i in 0..9u16 {
            self.ad_a0b0_reg(i);
        }
        self.e0_reg_flag = 0x20;
        for i in 0..18usize {
            self.ad_bop(0xE0 + REG_DATA[i] as u16, 0);
        }
        let flag = self.e0_reg_flag;
        self.ad_bop(1, flag);
    }

    fn data_initial(&mut self) {
        self.rhythm = self.rix_buf[2];
        self.mus_block = ((self.rix_buf[0x0D] as u16) << 8) + self.rix_buf[0x0C] as u16;
        self.ins_block = ((self.rix_buf[0x09] as u16) << 8) + self.rix_buf[0x08] as u16;
        self.i_reg = self.mus_block as u32 + 1;
        if self.rhythm != 0 {
            self.ad_a0b0_reg(6);
            self.ad_a0b0_reg(7);
            self.ad_a0b0_reg(8);
            self.ad_a0b0l_reg_(8, 0x18, 0);
            self.ad_a0b0l_reg_(7, 0x1F, 0);

            // Required for correct attack effect (louyihua), non-USE_RIX_EXTRA_INIT path.
            self.opl.write(0xa8, 87);
            self.opl.write(0xb8, 9);
            self.opl.write(0xa7, 3);
            self.opl.write(0xb7, 15);
        }
        self.bd_modify = 0;
        self.ad_bd_reg();
        self.band = 0;
        self.music_on = 1;
    }

    // ------- instrument / register helpers -------

    fn rix_get_ins(&mut self) {
        let baddr = self.ins_block as usize + ((self.band_low as usize) << 6);
        for i in 0..28usize {
            self.insbuf[i] = ((self.rix_buf[baddr + i * 2 + 1] as u16) << 8)
                + self.rix_buf[baddr + i * 2] as u16;
        }
    }

    fn rix_90_pro(&mut self, ctrl_l: u16) {
        let c = ctrl_l as usize;
        if self.rhythm == 0 || ctrl_l < 6 {
            let (i26, i27) = (self.insbuf[26], self.insbuf[27]);
            self.ins_to_reg(MODIFY[c * 2] as u16, 0, i26);
            self.ins_to_reg(MODIFY[c * 2 + 1] as u16, 13, i27);
        } else if ctrl_l > 6 {
            let i26 = self.insbuf[26];
            self.ins_to_reg(MODIFY[c * 2 + 6] as u16, 0, i26);
        } else {
            let (i26, i27) = (self.insbuf[26], self.insbuf[27]);
            self.ins_to_reg(12, 0, i26);
            self.ins_to_reg(15, 13, i27);
        }
    }

    fn ins_to_reg(&mut self, index: u16, ins_off: usize, value: u16) {
        let idx = index as usize;
        for i in 0..13usize {
            self.reg_bufs[idx][i] = self.insbuf[ins_off + i] as u8;
        }
        self.reg_bufs[idx][13] = (value & 3) as u8;
        self.ad_bd_reg();
        self.ad_08_reg();
        self.ad_40_reg(index);
        self.ad_c0_reg(index);
        self.ad_60_reg(index);
        self.ad_80_reg(index);
        self.ad_20_reg(index);
        self.ad_e0_reg(index);
    }

    fn ad_e0_reg(&mut self, index: u16) {
        let idx = index as usize;
        let data = if self.e0_reg_flag == 0 {
            0
        } else {
            (self.reg_bufs[idx][13] & 3) as u16
        };
        self.ad_bop(0xE0 + REG_DATA[idx] as u16, data);
    }

    fn ad_20_reg(&mut self, index: u16) {
        let idx = index as usize;
        let v = &self.reg_bufs[idx];
        let mut data: u16 = if v[9] < 1 { 0 } else { 0x80 };
        data += if v[10] < 1 { 0 } else { 0x40 };
        data += if v[5] < 1 { 0 } else { 0x20 };
        data += if v[11] < 1 { 0 } else { 0x10 };
        data += (v[1] & 0x0F) as u16;
        self.ad_bop(0x20 + REG_DATA[idx] as u16, data);
    }

    fn ad_80_reg(&mut self, index: u16) {
        let idx = index as usize;
        let v = &self.reg_bufs[idx];
        let mut data: u16 = (v[7] & 0x0F) as u16;
        let temp = v[4] as u16;
        data |= temp << 4;
        self.ad_bop(0x80 + REG_DATA[idx] as u16, data);
    }

    fn ad_60_reg(&mut self, index: u16) {
        let idx = index as usize;
        let v = &self.reg_bufs[idx];
        let mut data: u16 = (v[6] & 0x0F) as u16;
        let temp = v[3] as u16;
        data |= temp << 4;
        self.ad_bop(0x60 + REG_DATA[idx] as u16, data);
    }

    fn ad_c0_reg(&mut self, index: u16) {
        let idx = index as usize;
        if ADFLAG[idx] == 1 {
            return;
        }
        let v = &self.reg_bufs[idx];
        let mut data: u16 = v[2] as u16;
        data = data.wrapping_mul(2);
        data |= if v[12] < 1 { 1 } else { 0 };
        self.ad_bop(0xC0 + AD_C0_OFFS[idx] as u16, data);
    }

    fn ad_40_reg(&mut self, index: u16) {
        let idx = index as usize;
        let temp = self.reg_bufs[idx][0] as u16;
        let mut data: u16 = 0x3F - (0x3F & self.reg_bufs[idx][8] as u16);
        data = data.wrapping_mul(self.for40reg[idx] as u16);
        data = data.wrapping_mul(2);
        data = data.wrapping_add(0x7F);
        let res: u32 = data as u32;
        data = (res / 0xFE) as u16;
        data = data.wrapping_sub(0x3F);
        data = (0u16).wrapping_sub(data);
        data |= temp << 6;
        self.ad_bop(0x40 + REG_DATA[idx] as u16, data);
    }

    fn ad_bd_reg(&mut self) {
        let mut data: u16 = if self.rhythm < 1 { 0 } else { 0x20 };
        data |= self.bd_modify as u16;
        self.ad_bop(0xBD, data);
    }

    fn ad_a0b0_reg(&mut self, index: u16) {
        self.ad_bop(0xA0 + index, 0);
        self.ad_bop(0xB0 + index, 0);
    }

    fn ad_a0b0l_reg_(&mut self, index: u16, p2: u16, p3: u16) {
        let idx = index as usize;
        self.a0b0_data4[idx] = p3 as u8;
        self.a0b0_data3[idx] = p2 as u8;
    }

    fn ad_a0b0l_reg(&mut self, index: u16, p2: u16, p3: u16) {
        let idx = index as usize;
        let mut i: u16 = p2.wrapping_add(self.a0b0_data2[idx]);
        self.a0b0_data4[idx] = p3 as u8;
        self.a0b0_data3[idx] = p2 as u8;
        i = if (i as i16) <= 0x5F { i } else { 0x5F };
        i = if (i as i16) >= 0 { i } else { 0 };
        let ii = i as usize;
        let data = self.f_buffer[self.addrs_head[ii] as usize + (self.displace[idx] as usize) / 2];
        self.ad_bop(0xA0 + index, data);
        let data2 =
            (self.a0b0_data5[ii] as u16) * 4 + (if p3 < 1 { 0 } else { 0x20 }) + ((data >> 8) & 3);
        self.ad_bop(0xB0 + index, data2);
    }

    fn prepare_a0b0(&mut self, index: u16, v: u16) {
        let idx = index as usize;
        let mut high: i16;
        let mut low: i16;
        let mut res: u32;
        let res1: i32 = (v as i32 - 0x2000) * 0x19;
        if res1 == 0xff {
            return;
        }
        low = (res1 / 0x2000) as i16;
        if low < 0 {
            low = (0x18 - low as i32) as i16;
            high = if low < 0 { 0xFFFFu16 as i16 } else { 0 };
            res = (high as i32) as u32;
            res <<= 16;
            res = res.wrapping_add((low as i32) as u32);
            low = (((res as u16) as i16) as i32 / (0xFFE7u16 as i16) as i32) as i16;
            self.a0b0_data2[idx] = low as u16;
            low = (res as u16) as i16;
            res = ((low as i32) - 0x18) as u32;
            high = (((res as u16) as i16) as i32 % 0x19) as i16;
            low = (((res as u16) as i16) as i32 / 0x19) as i16;
            if high != 0 {
                low = 0x19;
                low = (low as i32 - high as i32) as i16;
            }
        } else {
            high = low;
            res = (high as i32) as u32;
            low = (((res as u16) as i16) as i32 / 0x19) as i16;
            self.a0b0_data2[idx] = low as u16;
            res = (high as i32) as u32;
            low = (((res as u16) as i16) as i32 % 0x19) as i16;
        }
        low = ((low as i32) * 0x18) as i16;
        self.displace[idx] = low as u16;
    }

    fn rix_a0_pro(&mut self, ctrl_l: u16, index: u16) {
        if self.rhythm == 0 || ctrl_l <= 6 {
            let v = if index > 0x3FFF { 0x3FFF } else { index };
            self.prepare_a0b0(ctrl_l, v);
            let (p2, p3) = (
                self.a0b0_data3[ctrl_l as usize] as u16,
                self.a0b0_data4[ctrl_l as usize] as u16,
            );
            self.ad_a0b0l_reg(ctrl_l, p2, p3);
        }
    }

    fn rix_b0_pro(&mut self, ctrl_l: u16, index: u16) {
        let temp: usize = if self.rhythm == 0 || ctrl_l < 6 {
            MODIFY[(ctrl_l * 2 + 1) as usize] as usize
        } else {
            let t = if ctrl_l > 6 {
                ctrl_l * 2
            } else {
                ctrl_l * 2 + 1
            };
            MODIFY[(t + 6) as usize] as usize
        };
        self.for40reg[temp] = if index > 0x7F { 0x7F } else { index as u8 };
        self.ad_40_reg(temp as u16);
    }

    fn rix_c0_pro(&mut self, ctrl_l: u16, index: u16) {
        let i = if index >= 12 { index - 12 } else { 0 };
        if ctrl_l < 6 || self.rhythm == 0 {
            self.ad_a0b0l_reg(ctrl_l, i, 1);
        } else {
            if ctrl_l != 6 {
                if ctrl_l == 8 {
                    self.ad_a0b0l_reg(ctrl_l, i, 0);
                    self.ad_a0b0l_reg(7, i + 7, 0);
                }
            } else {
                self.ad_a0b0l_reg(ctrl_l, i, 0);
            }
            self.bd_modify |= BD_REG_DATA[ctrl_l as usize];
            self.ad_bd_reg();
        }
    }

    fn switch_ad_bd(&mut self, index: u16) {
        if self.rhythm == 0 || index < 6 {
            let p2 = self.a0b0_data3[index as usize] as u16;
            self.ad_a0b0l_reg(index, p2, 0);
        } else {
            self.bd_modify &= !BD_REG_DATA[index as usize];
            self.ad_bd_reg();
        }
    }

    fn music_ctrl(&mut self) {
        for i in 0..11u16 {
            self.switch_ad_bd(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDir;

    fn data() -> DataDir {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        DataDir::new().expect("game data dir")
    }

    #[test]
    fn loads_and_renders_real_songs() {
        let d = data();
        let mkf = d.mkf("mus.mkf").unwrap();
        let rate = 44100u32;
        // A handful of known non-empty songs.
        let mut tested = 0;
        for song in 1..=6usize {
            let chunk = mkf.chunk(song).unwrap();
            let mut player = RixPlayer::new(chunk, rate)
                .unwrap_or_else(|| panic!("song {song}: not a valid RIX chunk"));

            // Render 1 second of audio.
            let mut out = vec![[0i16; 2]; rate as usize];
            player.render(&mut out);

            // Left/right must be identical (mono duplicated to stereo).
            assert!(out.iter().all(|s| s[0] == s[1]), "song {song}: not mono");

            // Output must be non-constant (the song actually plays something).
            let first = out[0][0];
            assert!(
                out.iter().any(|s| s[0] != first),
                "song {song}: constant output"
            );
            let nonzero = out.iter().filter(|s| s[0] != 0).count();
            assert!(nonzero > 0, "song {song}: silent output");
            tested += 1;
        }
        assert_eq!(tested, 6);
    }

    #[test]
    fn rejects_non_rix_data() {
        assert!(RixPlayer::new(&[0u8; 32], 44100).is_none());
        assert!(RixPlayer::new(&[0xAA, 0x55], 44100).is_none()); // too short
    }
}
