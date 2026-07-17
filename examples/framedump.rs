//! Headless visual verification: renders key engine frames (trademark RNG
//! animation, splash composition, scene 1) and dumps them as PPM files.
//!
//! Usage: framedump <output-dir>

use rustpal::game_loop::Engine;
use rustpal::global::{LOAD_PLAYER_SPRITE, LOAD_SCENE};
use rustpal::surface::{SCREEN_H, SCREEN_W};

fn dump(engine: &Engine, dir: &str, name: &str) {
    let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for &px in engine.screen.pixels.iter() {
        let c = engine.palette[px as usize];
        ppm.extend_from_slice(&c);
    }
    std::fs::write(format!("{dir}/{name}.ppm"), ppm).expect("write ppm");
    println!("dumped {name}");
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut e = Engine::new(true).expect("engine");

    // Trademark: palette 3, RNG animation 6 (play at absurd speed: headless
    // delays are wall-clock; speed=100000 makes per-frame delay ~0).
    if let Ok(p) = e.get_palette(3, false) {
        e.palette = p;
    }
    e.rng_play(6, 0, -1, 100000);
    dump(&e, &dir, "trademark");

    // Splash composition: FBP 0x26/0x27 + title sprite, palette 1.
    if let Ok(p) = e.get_palette(1, false) {
        e.palette = p;
    }
    let up = e.globals.files.fbp.chunk_decompressed(0x26).unwrap();
    let down = e.globals.files.fbp.chunk_decompressed(0x27).unwrap();
    rustpal::surface::copy_rows(&up, 0, &mut e.screen, 0, 200);
    let title = e.globals.files.mgo.chunk_decompressed(0x47).unwrap();
    if let Some(f) = rustpal::surface::sprite_frame(&title, 0) {
        e.screen.blit_rle(f, 255, 10);
    }
    dump(&e, &dir, "splash_up");
    rustpal::surface::copy_rows(&down, 0, &mut e.screen, 0, 200);
    dump(&e, &dir, "splash_down");

    // Scene 1 with the party at the game start position.
    e.globals.load_default_game().unwrap();
    e.globals.max_party_member_index = 0;
    e.globals.party[0].player_role = 0;
    e.globals.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;
    e.res.load_resources(&mut e.globals).unwrap();
    if let Ok(p) = e.get_palette(0, false) {
        e.palette = p;
    }
    // A viewport known to show map content (scene 1's village).
    e.globals.viewport = (2200, 1280);
    e.globals.partyoffset = (160, 112);
    e.globals.party[0].x = 160;
    e.globals.party[0].y = 112;
    e.make_scene();
    dump(&e, &dir, "scene1");
}
