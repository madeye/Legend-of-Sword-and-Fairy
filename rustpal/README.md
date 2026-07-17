# rustpal

A Rust reimplementation of the PAL (仙剑奇侠传 / Legend of Sword and Fairy,
DOS version) game engine, ported from [SDLPAL](https://github.com/sdlpal/sdlpal).

## Running

The engine needs the original DOS game data. It looks for it in
`PAL_DATA_DIR`, `./pal`, `../pal`, or next to the executable — this
repository ships the data in `../pal`, so from this directory:

```shell
cargo run --release
```

Controls: arrow keys to walk, Space/Enter to search/confirm, Esc for the
menu (the full DOS key map from SDLPAL applies: R/A/D/E/W/Q/F/S in battle).

## Architecture

- `game_loop.rs` — the `Engine` that owns all game state (the Rust
  counterpart of SDLPAL's globals), the winit/pixels video shell, frame
  timing, palette effects and screen transitions.
- Subsystem modules extend `Engine` with `impl` blocks:
  `scene` (map + sprite rendering, movement, collision), `script` (the
  full opcode interpreter), `play` (per-frame update), `ui`/`uigame`/
  `itemmenu`/`magicmenu` (dialogs and menus), `battle`/`fight`/`uibattle`
  (turn-based classic combat), `ending`, `rngplay` (cutscene videos).
- Data layer: `mkf` (archives), `yj` (YJ_1/YJ_2 decompression), `map`,
  `global` (game data + DOS save format), `res`, `text`/`font`
  (Big5 text with the original WOR16 fonts), `palette`, `surface`.
- Audio: `opl` (DOSBox DBOPL emulator port), `rix` (RIX/AdLib music
  player), `voc` (sound effects), `audio` (cpal mixer).

## Fidelity

Ported from the SDLPAL C source, DOS paths (`PAL_CLASSIC`), and validated
against the original game data: YJ_1 decompression and the OPL/RIX music
renderer are verified byte-identical to the C implementations across the
full data set; rendering is verified against known-good frames
(`examples/framedump.rs`, `examples/newgame.rs`, `examples/uidump.rs`).

## Tests

```shell
cargo test          # unit + integration tests (require ../pal data)
```
