//! Record a complete headless auto-battle as raw RGBA frames, for producing
//! the README battle demo GIF.
//!
//! Usage:
//!   battlerecord list                              list non-empty enemy teams
//!   battlerecord preview <dir> <team> <field>       compose the scene, write PPM
//!   battlerecord record <dir> <team> <field> [mus]  run the battle, dump frames
//!   battlerecord play <dir> <team> <field> [mus]    run the battle in a window
//!
//! `mus` is the MUS.MKF battle-music track (default 6; 0 = silent).
//!
//! `record` writes `<dir>/frames.rgba` (concatenated 320x200 RGBA frames)
//! and `<dir>/times.txt` (tick milliseconds per frame, one per line).

use rustpal::battle::BattleResult;
use rustpal::game_loop::Engine;
use rustpal::global::{seed_random, MAX_PLAYER_MAGICS};
use rustpal::surface::{SCREEN_H, SCREEN_W};
use std::io::Write;

/// Party of three (李逍遥/赵灵儿/林月如) with full HP/MP, real default-game
/// stats otherwise.
fn setup_party(e: &mut Engine) {
    e.globals.max_party_member_index = 2;
    for i in 0..3 {
        e.globals.party[i].player_role = i as u16;
        let roles = &mut e.globals.game.player_roles;
        roles.hp[i] = roles.max_hp[i];
        roles.mp[i] = roles.max_mp[i];
    }
}

/// Multiply the enemies' health so the demo battle runs several rounds instead
/// of ending in a one-hit sweep.  Auto-battle (which casts area magic) clears
/// enemies far faster than the human-cadence pilot, so it wants a bigger boost.
fn boost_enemy_health(e: &mut Engine, team: u16, mult: u16) {
    let mut boosted = std::collections::HashSet::new();
    for j in 0..e.globals.game.enemy_teams[team as usize].enemy.len() {
        let w = e.globals.game.enemy_teams[team as usize].enemy[j];
        if w != 0 && w != 0xFFFF && boosted.insert(w) {
            let eid = e.globals.game.objects[w as usize].enemy_id() as usize;
            e.globals.game.enemies[eid].health =
                e.globals.game.enemies[eid].health.saturating_mul(mult);
        }
    }
}

fn list_teams(e: &Engine) {
    let roles = &e.globals.game.player_roles;
    for i in 0..3 {
        let magics = (0..MAX_PLAYER_MAGICS)
            .filter(|&m| roles.magic[m][i] != 0)
            .count();
        println!(
            "role {i}: lv{} hp{}/{} mp{}/{} atk{} def{} dex{} magics{}",
            roles.level[i],
            roles.hp[i],
            roles.max_hp[i],
            roles.mp[i],
            roles.max_mp[i],
            roles.attack_strength[i],
            roles.defense[i],
            roles.dexterity[i],
            magics
        );
    }
    for (idx, t) in e.globals.game.enemy_teams.iter().enumerate() {
        let mut hp = Vec::new();
        for &w in t.enemy.iter() {
            if w != 0 && w != 0xFFFF {
                let eid = e.globals.game.objects[w as usize].enemy_id() as usize;
                hp.push(e.globals.game.enemies[eid].health);
            }
        }
        if !hp.is_empty() {
            println!("team {idx}: {} enemies, hp {:?}", hp.len(), hp);
        }
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "list".into());
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let team: u16 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let field: u16 = std::env::args()
        .nth(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Battle music (MUS.MKF track). The real game sets this via script op
    // 0x0045 before each encounter; this harness bypasses scripts, so pick a
    // track here (default 6, the classic random-battle theme). 0 = silent.
    let music: u16 = std::env::args()
        .nth(5)
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);

    seed_random(20260719);
    let mut e = Engine::new(mode != "play").expect("engine");
    e.init_ui().expect("ui");
    e.globals.load_default_game().expect("default game");
    setup_party(&mut e);
    e.globals.num_battle_field = field;
    e.globals.num_battle_music = music;
    if let Ok(p) = e.get_palette(e.globals.num_palette as usize, false) {
        e.palette = p;
    }

    match mode.as_str() {
        "list" => list_teams(&e),
        "preview" => {
            let scene = e
                .compose_battle_scene(team, false)
                .expect("compose battle scene");
            let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
            for &px in scene.pixels.iter() {
                ppm.extend_from_slice(&e.palette[px as usize]);
            }
            let path = format!("{dir}/preview-t{team}-f{field}.ppm");
            std::fs::write(&path, ppm).expect("write ppm");
            println!("wrote {path}");
        }
        "play" => {
            boost_enemy_health(&mut e, team, 3);
            let result = e.start_battle(team, false);
            println!("battle result: {result:?}");
        }
        "record" => {
            std::fs::create_dir_all(&dir).expect("mkdir");
            boost_enemy_health(&mut e, team, 8);
            let frames = std::fs::File::create(format!("{dir}/frames.rgba")).expect("frames");
            let times = std::fs::File::create(format!("{dir}/times.txt")).expect("times");
            let mut frames = std::io::BufWriter::new(frames);
            let mut times = std::io::BufWriter::new(times);
            let mut count = 0u32;
            // Natural pacing: the win/level-up panels use their key-or-timeout
            // waits instead of the headless instant-confirm escape hatch.
            e.ui.auto_confirm = false;
            // Auto-battle drives the party through real turns, choosing the
            // best offensive magic when one is worth casting (and physical
            // attacks otherwise) -- so the demo exercises attack + damage
            // numbers *and* the full magic-effect animation.
            e.globals.auto_battle = true;
            e.frame_sink = Some(Box::new(move |rgba, ticks| {
                frames.write_all(rgba).expect("write frame");
                writeln!(times, "{ticks}").expect("write time");
                count += 1;
                if count.is_multiple_of(100) {
                    eprintln!("{count} frames, t={ticks}ms");
                }
            }));
            let result = e.start_battle(team, false);
            e.frame_sink = None;
            println!("battle result: {result:?}");
            assert_eq!(result, BattleResult::Won, "demo battle must be won");
        }
        other => eprintln!("unknown mode {other}"),
    }
}
