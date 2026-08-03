//! Print raw script entries and referenced dialog bytes for route debugging.

use rustpal::game_loop::Engine;

fn main() {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
    let engine = Engine::new(true).expect("headless engine");
    let start = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .expect("start script index");
    let count = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    for index in start..start.saturating_add(count) {
        let Some(entry) = engine.globals.game.script_entries.get(index) else {
            break;
        };
        print!(
            "{index:05} op={:04X} args={:04X},{:04X},{:04X}",
            entry.operation, entry.operand[0], entry.operand[1], entry.operand[2]
        );
        if entry.operation == 0xFFFF {
            print!(" msg=");
            for byte in engine.texts.msg(entry.operand[0] as usize) {
                print!("{byte:02x}");
            }
        }
        println!();
    }
}
