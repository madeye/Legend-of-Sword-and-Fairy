//! Headless battle visual verification: sets up a real battle and dumps the
//! composed battle scene as PPM.
//!
//! Usage: battledump <output-dir> [enemy-team]

use rustpal::game_loop::Engine;
use rustpal::surface::{SCREEN_H, SCREEN_W};

fn dump(engine: &Engine, screen: &rustpal::surface::Surface, dir: &str, name: &str) {
    let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for &px in screen.pixels.iter() {
        let c = engine.palette[px as usize];
        ppm.extend_from_slice(&c);
    }
    std::fs::write(format!("{dir}/{name}.ppm"), ppm).expect("write ppm");
    println!("dumped {name}");
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let team: u16 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut e = Engine::new(true).expect("engine");
    e.init_ui().expect("ui");
    e.globals.load_default_game().expect("default game");
    e.globals.max_party_member_index = 0;
    e.globals.party[0].player_role = 0;
    e.globals.num_battle_field = 0;

    // Pick a team: given number, or the first non-empty team.
    let team = if team != 0 {
        team
    } else {
        e.globals
            .game
            .enemy_teams
            .iter()
            .position(|t| t.enemy.iter().any(|&w| w != 0 && w != 0xFFFF))
            .unwrap_or(1) as u16
    };

    if let Ok(p) = e.get_palette(e.globals.num_palette as usize, false) {
        e.palette = p;
    }

    // Compose the battle scene exactly like PAL_StartBattle does before the
    // main loop, then dump the scene buffer.
    match e.compose_battle_scene(team, false) {
        Some(scene) => dump(&e, &scene, &dir, "battle"),
        None => eprintln!("failed to compose battle scene for team {team}"),
    }
}
