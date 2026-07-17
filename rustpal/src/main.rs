//! rustpal - a Rust reimplementation of the PAL (Legend of Sword and Fairy) DOS engine.
//!
//! Modules are being brought up one by one; `main` will become the real game
//! loop once the engine subsystems land.

mod audio;
mod battle;
mod data;
mod ending;
mod fight;
mod font;
mod game_loop;
mod global;
mod input;
mod itemmenu;
mod magicmenu;
mod map;
mod mkf;
mod opl;
mod palette;
mod play;
mod res;
mod rix;
mod rngplay;
mod scene;
mod script;
mod surface;
mod text;
mod ui;
mod uibattle;
mod uigame;
mod voc;
mod yj;

fn main() {
    match game_loop::Engine::new(false) {
        Ok(mut engine) => engine.run(),
        Err(e) => {
            eprintln!("rustpal: failed to start: {e}");
            std::process::exit(1);
        }
    }
}
