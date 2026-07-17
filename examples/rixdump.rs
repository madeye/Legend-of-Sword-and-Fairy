//! Render a RIX song from a MUS.MKF chunk to raw interleaved stereo i16 PCM on
//! stdout, for byte-exact verification against the SDLPAL C++ reference.
//!
//! Usage: `rixdump <mus.mkf> <chunk> <seconds>`
//!
//! The OPL runs at 44100 Hz and the player ticks at 70 Hz, so each tick renders
//! `44100 / 70 = 630` stereo samples; `seconds * 70` ticks are produced. This
//! matches `rixplay.cpp`'s `RIX_FillBuffer` with `iOPLSampleRate == iSampleRate`.

#[path = "../src/opl.rs"]
mod opl;
#[path = "../src/rix.rs"]
mod rix;

use rix::RixPlayer;
use std::io::Write;

const RATE: u32 = 44100;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: rixdump <mus.mkf> <chunk> <seconds>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read mkf");
    let chunk_num: usize = args[2].parse().expect("chunk number");
    let seconds: u32 = args[3].parse().expect("seconds");

    // Parse the MKF offset table (little-endian u32 offsets).
    let first = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let count = (first - 4) >> 2;
    assert!(chunk_num < count, "chunk out of range");
    let off = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
    let start = off(chunk_num);
    let end = off(chunk_num + 1);
    let chunk = &data[start..end];

    let mut player = RixPlayer::new(chunk, RATE).expect("valid RIX song");

    let spt = (RATE / 70) as usize;
    let ticks = (seconds * 70) as usize;
    let total = ticks * spt;

    let mut buf = vec![[0i16; 2]; total];
    player.render(&mut buf);

    let stdout = std::io::stdout();
    let mut w = std::io::BufWriter::new(stdout.lock());
    let mut bytes = Vec::with_capacity(total * 4);
    for s in &buf {
        bytes.extend_from_slice(&s[0].to_le_bytes());
        bytes.extend_from_slice(&s[1].to_le_bytes());
    }
    w.write_all(&bytes).expect("write pcm");
}
