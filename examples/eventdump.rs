//! Print event-object state from an autoplay checkpoint.

use rustpal::game_loop::Engine;

fn main() {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
    let path = std::env::args().nth(1).expect("checkpoint path");
    let bytes = std::fs::read(path).expect("read checkpoint");
    let mut engine = Engine::new(true).expect("headless engine");
    engine
        .globals
        .load_game_from_bytes(&bytes)
        .expect("load checkpoint");
    println!(
        "current scene={} viewport={:?}",
        engine.globals.num_scene, engine.globals.viewport
    );
    let scene = engine.globals.game.scenes[engine.globals.num_scene as usize - 1];
    println!(
        "scene map={} enter={} teleport={} end_event={}",
        scene.map_num, scene.script_on_enter, scene.script_on_teleport, scene.event_object_index
    );
    for entry in engine
        .globals
        .inventory
        .iter()
        .filter(|entry| entry.item != 0 && entry.amount != 0)
    {
        let object = engine.globals.game.objects[entry.item as usize];
        println!(
            "item={} amount={} use={} equip={} throw={} flags={:04x}",
            entry.item,
            entry.amount,
            object.item_script_on_use(),
            object.item_script_on_equip(),
            object.item_script_on_throw(),
            object.item_flags()
        );
    }
    for (slot, equipment) in engine
        .globals
        .game
        .player_roles
        .equipment
        .iter()
        .enumerate()
    {
        for (role, item) in equipment.iter().copied().enumerate() {
            if item != 0 {
                println!("equipment role={role} slot={slot} item={item}");
            }
        }
    }
    for scene in 1..engine.globals.game.scenes.len() {
        let scene_entry = engine.globals.game.scenes[scene - 1];
        println!(
            "scene_header={scene} map={} enter={} teleport={} end_event={}",
            scene_entry.map_num,
            scene_entry.script_on_enter,
            scene_entry.script_on_teleport,
            scene_entry.event_object_index
        );
        let first = engine.globals.game.scenes[scene - 1].event_object_index as usize;
        let last = engine.globals.game.scenes[scene].event_object_index as usize;
        for index in first..last {
            let event = engine.globals.game.event_objects[index];
            if event.trigger_script == 0 && event.auto_script == 0 {
                continue;
            }
            println!(
                "scene={scene} event={} pos=({}, {}) state={} mode={} trigger={} auto={} vanish={}",
                index + 1,
                event.x,
                event.y,
                event.state,
                event.trigger_mode,
                event.trigger_script,
                event.auto_script,
                event.vanish_time
            );
        }
    }
}
