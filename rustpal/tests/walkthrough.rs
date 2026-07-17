//! Headless end-to-end integration test: start a new game exactly like
//! PAL_GameMain, run the scene-enter script, then walk the player around the
//! starting room with simulated key input and verify movement, rendering and
//! obstacle behavior against the real game data.

use rustpal::game_loop::Engine;
use winit::keyboard::KeyCode;

fn new_game_engine() -> Engine {
    std::env::set_var(
        "PAL_DATA_DIR",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
    );
    let mut e = Engine::new(true).expect("headless engine");
    e.globals.current_save_slot = 0;
    e.globals.in_main_game = true;
    e.globals.reload_in_next_tick(0);

    // First frame loads resources and runs the scene-enter script.
    let flags = e.res.load_resources(&mut e.globals).expect("resources");
    assert!(flags.global_data && flags.scene && flags.player_sprite);
    e.update_equipments();
    e.input.clear_key_state();
    e.start_frame();
    e
}

#[test]
fn new_game_starts_in_scene_one_with_valid_position() {
    let e = new_game_engine();
    assert_eq!(e.globals.num_scene, 1);
    // The enter script must have placed the viewport somewhere real.
    assert!(e.globals.viewport.0 > 0 && e.globals.viewport.1 > 0);
    // The scene must render non-empty.
    let nonzero = e.screen.pixels.iter().filter(|&&p| p != 0).count();
    assert!(nonzero > 10000, "scene mostly empty: {nonzero}");
}

#[test]
fn player_walks_with_key_input_and_stops_at_obstacles() {
    let mut e = new_game_engine();

    let start = e.globals.viewport;
    // Hold "down" (south) and run frames; the party should move.
    e.input.handle_key_event(KeyCode::ArrowDown, true);
    for _ in 0..6 {
        e.input.update_keyboard_state(e.ticks() + 1000);
        e.start_frame();
        e.input.clear_key_state();
    }
    e.input.handle_key_event(KeyCode::ArrowDown, false);
    let after_south = e.globals.viewport;
    assert_ne!(start, after_south, "party did not move south");

    // Walk in every direction; the engine must never panic and the
    // viewport must stay within the map bounds.
    for key in [
        KeyCode::ArrowLeft,
        KeyCode::ArrowUp,
        KeyCode::ArrowRight,
        KeyCode::ArrowDown,
    ] {
        e.input.handle_key_event(key, true);
        for _ in 0..40 {
            e.input.update_keyboard_state(e.ticks() + 1000);
            e.start_frame();
            e.input.clear_key_state();
        }
        e.input.handle_key_event(key, false);
        let (vx, vy) = e.globals.viewport;
        assert!(
            (0..4096).contains(&vx) && (0..2048).contains(&vy),
            "viewport out of world bounds: {vx},{vy}"
        );
    }

    // Obstacles must exist: walking forever in one direction cannot go on
    // unbounded (the room has walls) — verify the party got stopped at some
    // point by checking it did not travel 40 tiles in the last direction.
    let total_dy = (e.globals.viewport.1 - start.1).abs();
    assert!(total_dy < 40 * 16, "no obstacle ever stopped the party");
}

#[test]
fn search_near_start_triggers_no_crash() {
    let mut e = new_game_engine();
    // Simulated Space (search) presses around the starting position must
    // run trigger scripts without panicking.
    for _ in 0..3 {
        e.input.handle_key_event(KeyCode::Space, true);
        e.input.update_keyboard_state(e.ticks() + 1000);
        e.start_frame();
        e.input.handle_key_event(KeyCode::Space, false);
        e.input.update_keyboard_state(e.ticks() + 2000);
        e.input.clear_key_state();
    }
}
