//! Locates and reads the original PAL DOS game data files (the `pal/` dir).
//! On the web the "directory" is the `PAL_FILES` map (file name ->
//! Uint8Array) that web/worker.js installs on the worker global scope before
//! starting the engine.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::mkf::Mkf;

pub struct DataDir {
    root: PathBuf,
}

#[cfg(target_arch = "wasm32")]
impl DataDir {
    pub fn new() -> io::Result<DataDir> {
        // Verify the file map is present so a bad setup fails at boot.
        files_map()?;
        Ok(DataDir {
            root: PathBuf::from("pal"),
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Read an entire data file. File name matching is case-insensitive
    /// (the map keys are the upper-case DOS file names).
    pub fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        let files = files_map()?;
        let v = js_sys::Reflect::get(&files, &name.to_uppercase().into())
            .map_err(|_| io::Error::other("PAL_FILES lookup failed"))?;
        if v.is_undefined() || v.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("data file not found: {}", name),
            ));
        }
        Ok(js_sys::Uint8Array::new(&v).to_vec())
    }

    /// Open an MKF archive from the data dir.
    pub fn mkf(&self, name: &str) -> io::Result<Mkf> {
        Mkf::from_bytes(self.read_file(name)?)
    }
}

#[cfg(target_arch = "wasm32")]
fn files_map() -> io::Result<wasm_bindgen::JsValue> {
    let v = js_sys::Reflect::get(&js_sys::global(), &"PAL_FILES".into())
        .map_err(|_| io::Error::other("no global scope"))?;
    if v.is_undefined() || v.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "PAL_FILES not set on the worker global scope",
        ));
    }
    Ok(v)
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
