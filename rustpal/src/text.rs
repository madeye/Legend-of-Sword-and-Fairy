//! Game text resources: WORD.DAT (word list) and M.MSG (dialogue messages).
//! Port of the DOS-branch (`!gConfig.pszMsgFile`) loading logic inside
//! `PAL_InitText`, plus `PAL_GetWord` / `PAL_GetMsg`, in SDLPAL text.c.
//!
//! All text stays in the game's native byte encoding (Big5 for this DOS data
//! set): no Unicode conversion happens here. Dialog control codes (e.g. the
//! `~NNN` delay/end marker, `-`/`'`/`@`/`"` color toggles) are left untouched
//! in the returned bytes; SDLPAL only interprets those at render time
//! (`TEXT_DisplayText`), not at retrieval time, and this port follows suit.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;

use crate::data::DataDir;

/// Bytes per WORD.DAT record. This is `gConfig.dwWordLength`, which
/// palcfg.c hardcodes to 10 for the Chinese data set (the only one this
/// port targets).
const WORD_LEN: usize = 10;

/// `MINIMAL_WORD_COUNT` from palcommon.h: `MAX_OBJECTS (600) + 13`. PAL_InitText
/// pads `nWords` up to this floor even when WORD.DAT holds fewer records, so
/// that fixed system-menu word indices past the item list stay valid.
const MINIMAL_WORD_COUNT: usize = 600 + 13;

pub struct Texts {
    /// Word `n`'s raw Big5 bytes, trailing padding/quirk-suffix already
    /// stripped (see `parse_word_record`). Always has `nWords` entries, some
    /// possibly empty.
    words: Vec<Vec<u8>>,
    /// Message `n`'s raw Big5 bytes. Always has `nMsgs` entries.
    msgs: Vec<Vec<u8>>,
}

impl Texts {
    /// Load WORD.DAT and M.MSG from the data dir.
    pub fn load(dir: &DataDir) -> io::Result<Texts> {
        let words = load_words(dir)?;
        let msgs = load_msgs(dir)?;
        Ok(Texts { words, msgs })
    }

    /// Word number `n`. Empty (matching `PAL_GetWord`'s `L""` fallback) if
    /// out of range.
    pub fn word(&self, n: usize) -> Vec<u8> {
        self.words.get(n).cloned().unwrap_or_default()
    }

    /// Message number `n`. Empty (matching `PAL_GetMsg`'s `L""` fallback) if
    /// out of range.
    pub fn msg(&self, n: usize) -> Vec<u8> {
        self.msgs.get(n).cloned().unwrap_or_default()
    }
}

fn load_words(dir: &DataDir) -> io::Result<Vec<Vec<u8>>> {
    let raw = dir.read_file("WORD.DAT")?;

    // PAL_InitText: nWords = ceil(file_size / dwWordLength), floored at
    // MINIMAL_WORD_COUNT. The word buffer is allocated as
    // dwWordLength * nWords bytes and zero-padded past file_size.
    let n_words = raw.len().div_ceil(WORD_LEN).max(MINIMAL_WORD_COUNT);

    let mut words = Vec::with_capacity(n_words);
    for i in 0..n_words {
        let base = i * WORD_LEN;
        let mut record = [0u8; WORD_LEN];
        if base < raw.len() {
            let end = (base + WORD_LEN).min(raw.len());
            record[..end - base].copy_from_slice(&raw[base..end]);
        }
        words.push(parse_word_record(record));
    }
    Ok(words)
}

/// Port of the word-splitting logic in PAL_InitText's DOS branch:
///
/// ```c
/// int pos = base + gConfig.dwWordLength - 1;
/// while (pos >= base && temp[pos] == ' ') temp[pos--] = 0;
/// ...
/// l = PAL_MultiByteToWideChar(temp + base, dwWordLength, ...);
/// if (l > 0 && lpWordBuf[i][l - 1] == '1')
///     lpWordBuf[i][l - 1] = 0;
/// ```
///
/// `PAL_MultiByteToWideChar`'s Big5 conversion loop stops at the first NUL
/// byte it scans from the start of the record, so after trailing spaces are
/// blanked out, the effective content is everything up to the first NUL.
/// Byte 0x31 ('1') can only ever be decoded as a standalone single-byte
/// character in this scheme (Big5 trail bytes are always >= 0x40), so
/// "strip a trailing decoded '1' character" is equivalent to "strip a
/// trailing raw 0x31 byte". Real WORD.DAT entries rely on this (e.g. record
/// 324 is `風雪冰天1`, trimmed to `風雪冰天`).
fn parse_word_record(mut record: [u8; WORD_LEN]) -> Vec<u8> {
    let mut end = WORD_LEN;
    while end > 0 && record[end - 1] == b' ' {
        record[end - 1] = 0;
        end -= 1;
    }
    let content_len = record[..end].iter().position(|&b| b == 0).unwrap_or(end);
    let mut content = record[..content_len].to_vec();
    if content.last() == Some(&b'1') {
        content.pop();
    }
    content
}

fn load_msgs(dir: &DataDir) -> io::Result<Vec<Vec<u8>>> {
    let mmsg = dir.read_file("M.MSG")?;
    let sss = dir.mkf("SSS.MKF")?;
    // The message offsets are in SSS.MKF #3: a table of little-endian u32
    // byte offsets into M.MSG, message n spanning offsets[n]..offsets[n+1].
    // PAL_InitText reads chunk_size/4 DWORDs and sets nMsgs = that count - 1.
    let chunk = sss.chunk_decompressed(3)?;
    let n_offsets = chunk.len() / 4;
    let offsets: Vec<u32> = (0..n_offsets)
        .map(|i| u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap()))
        .collect();
    let n_msgs = n_offsets.saturating_sub(1);

    let mut msgs = Vec::with_capacity(n_msgs);
    for i in 0..n_msgs {
        let start = (offsets[i] as usize).min(mmsg.len());
        let end = (offsets[i + 1] as usize).min(mmsg.len()).max(start);
        let slice = &mmsg[start..end];
        // PAL_MultiByteToWideChar's conversion loop also stops at the first
        // embedded NUL within the given length.
        let content_len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        msgs.push(slice[..content_len].to_vec());
    }
    Ok(msgs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> DataDir {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        DataDir::new().expect("game data dir")
    }

    #[test]
    fn word_count_is_at_least_600() {
        let t = Texts::load(&data()).expect("texts load");
        assert!(t.words.len() > 600, "got {} words", t.words.len());
        // WORD.DAT in the real DOS data set is smaller than the
        // MINIMAL_WORD_COUNT floor (600 + 13 = 613), so nWords must land
        // exactly on the floor.
        assert_eq!(t.words.len(), 613);
    }

    #[test]
    fn word_2_and_3_decode_to_expected_big5_text() {
        // Derived directly from the raw WORD.DAT bytes with a small Python
        // script; cross-checked by decoding as Big5.
        let t = Texts::load(&data()).expect("texts load");
        assert_eq!(t.word(2), b"\xb8g\xc5\xe7\xad\xc8"); // 經驗值 ("experience value")
        assert_eq!(t.word(3), b"\xaa\xac\xba\x41"); // 狀態 ("status")
    }

    #[test]
    fn word_trailing_quirk_digit_is_stripped() {
        // Record 324 is stored in WORD.DAT as "風雪冰天1" (a trailing ASCII
        // '1' byte after the trimmed content); text.c's DOS loader strips
        // that trailing '1' character. Verified against the real file with
        // a Python script (see task notes): raw record ends in 0x31 ('1'),
        // but PAL_GetWord's decoded string does not.
        let t = Texts::load(&data()).expect("texts load");
        let w = t.word(324);
        assert_eq!(w, b"\xad\xb7\xb3\xb7\xa6\x42\xa4\xd1"); // 風雪冰天
        assert!(!w.ends_with(b"1"));
    }

    #[test]
    fn word_out_of_range_and_padding_tail_are_empty() {
        let t = Texts::load(&data()).expect("texts load");
        // Indices 565..613 fall past WORD.DAT's real records (565 records)
        // but within the MINIMAL_WORD_COUNT-padded range; PAL_InitText
        // zero-fills that tail, so these must decode to empty strings.
        assert_eq!(t.word(565), Vec::<u8>::new());
        assert_eq!(t.word(600), Vec::<u8>::new());
        assert_eq!(t.word(612), Vec::<u8>::new());
        // Fully out of range (matches PAL_GetWord's "" fallback).
        assert_eq!(t.word(9999), Vec::<u8>::new());
    }

    #[test]
    fn word_bytes_decode_as_plausible_big5() {
        let t = Texts::load(&data()).expect("texts load");
        let mut checked = 0;
        for i in 0..t.words.len() {
            let w = t.word(i);
            if w.is_empty() {
                continue;
            }
            // Every non-empty word in the real data set must be valid Big5
            // (this will fail loudly if our trimming logic corrupts a
            // multi-byte sequence, e.g. by cutting off a lead byte).
            let decoded = big5_decode(&w);
            assert!(
                decoded.is_some(),
                "word {i} ({w:02x?}) is not valid Big5",
                w = w
            );
            checked += 1;
        }
        assert!(checked > 400, "expected most words to be non-empty");
    }

    #[test]
    fn msg_count_matches_sss_chunk3_offsets() {
        let t = Texts::load(&data()).expect("texts load");
        // sss.mkf chunk 3 in the real DOS data set holds 12881 u32 offsets
        // (verified directly against the file), so nMsgs = 12881 - 1.
        assert_eq!(t.msgs.len(), 12880);
    }

    #[test]
    fn msg_0_decodes_to_expected_big5_text() {
        // Derived directly from M.MSG via SSS.MKF chunk 3's offset table
        // with a Python script; decodes as Big5 to "此門已上鎖" ("this door
        // is already locked").
        let t = Texts::load(&data()).expect("texts load");
        assert_eq!(t.msg(0), b"\xa6\xb9\xaa\xf9\xa4\x77\xa4\x57\xc2\xea");
    }

    #[test]
    fn msg_1_is_a_placeholder_marker_kept_verbatim() {
        let t = Texts::load(&data()).expect("texts load");
        // Some low-numbered messages in M.MSG are just placeholder markers
        // like "?(2)"; retrieval must not interpret or strip them.
        assert_eq!(t.msg(1), b"?(2)");
        assert_eq!(t.msg(2), b"?(4)");
        assert_eq!(t.msg(3), b"?(2)");
    }

    #[test]
    fn msg_control_codes_are_kept_raw() {
        let t = Texts::load(&data()).expect("texts load");
        // The last message in the real M.MSG ends with a raw "~60" delay
        // control code; PAL_GetMsg must return it untouched (interpretation
        // happens later, in TEXT_DisplayText / PAL_DrawTextUnescape).
        let last = t.msg(t.msgs.len() - 1);
        assert!(last.ends_with(b"~60"), "{last:02x?}");
    }

    #[test]
    fn msg_out_of_range_is_empty() {
        let t = Texts::load(&data()).expect("texts load");
        assert_eq!(t.msg(t.msgs.len()), Vec::<u8>::new());
        assert_eq!(t.msg(999_999), Vec::<u8>::new());
    }

    /// Minimal Big5 validity check (lead byte 0x81-0xFE followed by a valid
    /// trail byte in 0x40-0x7E or 0xA1-0xFE; anything else must be ASCII).
    /// Used only to sanity-check that trimming didn't corrupt a multi-byte
    /// sequence; not a full Big5 implementation.
    fn big5_decode(bytes: &[u8]) -> Option<()> {
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x80 {
                i += 1;
            } else if (0x81..=0xFE).contains(&b) {
                let trail = *bytes.get(i + 1)?;
                if (0x40..=0x7E).contains(&trail) || (0xA1..=0xFE).contains(&trail) {
                    i += 2;
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        Some(())
    }
}
