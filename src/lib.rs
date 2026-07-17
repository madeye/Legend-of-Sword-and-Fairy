//! rustpal — a Rust reimplementation of the PAL (仙剑奇侠传 / Legend of
//! Sword and Fairy) DOS engine, ported from SDLPAL.

pub mod audio;
pub mod battle;
pub mod data;
pub mod ending;
pub mod fight;
pub mod font;
pub mod game_loop;
pub mod global;
pub mod input;
pub mod itemmenu;
pub mod magicmenu;
pub mod map;
pub mod mkf;
pub mod opl;
pub mod palette;
pub mod play;
pub mod res;
pub mod rix;
pub mod rngplay;
pub mod scene;
pub mod script;
pub mod surface;
pub mod text;
pub mod ui;
pub mod uibattle;
pub mod uigame;
pub mod voc;
#[cfg(target_arch = "wasm32")]
pub mod web;
pub mod yj;
