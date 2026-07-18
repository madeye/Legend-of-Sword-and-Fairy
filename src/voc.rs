//! Creative Voice File (VOC) sound effect decoding.
//!
//! Port of SDLPAL `SOUND_LoadVOCData` (reference/sdlpal/sound.c). Only
//! type-0x01 sound-data blocks holding 8-bit unsigned mono PCM are
//! supported, which covers everything the game's `voc.mkf` contains.
#![allow(dead_code)]

/// VOC file signature (0x14 bytes, including the 0x1A terminator).
const VOC_SIGNATURE: &[u8; 0x14] = b"Creative Voice File\x1A";
/// Size of the VOC file header in bytes.
const VOC_HEADER_LEN: usize = 0x1A;
/// Block type: sound data (8-bit PCM).
const BLOCK_SOUND_DATA: u8 = 0x01;

/// Decoded VOC sound: 8-bit unsigned mono PCM.
pub struct VocSound {
    pub samples: Vec<u8>,
    pub rate: u32,
}

/// Decode a VOC file (a raw chunk from voc.mkf). Returns None if invalid.
pub fn decode_voc(data: &[u8]) -> Option<VocSound> {
    if data.len() < VOC_HEADER_LEN || !data.starts_with(VOC_SIGNATURE) {
        return None;
    }
    let data_offset = u16::from_le_bytes([data[0x14], data[0x15]]) as usize;
    if data_offset >= data.len() {
        return None;
    }

    // Iterate blocks: 1-byte type, 24-bit little-endian length, payload.
    // A zero type byte is the terminator and ends the search.
    let mut pos = data_offset;
    while pos < data.len() && data[pos] != 0 {
        if pos + 4 > data.len() {
            return None;
        }
        let len =
            data[pos + 1] as usize | (data[pos + 2] as usize) << 8 | (data[pos + 3] as usize) << 16;
        if pos + 4 + len > data.len() {
            return None;
        }
        if data[pos] == BLOCK_SOUND_DATA {
            // Payload: time constant, pack byte, then PCM samples.
            // Only 8-bit unsigned PCM (pack byte 0) is supported.
            if len < 2 || data[pos + 5] != 0 {
                return None;
            }
            let time_constant = data[pos + 4] as u32;
            // Sample rate from the time constant, rounded to the next
            // 100 Hz exactly as sound.c does.
            let rate = (1_000_000 / (256 - time_constant)).div_ceil(100) * 100;
            let samples = data[pos + 6..pos + 4 + len].to_vec();
            return Some(VocSound { samples, rate });
        }
        pos += 4 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Split an MKF archive into its chunks (inline; the crate's mkf module
    /// is a stub). Format: little-endian u32 offset table,
    /// count = first_offset / 4 - 1, chunk i = bytes offsets[i]..offsets[i+1].
    fn read_mkf_chunks(path: &str) -> Vec<Vec<u8>> {
        let data = std::fs::read(path).expect("read MKF archive");
        let read_u32 =
            |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
        let count = read_u32(0) / 4 - 1;
        (0..count)
            .map(|i| data[read_u32(i)..read_u32(i + 1)].to_vec())
            .collect()
    }

    /// Build a minimal VOC file around the given block list. Each block is
    /// (type, payload); a terminator block is appended automatically.
    fn build_voc(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(VOC_SIGNATURE);
        v.extend_from_slice(&(VOC_HEADER_LEN as u16).to_le_bytes()); // data offset
        v.extend_from_slice(&0x0114u16.to_le_bytes()); // version 1.20
        v.extend_from_slice(&(!0x0114u16).wrapping_add(0x1234).to_le_bytes()); // checksum
        for (ty, payload) in blocks {
            v.push(*ty);
            let len = payload.len() as u32;
            v.extend_from_slice(&[len as u8, (len >> 8) as u8, (len >> 16) as u8]);
            v.extend_from_slice(payload);
        }
        v.push(0); // terminator
        v
    }

    /// Sound-data block payload for the given time constant / pack byte.
    fn sound_block(tc: u8, pack: u8, samples: &[u8]) -> (u8, Vec<u8>) {
        let mut p = vec![tc, pack];
        p.extend_from_slice(samples);
        (BLOCK_SOUND_DATA, p)
    }

    #[test]
    fn decodes_all_real_voc_chunks() {
        // Upper-case literal path: unlike DataDir lookups this bypasses the
        // case-insensitive matching, and the committed file is VOC.MKF
        // (breaks on case-sensitive filesystems otherwise, e.g. Linux CI).
        let chunks = read_mkf_chunks(concat!(env!("CARGO_MANIFEST_DIR"), "/pal/VOC.MKF"));
        assert!(!chunks.is_empty());

        // Empty chunks are unused MKF slots, not VOC data; the success rate
        // is measured over non-empty chunks (decode_voc is still called on
        // every chunk and must return None for empty/invalid ones).
        let mut non_empty = 0usize;
        let mut decoded = 0usize;
        for chunk in &chunks {
            let result = decode_voc(chunk);
            if chunk.is_empty() {
                assert!(result.is_none());
                continue;
            }
            non_empty += 1;
            if let Some(sound) = result {
                assert!(!sound.samples.is_empty(), "decoded samples empty");
                assert!(
                    (4000..=44100).contains(&sound.rate),
                    "rate {} out of range",
                    sound.rate
                );
                decoded += 1;
            }
        }
        assert!(non_empty > 0);
        assert!(
            decoded * 100 >= non_empty * 90,
            "decoded {decoded}/{non_empty} chunks (< 90%)"
        );
    }

    #[test]
    fn decodes_synthetic_voc() {
        // tc = 165 -> 1000000 / (256 - 165) = 10989 -> rounded up to 11000.
        let voc = build_voc(&[sound_block(165, 0, &[0, 64, 128, 192, 255])]);
        let sound = decode_voc(&voc).expect("decode synthetic VOC");
        assert_eq!(sound.rate, 11000);
        assert_eq!(sound.samples, vec![0, 64, 128, 192, 255]);
    }

    #[test]
    fn skips_non_sound_blocks() {
        // Block type 2 (sound continuation) before the sound-data block must
        // be skipped, like sound.c does.
        let voc = build_voc(&[(2, vec![1, 2, 3]), sound_block(200, 0, &[128])]);
        let sound = decode_voc(&voc).expect("decode after skipping block");
        assert_eq!(sound.samples, vec![128]);
    }

    #[test]
    fn rejects_invalid_data() {
        assert_eq!(decode_voc(&[]).map(|_| ()), None);
        assert!(decode_voc(&[0u8; 64]).is_none(), "bad signature");

        // Valid header but only a terminator block.
        assert!(decode_voc(&build_voc(&[])).is_none());

        // Non-zero pack byte (non-8-bit PCM) is not supported.
        let voc = build_voc(&[sound_block(165, 4, &[0; 16])]);
        assert!(decode_voc(&voc).is_none());

        // Truncated block payload.
        let mut voc = build_voc(&[sound_block(165, 0, &[0; 16])]);
        voc.truncate(voc.len() - 6);
        assert!(decode_voc(&voc).is_none());

        // Sound block with payload shorter than tc+pack.
        let voc = build_voc(&[(BLOCK_SOUND_DATA, vec![165])]);
        assert!(decode_voc(&voc).is_none());
    }
}
