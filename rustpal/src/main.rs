//! rustpal - a Rust reimplementation of the PAL (Legend of Sword and Fairy)
//! DOS engine.

fn main() {
    match rustpal::game_loop::Engine::new(false) {
        Ok(mut engine) => engine.run(),
        Err(e) => {
            eprintln!("rustpal: failed to start: {e}");
            std::process::exit(1);
        }
    }
}
