use rustpal::battle::BattleResult;
use rustpal::game_loop::Engine;
use rustpal::global::MAX_PLAYER_ROLES;

fn arm_party(engine: &mut Engine) {
    let roles = &mut engine.globals.game.player_roles;
    for role in 0..MAX_PLAYER_ROLES {
        roles.max_hp[role] = roles.max_hp[role].max(30_000);
        roles.hp[role] = roles.max_hp[role];
        roles.max_mp[role] = roles.max_mp[role].max(10_000);
        roles.mp[role] = roles.max_mp[role];
        roles.attack_strength[role] = roles.attack_strength[role].max(5_000);
        roles.magic_strength[role] = roles.magic_strength[role].max(5_000);
        roles.defense[role] = roles.defense[role].max(5_000);
        roles.dexterity[role] = roles.dexterity[role].max(500);
        roles.poison_resistance[role] = 100;
    }
}

fn main() {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
    let team = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(40);
    let mut engine = Engine::new(true).expect("headless engine");
    engine.init_ui().expect("initialize UI");
    engine.globals.load_default_game().expect("load new game");
    arm_party(&mut engine);
    engine.globals.auto_battle = true;
    engine.battle_instant = true;
    let result = engine.start_battle(team, false);
    println!("team={team} result={result:?}");
    assert_eq!(result, BattleResult::Won);
}
