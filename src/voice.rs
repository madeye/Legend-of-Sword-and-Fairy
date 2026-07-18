//! Dialog voice-over banks (an addition over the original DOS game).
//!
//! `pal/voice/NNN.vbk` (NNN = zero-padded scene number) holds all TTS lines
//! for one scene, generated offline by tools/voice/gen_voices.py:
//!
//! ```text
//! "PVB1" | u32 count | count * { u32 msg_id, u32 start_sample, u32 n_samples }
//!        | one Ogg Vorbis stream (mono) with every clip concatenated
//! ```
//!
//! A single Vorbis stream per scene avoids per-clip codebook headers; the
//! index addresses clips as sample ranges into the decoded stream. Banks are
//! loaded lazily on scene switch (`VoiceState::ensure_scene`) and evicted
//! LRU. Everything degrades silently: missing banks, unknown message ids, or
//! decode errors just mean no voice.

use std::collections::HashMap;
use std::io::Cursor;

use crate::data::DataDir;

/// One scene's decoded voice clips.
pub struct VoiceBank {
    /// msg id -> (start, len) sample range into `pcm`.
    index: HashMap<u32, (u32, u32)>,
    /// The whole bank's mono PCM, decoded at load time.
    pcm: Vec<i16>,
    /// Source sample rate of `pcm`.
    rate: u32,
}

impl VoiceBank {
    pub fn parse(bytes: &[u8]) -> Option<VoiceBank> {
        let count = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        if bytes.get(..4)? != b"PVB1" || count > 0x10000 {
            return None;
        }
        let mut index = HashMap::with_capacity(count);
        let mut pos = 8;
        for _ in 0..count {
            let e = bytes.get(pos..pos + 12)?;
            let msg = u32::from_le_bytes(e[0..4].try_into().ok()?);
            let start = u32::from_le_bytes(e[4..8].try_into().ok()?);
            let n = u32::from_le_bytes(e[8..12].try_into().ok()?);
            index.insert(msg, (start, n));
            pos += 12;
        }
        let (pcm, rate) = decode_ogg(&bytes[pos..])?;
        Some(VoiceBank { index, pcm, rate })
    }

    /// The clip for a message, as (mono samples, sample rate).
    pub fn get(&self, msg_id: u32) -> Option<(&[i16], u32)> {
        let &(start, n) = self.index.get(&msg_id)?;
        let clip = self
            .pcm
            .get(start as usize..(start as usize).checked_add(n as usize)?)?;
        Some((clip, self.rate))
    }
}

/// Decode an Ogg Vorbis stream to mono i16.
fn decode_ogg(data: &[u8]) -> Option<(Vec<i16>, u32)> {
    let mut rdr = lewton::inside_ogg::OggStreamReader::new(Cursor::new(data)).ok()?;
    let rate = rdr.ident_hdr.audio_sample_rate;
    let channels = rdr.ident_hdr.audio_channels.max(1) as usize;
    let mut pcm = Vec::new();
    while let Ok(Some(packet)) = rdr.read_dec_packet_itl() {
        if channels == 1 {
            pcm.extend_from_slice(&packet);
        } else {
            pcm.extend(packet.iter().step_by(channels));
        }
    }
    Some((pcm, rate))
}

/// Lazily loaded per-scene banks (small LRU: the current scene plus the
/// previous one, so quick back-and-forth transitions don't re-decode).
#[derive(Default)]
pub struct VoiceState {
    /// Most recently used first.
    banks: Vec<(u16, VoiceBank)>,
    /// Scenes whose bank is known to be absent (don't retry every frame).
    missing: Vec<u16>,
}

const BANK_LRU: usize = 2;

impl VoiceState {
    pub fn new() -> VoiceState {
        VoiceState::default()
    }

    /// Make sure the bank for `scene` is resident (no-op if cached or known
    /// missing).
    pub fn ensure_scene(&mut self, scene: u16, dir: &DataDir) {
        if let Some(i) = self.banks.iter().position(|(s, _)| *s == scene) {
            let b = self.banks.remove(i);
            self.banks.insert(0, b);
            return;
        }
        if self.missing.contains(&scene) {
            return;
        }
        let bank = dir
            .read_file_lazy(&format!("voice/{scene:03}.vbk"))
            .ok()
            .and_then(|bytes| VoiceBank::parse(&bytes));
        match bank {
            Some(b) => {
                self.banks.insert(0, (scene, b));
                self.banks.truncate(BANK_LRU);
            }
            None => self.missing.push(scene),
        }
    }

    /// The clip for a message in `scene`, if its bank is resident.
    pub fn get(&self, scene: u16, msg_id: u32) -> Option<(&[i16], u32)> {
        self.banks
            .iter()
            .find(|(s, _)| *s == scene)
            .and_then(|(_, b)| b.get(msg_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bank(entries: &[(u32, u32, u32)], ogg: &[u8]) -> Vec<u8> {
        let mut v = b"PVB1".to_vec();
        v.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for &(msg, start, n) in entries {
            v.extend_from_slice(&msg.to_le_bytes());
            v.extend_from_slice(&start.to_le_bytes());
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(ogg);
        v
    }

    #[test]
    fn rejects_garbage() {
        assert!(VoiceBank::parse(b"").is_none());
        assert!(VoiceBank::parse(b"XXXX\0\0\0\0").is_none());
        // Valid header but truncated index.
        assert!(VoiceBank::parse(&fake_bank(&[], b"")[..6]).is_none());
        // Valid index but garbage ogg payload.
        assert!(VoiceBank::parse(&fake_bank(&[(1, 0, 10)], b"not an ogg")).is_none());
    }

    #[test]
    fn real_bank_roundtrip() {
        // Generated bank (present only after running tools/voice).
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/pal/voice/001.vbk");
        let Ok(bytes) = std::fs::read(dir) else {
            return;
        };
        let bank = VoiceBank::parse(&bytes).expect("bank parses");
        let (clip, rate) = bank.get(587).expect("msg 587 voiced in scene 1");
        assert_eq!(rate, 16000);
        assert!(!clip.is_empty());
        assert!(bank.get(0xFFFF_FFFF).is_none());
    }
}
