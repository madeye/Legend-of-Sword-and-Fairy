//! MKF archive access (port of the PAL_MKF* functions in SDLPAL
//! palcommon.c). An MKF file is a u32 little-endian offset table followed by
//! chunk data; the table has chunk_count + 1 entries so chunk `i` spans
//! offsets[i]..offsets[i+1].
#![allow(dead_code)]

use std::io;

use crate::yj;

/// An MKF archive loaded fully into memory.
pub struct Mkf {
    data: Vec<u8>,
    offsets: Vec<usize>,
}

fn err(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl Mkf {
    pub fn from_bytes(data: Vec<u8>) -> io::Result<Mkf> {
        if data.len() < 4 {
            return Err(err("MKF: too short".into()));
        }
        let first = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        // PAL_MKFGetChunkCount: (first - 4) >> 2 chunks; the table itself
        // has count + 1 entries.
        if first < 8 || !first.is_multiple_of(4) || first > data.len() {
            return Err(err(format!("MKF: bad offset table start {first}")));
        }
        let count = (first - 4) >> 2;
        let mut offsets = Vec::with_capacity(count + 1);
        for i in 0..=count {
            let o = i * 4;
            let v = u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
            if v > data.len() {
                return Err(err(format!("MKF: offset {v} beyond file end")));
            }
            if let Some(&prev) = offsets.last() {
                if v < prev {
                    return Err(err(format!("MKF: offsets not monotonic at chunk {i}")));
                }
            }
            offsets.push(v);
        }
        Ok(Mkf { data, offsets })
    }

    /// Number of chunks in the archive.
    pub fn chunk_count(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Size in bytes of chunk `n` (0 for out-of-range, like a missing chunk).
    pub fn chunk_size(&self, n: usize) -> usize {
        if n + 1 >= self.offsets.len() {
            0
        } else {
            self.offsets[n + 1] - self.offsets[n]
        }
    }

    /// Raw (possibly YJ_1-compressed) chunk bytes.
    pub fn chunk(&self, n: usize) -> io::Result<&[u8]> {
        if n + 1 >= self.offsets.len() {
            return Err(err(format!(
                "MKF: chunk {n} out of range (count {})",
                self.chunk_count()
            )));
        }
        Ok(&self.data[self.offsets[n]..self.offsets[n + 1]])
    }

    /// Chunk bytes, transparently decompressed if it carries a YJ_1
    /// signature (PAL_MKFDecompressChunk with the DOS decompressor).
    pub fn chunk_decompressed(&self, n: usize) -> io::Result<Vec<u8>> {
        yj::decompress(self.chunk(n)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::data::DataDir;

    pub fn data() -> DataDir {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        DataDir::new().expect("game data dir")
    }

    #[test]
    fn reads_real_archives() {
        let d = data();
        for name in [
            "abc.mkf", "ball.mkf", "data.mkf", "f.mkf", "fbp.mkf", "fire.mkf", "gop.mkf",
            "map.mkf", "mgo.mkf", "midi.mkf", "mus.mkf", "pat.mkf", "rgm.mkf", "rng.mkf",
            "sss.mkf", "voc.mkf",
        ] {
            let mkf = d.mkf(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(mkf.chunk_count() > 0, "{name}: no chunks");
            // every chunk must be addressable
            for i in 0..mkf.chunk_count() {
                mkf.chunk(i).unwrap_or_else(|e| panic!("{name}#{i}: {e}"));
            }
        }
    }

    #[test]
    fn decompresses_all_yj1_chunks() {
        let d = data();
        // These archives contain YJ_1-compressed chunks in the DOS data set
        // (sss/ball/gop/rng/voc/mus/pat are stored raw).
        for name in [
            "map.mkf", "f.mkf", "mgo.mkf", "abc.mkf", "fire.mkf", "fbp.mkf", "data.mkf",
        ] {
            let mkf = d.mkf(name).unwrap();
            let mut compressed = 0usize;
            for i in 0..mkf.chunk_count() {
                let raw = mkf.chunk(i).unwrap();
                if raw.is_empty() {
                    continue;
                }
                if crate::yj::is_yj1(raw) {
                    compressed += 1;
                    let out = mkf
                        .chunk_decompressed(i)
                        .unwrap_or_else(|e| panic!("{name}#{i}: {e}"));
                    let expect = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
                    assert_eq!(out.len(), expect, "{name}#{i}: length mismatch");
                }
            }
            assert!(compressed > 0, "{name}: expected YJ_1 chunks");
        }
    }
}
