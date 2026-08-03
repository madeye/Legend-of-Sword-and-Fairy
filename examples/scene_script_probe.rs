//! Run one scene-entry script from an autoplay checkpoint for hang diagnosis.

use rustpal::game_loop::Engine;
use rustpal::global::{LOAD_PLAYER_SPRITE, LOAD_SCENE};

fn main() {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
    std::env::set_var("RUSTPAL_HEADLESS_TIME_SCALE", "10000");
    let path = std::env::args().nth(1).expect("checkpoint path");
    let scene = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<u16>().ok())
        .expect("scene number");
    let script = std::env::args()
        .nth(3)
        .and_then(|value| value.parse::<u16>().ok())
        .expect("script number");

    let mut engine = Engine::new(true).expect("headless engine");
    engine.init_ui().expect("initialize UI");
    engine.globals.in_main_game = true;
    engine
        .globals
        .load_game_from_bytes(&std::fs::read(path).expect("read checkpoint"))
        .expect("load checkpoint");
    engine.globals.num_scene = scene;
    engine.globals.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;
    engine
        .res
        .load_resources(&mut engine.globals)
        .expect("load scene resources");
    engine.globals.auto_battle = true;
    engine.battle_instant = true;

    eprintln!("running scene={scene} script={script}");
    let next = engine.run_trigger_script(script, 0);
    println!(
        "completed next={next} scene={} frame={} quit={}",
        engine.globals.num_scene, engine.globals.frame_num, engine.quit_requested
    );
}
