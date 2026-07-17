//! rustpal - a Rust reimplementation of the PAL (Legend of Sword and Fairy) DOS engine.
//!
//! Modules are being brought up one by one; `main` will become the real game
//! loop once the engine subsystems land.

mod audio;
mod data;
mod font;
mod input;
mod mkf;
mod opl;
mod palette;
mod rix;
mod surface;
mod text;
mod voc;
mod yj;

fn main() {
    println!("rustpal: engine bring-up in progress");
}
