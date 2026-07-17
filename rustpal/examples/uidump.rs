//! Headless UI visual verification: renders the opening menu, an in-game
//! dialog with real text, and the player status screen; dumps PPM frames.
//!
//! Usage: uidump <output-dir>

use rustpal::game_loop::Engine;
use rustpal::surface::{SCREEN_H, SCREEN_W};
use rustpal::ui::MenuItem;

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
    e.init_ui().expect("ui sprites");

    // Opening menu: background + the two menu labels.
    e.draw_opening_menu_background();
    if let Ok(p) = e.get_palette(0, false) {
        e.palette = p;
    }
    let items = [
        MenuItem {
            value: 0,
            num_word: 7,
            enabled: true,
            pos: (125, 95),
        },
        MenuItem {
            value: 1,
            num_word: 8,
            enabled: true,
            pos: (125, 112),
        },
    ];
    // read_menu with auto_confirm draws once and returns the default.
    let _ = e.read_menu(&items, 0x4F, None);
    dump(&e, &dir, "opening_menu");

    // In-game scene + dialog with a real message.
    e.globals.current_save_slot = 0;
    e.globals.in_main_game = true;
    e.globals.reload_in_next_tick(0);
    let _ = e.res.load_resources(&mut e.globals);
    e.update_equipments();
    e.input.clear_key_state();
    e.start_frame();
    if let Ok(p) = e.get_palette(e.globals.num_palette as usize, e.globals.night_palette) {
        e.palette = p;
    }
    e.make_scene();
    e.start_dialog(2, 0x4F, 0, false); // lower position
    let msg = e.texts.msg(0);
    e.show_dialog_text(&msg);
    dump(&e, &dir, "dialog");
    e.end_dialog();

    // Status screen for Li Xiaoyao.
    e.player_status();
    dump(&e, &dir, "status");
}
