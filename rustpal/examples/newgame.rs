//! Headless new-game integration test: boots the engine state exactly like
//! PAL_GameMain (opening menu -> slot 0), loads resources, then runs a few
//! frames of the real game loop (enter-scene script included) and dumps the
//! resulting scene as PPM.
//!
//! Usage: newgame <output-dir> [frames]

use rustpal::game_loop::Engine;
use rustpal::surface::{SCREEN_H, SCREEN_W};

fn dump(engine: &Engine, dir: &str, name: &str) {
    let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for &px in engine.screen.pixels.iter() {
        let c = engine.palette[px as usize];
        ppm.extend_from_slice(&c);
    }
    std::fs::write(format!("{dir}/{name}.ppm"), ppm).expect("write ppm");
    println!(
        "dumped {name} (scene {}, viewport {:?}, party at {},{})",
        engine.globals.num_scene,
        engine.globals.viewport,
        engine.globals.party[0].x,
        engine.globals.party[0].y
    );
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let frames: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let mut e = Engine::new(true).expect("engine");

    // PAL_GameMain: slot 0 = new game.
    e.globals.current_save_slot = 0;
    e.globals.in_main_game = true;
    e.globals.reload_in_next_tick(0);

    for frame in 0..frames {
        match e.res.load_resources(&mut e.globals) {
            Ok(flags) => {
                if flags.global_data {
                    e.update_equipments();
                }
            }
            Err(err) => {
                eprintln!("load_resources failed: {err}");
                return;
            }
        }
        e.input.clear_key_state();
        e.start_frame();
        println!(
            "frame {frame}: scene={} viewport={:?} party=({},{}) entering={}",
            e.globals.num_scene,
            e.globals.viewport,
            e.globals.party[0].x,
            e.globals.party[0].y,
            e.globals.entering_scene
        );
    }

    // Use the real palette and render the final state.
    if let Ok(p) = e.get_palette(e.globals.num_palette as usize, e.globals.night_palette) {
        e.palette = p;
    }
    e.make_scene();
    dump(&e, &dir, "newgame");
}
