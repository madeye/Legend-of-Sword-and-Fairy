//! rustpal - a Rust reimplementation of the PAL (Legend of Sword and Fairy)
//! DOS engine.

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut ui_driver = None;
        let mut offscreen = false;
        let mut mute = false;
        for argument in std::env::args().skip(1) {
            match argument.as_str() {
                "--ui-driver" => ui_driver = Some(rustpal::ui_driver::DEFAULT_BIND.to_owned()),
                "--offscreen" => offscreen = true,
                "--mute" => mute = true,
                "--help" | "-h" => {
                    println!(
                        "Usage: rustpal [--ui-driver[=ADDR]] [--offscreen] [--mute]\n\n\
                         --ui-driver         Enable local control API at {}\n\
                         --ui-driver=ADDR    Enable it at a loopback IP and port\n\
                         --offscreen         Render without a window or audio\n\
                         --mute              Disable music and sound effects",
                        rustpal::ui_driver::DEFAULT_BIND
                    );
                    return;
                }
                _ => {
                    if let Some(bind) = argument.strip_prefix("--ui-driver=") {
                        ui_driver = Some(bind.to_owned());
                    } else {
                        eprintln!("rustpal: unknown argument {argument:?}; try --help");
                        std::process::exit(2);
                    }
                }
            }
        }
        if let Some(bind) = ui_driver {
            std::env::set_var("RUSTPAL_UI_DRIVER", bind);
        }
        if offscreen {
            std::env::set_var("RUSTPAL_OFFSCREEN", "1");
        }
        if offscreen || mute {
            std::env::set_var("RUSTPAL_DISABLE_AUDIO", "1");
        }
    }

    match rustpal::game_loop::Engine::new(false) {
        Ok(mut engine) => engine.run(),
        Err(e) => {
            eprintln!("rustpal: failed to start: {e}");
            std::process::exit(1);
        }
    }
}
