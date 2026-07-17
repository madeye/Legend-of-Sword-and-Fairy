//! Locates and reads the original PAL DOS game data files (the `pal/` dir).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::mkf::Mkf;

pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Find the game data directory. Searches, in order:
    /// `PAL_DATA_DIR` env var, `<cwd>/pal`, `<cwd>`, `<exe>/pal`, `<exe>/../pal`.
    pub fn new() -> io::Result<DataDir> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var("PAL_DATA_DIR") {
            candidates.push(PathBuf::from(p));
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("pal"));
            candidates.push(cwd.join("../pal"));
            candidates.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("pal"));
                candidates.push(dir.join("../pal"));
            }
        }
        for c in candidates {
            if has_data_files(&c) {
                return Ok(DataDir { root: c });
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "could not locate PAL game data (pal/ directory); set PAL_DATA_DIR",
        ))
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Read an entire data file. File name matching is case-insensitive.
    pub fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        let direct = self.root.join(name);
        if direct.is_file() {
            return fs::read(direct);
        }
        let lower = name.to_lowercase();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().to_lowercase() == lower {
                return fs::read(entry.path());
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("data file not found: {}", name),
        ))
    }

    /// Open an MKF archive from the data dir.
    pub fn mkf(&self, name: &str) -> io::Result<Mkf> {
        Mkf::from_bytes(self.read_file(name)?)
    }
}

fn has_data_files(dir: &PathBuf) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut has_gop = false;
    let mut has_fbp = false;
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if n == "gop.mkf" {
            has_gop = true;
        }
        if n == "fbp.mkf" {
            has_fbp = true;
        }
    }
    has_gop && has_fbp
}
