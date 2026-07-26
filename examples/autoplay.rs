//! Auto-play the game and record it to a video.
//!
//! Boots the engine exactly like `main()` does (trademark reel, splash screen,
//! opening menu, then `PAL_GameMain`) with two things attached:
//!
//!   * a **synthetic keyboard** (`Engine::autopilot`) that presses confirm and
//!     the arrow keys at a human cadence.  It goes through the same
//!     `InputState` a real keyboard does, so every dialog, menu, script and
//!     battle runs the normal way — nothing is faked or fast-pathed.
//!   * an **offline audio mixer** plus the per-present `frame_sink`, so the
//!     recording carries the real OPL music and sound effects, sample-aligned
//!     with the captured frames.
//!
//! Recording happens in two phases, because the engine clock is wall-clock
//! (`Engine::ticks` is `Instant::elapsed`) and the neural upscaler costs tens
//! of milliseconds per frame.  Upscaling *while* the game runs would push the
//! game behind real time while the music — rendered against the same clock —
//! kept its pace, so the picture would drift into slow motion against the
//! sound.  Instead:
//!
//!   * **record** plays the game in real time and dumps cheap 320x200 frames,
//!     their engine ticks, and the PCM audio;
//!   * **encode** comes back afterwards with no deadline, runs every distinct
//!     frame through the upscaler, sample-and-holds to a constant frame rate,
//!     and muxes 1280x800 H.264 with the audio.
//!
//! Usage:
//!   autoplay record <dir> [seconds]   record only
//!   autoplay encode <dir> [fps]       encode a previous recording
//!   autoplay <dir> [seconds] [fps]    both

use rustpal::audio::Mixer;
use rustpal::game_loop::Engine;
use rustpal::global::seed_random;
use rustpal::native_upscale::offline::{
    OfflineUpscaler, INPUT_SIZE, OUTPUT_HEIGHT, OUTPUT_SIZE, OUTPUT_WIDTH,
};
use rustpal::surface::{SCREEN_H, SCREEN_W};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use winit::keyboard::KeyCode;

const AUDIO_RATE: u32 = 44100;

/// The encoder reads `frames.rgba` in `INPUT_SIZE` strides, which only lines up
/// with what `frame_sink` wrote while the upscaler's input is the screen.
const _: () = assert!(INPUT_SIZE == SCREEN_W * SCREEN_H * 4);

/// Hands off the keyboard for the first stretch, so the recording opens with
/// the trademark reel and the full title screen (cranes, 15-second palette
/// fade) instead of skipping straight past them.
const INTRO_HOLD_MS: u64 = 12_000;

/// The walking key for each engine direction (`DIR_SOUTH` .. `DIR_EAST`).
const DIR_KEYS: [KeyCode; 4] = [
    KeyCode::ArrowDown,  // DIR_SOUTH
    KeyCode::ArrowLeft,  // DIR_WEST
    KeyCode::ArrowUp,    // DIR_NORTH
    KeyCode::ArrowRight, // DIR_EAST
];

/// One walking step per direction, in map pixels (`PAL_UpdateParty`): the map
/// is isometric, so every step moves diagonally.
const DIR_STEP: [(i32, i32); 4] = [(-16, 8), (-16, -8), (16, -8), (16, 8)];

/// Granularity of the "have I been here?" map, in map pixels — about four
/// walking steps across, so a room is a handful of cells.
const CELL: (i32, i32) = (64, 32);

/// How far ahead a direction is judged when choosing where to walk.
const LOOKAHEAD_STEPS: i32 = 4;

/// How long the party may fail to reach new ground before the pilot decides
/// its own map is the problem and shoves off in a random open direction.
/// A cell is four walking steps across, so crossing one takes a second or two
/// even when everything is going well.
const STUCK_MS: u64 = 6_000;

/// What the pilot is doing right now.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// Nothing held (between key presses).
    Idle,
    /// Holding a walking key.
    Walk,
    /// Holding the confirm key.
    Confirm,
}

/// A synthetic player.  It sees what a player sees — where the party is, which
/// way is walkable, whether the world is still ticking, whether a battle is on
/// — and does what a player does: hold a key down, let it go, wait.  It heads
/// for the parts of the map it has not been to yet, which is what gets it out
/// of the first room and on with the story.
struct Pilot {
    rng: u64,
    /// Stop the recording at this tick.
    deadline: u64,
    act: Act,
    held: Option<KeyCode>,
    /// Tick at which the current action ends.
    until: u64,
    dir: usize,
    /// Party position when the current walk started, to notice walls.
    walk_from: (i32, i32),
    /// Visit count per (scene, cell) — the pilot's memory of where it has been.
    visited: HashMap<(u16, i32, i32), u32>,
    last_cell: (u16, i32, i32),
    /// Last observed `frame_num` and when it last moved: the world only ticks
    /// while we are walking around, so a frozen counter means a script,
    /// dialog or menu has the floor and confirm is the only useful key.
    last_frame_num: u32,
    frame_moved_at: u64,
    /// When the party last reached a *new cell*.  Not a new position: a pocket
    /// the frontier score keeps steering back into shows up as pacing between
    /// two adjacent tiles forever, which does change the position every step
    /// while getting precisely nowhere.
    moved_at: u64,
    /// Tick after which being stuck may trigger another shove.
    unstick_after: u64,
    next_log: u64,
}

impl Pilot {
    fn new(deadline: u64) -> Pilot {
        Pilot {
            rng: 0x5EED_1995,
            deadline,
            act: Act::Idle,
            held: None,
            until: 0,
            dir: 0,
            walk_from: (0, 0),
            visited: HashMap::new(),
            last_cell: (0, 0, 0),
            last_frame_num: 0,
            frame_moved_at: 0,
            moved_at: 0,
            unstick_after: 0,
            next_log: 0,
        }
    }

    fn rand(&mut self, n: u64) -> u64 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 33) % n
    }

    fn press(&mut self, e: &mut Engine, key: KeyCode, act: Act, ms: u64) {
        e.input.handle_key_event(key, true);
        self.held = Some(key);
        self.act = act;
        self.until = e.ticks() + ms;
    }

    fn release(&mut self, e: &mut Engine) {
        if let Some(k) = self.held.take() {
            e.input.handle_key_event(k, false);
        }
    }

    /// Where the party stands, in map pixels.
    fn party_pos(e: &Engine) -> (i32, i32) {
        (
            e.globals.viewport.0 + e.globals.partyoffset.0,
            e.globals.viewport.1 + e.globals.partyoffset.1,
        )
    }

    fn cell_of(e: &Engine, pos: (i32, i32)) -> (u16, i32, i32) {
        (
            e.globals.num_scene,
            pos.0.div_euclid(CELL.0),
            pos.1.div_euclid(CELL.1),
        )
    }

    /// Note that the party is standing here (once per cell entered, not once
    /// per poll) so the explorer knows this ground is covered.  Entering a new
    /// cell is also the definition of progress the stuck detector uses.
    fn mark_visited(&mut self, e: &Engine, now: u64) {
        let cell = Self::cell_of(e, Self::party_pos(e));
        if cell != self.last_cell {
            self.last_cell = cell;
            self.moved_at = now;
            *self.visited.entry(cell).or_insert(0) += 1;
        }
    }

    /// Which of the four directions the engine would actually let the party
    /// step into — walls, scenery and NPCs, the same test `PAL_UpdateParty`
    /// makes before it moves.
    fn free_dirs(e: &Engine) -> [bool; 4] {
        let pos = Self::party_pos(e);
        let mut free = [false; 4];
        for (dir, &(dx, dy)) in DIR_STEP.iter().enumerate() {
            free[dir] = !e.check_obstacle_with_range((pos.0 + dx, pos.1 + dy), true, 0, true);
        }
        free
    }

    /// Pick a direction to walk: whatever is walkable and leads somewhere the
    /// party has been least often, with a nudge against turning straight back.
    fn choose_direction(&mut self, e: &Engine) -> Option<usize> {
        let pos = Self::party_pos(e);
        let free = Self::free_dirs(e);
        let mut best: Option<(u32, usize)> = None;
        for (dir, &(dx, dy)) in DIR_STEP.iter().enumerate() {
            if !free[dir] {
                continue;
            }
            let ahead = (pos.0 + dx * LOOKAHEAD_STEPS, pos.1 + dy * LOOKAHEAD_STEPS);
            let seen = self
                .visited
                .get(&Self::cell_of(e, ahead))
                .copied()
                .unwrap_or(0);
            let backtrack = if dir == (self.dir + 2) % 4 { 2 } else { 0 };
            let score = seen * 8 + backtrack + self.rand(3) as u32;
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, dir));
            }
        }
        best.map(|(_, d)| d)
    }

    fn step(&mut self, e: &mut Engine) {
        let now = e.ticks();

        // Watch the world clock (see `last_frame_num`).
        if e.globals.frame_num != self.last_frame_num {
            self.last_frame_num = e.globals.frame_num;
            self.frame_moved_at = now;
        }

        let roaming = e.globals.in_main_game && e.battle.is_none();
        if now >= self.next_log {
            self.next_log = now + 5000;
            let open = if roaming {
                let free = Self::free_dirs(e);
                const NAMES: [&str; 4] = ["S", "W", "N", "E"];
                (0..4)
                    .map(|d| if free[d] { NAMES[d] } else { "-" })
                    .collect::<String>()
            } else {
                "....".into()
            };
            eprintln!(
                "[{:>6}ms] scene {:>3} pos {:?} frame {} open {open} stuck {}ms{}",
                now,
                e.globals.num_scene,
                e.globals.viewport,
                e.globals.frame_num,
                now.saturating_sub(self.moved_at),
                if e.battle.is_some() { " BATTLE" } else { "" }
            );
        }

        if now >= self.deadline {
            self.release(e);
            e.quit_requested = true;
            return;
        }

        if now < INTRO_HOLD_MS {
            return;
        }

        // Battles drive themselves through `demo_pilot`, which commits a real
        // command on the real battle menu; keep our hands off the keyboard.
        if e.battle.is_some() {
            self.release(e);
            self.act = Act::Idle;
            self.until = now + 200;
            self.moved_at = now;
            return;
        }

        // Keep the coverage map up to date while walking, not just at the
        // moments a decision is made.  "Stuck" only means anything while the
        // party is free to walk, so the clock restarts whenever it is not.
        if roaming {
            self.mark_visited(e, now);
        } else {
            self.moved_at = now;
        }

        if now < self.until {
            return;
        }

        // End of an action: let the key up, then decide the next one after a
        // short gap (a key is never released and re-pressed in the same tick).
        if self.held.is_some() {
            // Walked into something the obstacle map didn't predict (a scripted
            // blocker, an NPC that stepped across): mark that way as covered so
            // the next choice goes elsewhere instead of shoving at it again.
            if self.act == Act::Walk && e.globals.viewport == self.walk_from {
                let pos = Self::party_pos(e);
                let (dx, dy) = DIR_STEP[self.dir];
                let ahead = (pos.0 + dx * LOOKAHEAD_STEPS, pos.1 + dy * LOOKAHEAD_STEPS);
                let cell = Self::cell_of(e, ahead);
                *self.visited.entry(cell).or_insert(0) += 4;
            }
            let gap = if self.act == Act::Confirm { 40 } else { 90 };
            self.release(e);
            self.act = Act::Idle;
            self.until = now + gap;
            return;
        }

        // The opening menu, cutscenes, dialogs and script menus: confirm at
        // reading speed and never touch the arrow keys (they would move the
        // menu cursor onto something we did not intend to pick).
        let scripted = !e.globals.in_main_game || now.saturating_sub(self.frame_moved_at) > 700;
        if scripted {
            self.press(e, KeyCode::Enter, Act::Confirm, 120);
            self.until += 480; // hold 120ms, then read for a beat
            return;
        }

        // A room can hem the party into a pocket that the frontier score keeps
        // steering it straight back into, and then it paces the same two tiles
        // for minutes.  When nothing has moved for a while, throw away this
        // scene's coverage — that memory is what is doing the steering — and
        // commit to a long walk in a direction drawn at random from whatever
        // is actually open.
        if now.saturating_sub(self.moved_at) > STUCK_MS && now >= self.unstick_after {
            self.unstick_after = now + STUCK_MS;
            let scene = e.globals.num_scene;
            self.visited.retain(|&(s, _, _), _| s != scene);
            let free = Self::free_dirs(e);
            let open: Vec<usize> = (0..4).filter(|&d| free[d]).collect();
            if !open.is_empty() {
                let dir = open[self.rand(open.len() as u64) as usize];
                eprintln!(
                    "[{now:>6}ms] stuck at {:?}, forgetting scene {scene} and walking {dir}",
                    e.globals.viewport,
                );
                self.dir = dir;
                self.walk_from = e.globals.viewport;
                let ms = 1400 + self.rand(900);
                self.press(e, DIR_KEYS[dir], Act::Walk, ms);
                return;
            }
        }

        // Walking around: mostly walk, occasionally investigate — the search
        // key is what opens chests, reads signs and starts a conversation with
        // whoever is standing in front of us.
        if self.rand(10) < 3 {
            self.press(e, KeyCode::Enter, Act::Confirm, 120);
            self.until += 260;
            return;
        }

        let Some(dir) = self.choose_direction(e) else {
            // Wedged in on all four sides (a script has us cornered): look
            // around instead of shoving at walls.
            self.press(e, KeyCode::Enter, Act::Confirm, 120);
            self.until += 260;
            return;
        };
        self.dir = dir;
        self.walk_from = e.globals.viewport;
        // Long enough to cross a room, short enough to react to what shows up.
        let ms = 600 + self.rand(700);
        self.press(e, DIR_KEYS[dir], Act::Walk, ms);
    }
}

/// Where a recording keeps its pieces.
struct Paths {
    dir: String,
    /// Captured 320x200 RGBA frames, back to back.
    frames: String,
    /// One engine tick (ms) per captured frame, in order.
    times: String,
    /// Headerless s16le stereo PCM at `AUDIO_RATE`.
    audio: String,
    /// Upscaled H.264, before the audio is muxed in.
    video: String,
    /// The finished file.
    out: String,
    /// Both ffmpeg passes' stderr, so a failed encode is diagnosable.
    log: String,
}

impl Paths {
    fn new(dir: &str) -> Paths {
        Paths {
            dir: dir.to_string(),
            frames: format!("{dir}/frames.rgba"),
            times: format!("{dir}/times.txt"),
            audio: format!("{dir}/audio.pcm"),
            video: format!("{dir}/video.mp4"),
            out: format!("{dir}/autoplay.mp4"),
            log: format!("{dir}/ffmpeg.log"),
        }
    }

    /// Append-mode handle on the ffmpeg log, so the second pass does not
    /// clobber the first pass's diagnostics.
    fn log_file(&self, truncate: bool) -> std::fs::File {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(truncate)
            .append(!truncate)
            .open(&self.log)
            .expect("ffmpeg log")
    }
}

/// Play the game for `seconds` of wall clock, dumping raw frames, their ticks
/// and the audio.  Deliberately cheap per frame: whatever this loop spends is
/// spent against the engine's own real-time deadlines.
fn record(paths: &Paths, seconds: u64) {
    let mut frames = std::io::BufWriter::with_capacity(
        1 << 22,
        std::fs::File::create(&paths.frames).expect("frames file"),
    );
    let mut times =
        std::io::BufWriter::new(std::fs::File::create(&paths.times).expect("times file"));
    let mut pcm = std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(&paths.audio).expect("audio file"),
    );

    seed_random(19950710);
    let mut e = Engine::new(true).expect("engine");
    // A recording is a performance, not a test: no headless escape hatches.
    // Dialogs wait for their key, menus wait for their key, the pilot presses.
    e.ui.auto_confirm = false;
    e.demo_pilot = Some(0);

    e.audio = Some(Mixer::offline(
        AUDIO_RATE,
        Box::new(move |samples: &[f32]| {
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for &s in samples {
                bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
            pcm.write_all(&bytes).expect("write audio");
        }),
    ));

    let mut captured: u64 = 0;
    e.frame_sink = Some(Box::new(move |rgba, ticks| {
        frames.write_all(rgba).expect("write frame");
        writeln!(times, "{ticks}").expect("write tick");
        captured += 1;
        if captured.is_multiple_of(500) {
            eprintln!("  captured {captured} frames, t={ticks}ms");
        }
    }));

    e.autopilot = Some(Box::new({
        let mut pilot = Pilot::new(seconds * 1000);
        move |e: &mut Engine| pilot.step(e)
    }));

    eprintln!("recording {seconds}s of autoplay into {}/ ...", paths.dir);
    e.run();

    // Dropping the sinks flushes and closes their files.
    e.frame_sink = None;
    e.autopilot = None;
    e.audio = None;
}

/// Turn a recording into `autoplay.mp4`: upscale, resample to `fps`, mux.
fn encode(paths: &Paths, fps: u64) {
    let ticks: Vec<u64> = std::io::BufReader::new(
        std::fs::File::open(&paths.times).expect("times.txt (record first?)"),
    )
    .lines()
    .map(|l| l.expect("read tick").trim().parse().expect("tick"))
    .collect();
    assert!(!ticks.is_empty(), "recording captured no frames");

    let upscaler = OfflineUpscaler::new().expect(
        "no GPU with SHADER_F16 available for the neural upscaler \
         (it is what produces the 720p image)",
    );
    eprintln!(
        "encoding {} captured frames at {fps}fps, upscaling on {}",
        ticks.len(),
        upscaler.adapter_name()
    );

    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{OUTPUT_WIDTH}x{OUTPUT_HEIGHT}"),
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            &paths.video,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(paths.log_file(true))
        .spawn()
        .expect("spawn ffmpeg (is it installed?)");
    let mut vin = std::io::BufWriter::with_capacity(1 << 22, ffmpeg.stdin.take().unwrap());

    let mut src = std::io::BufReader::with_capacity(
        1 << 22,
        std::fs::File::open(&paths.frames).expect("frames.rgba (record first?)"),
    );
    let mut frame = vec![0u8; INPUT_SIZE];
    // Opaque black, for output frames that precede the first captured one.
    let mut upscaled = vec![0u8; OUTPUT_SIZE];
    for px in upscaled.chunks_exact_mut(4) {
        px[3] = 255;
    }

    // Sample-and-hold against the engine's own tick stamps: the engine
    // presents irregularly (10 fps overworld, 25 in battle, much faster inside
    // fades), so each output frame shows the last frame presented by its time.
    // Output frame 0 sits at engine tick 0, the same origin the offline mixer
    // counts samples from, which is what keeps picture and sound together.
    let total = ticks[ticks.len() - 1] * fps / 1000 + 1;
    let mut held: i64 = -1;
    let mut upscales: u64 = 0;
    let started = std::time::Instant::now();
    for n in 0..total {
        let t = n * 1000 / fps;
        let mut want = held;
        while ((want + 1) as usize) < ticks.len() && ticks[(want + 1) as usize] <= t {
            want += 1;
        }
        if want != held {
            // The source index only ever moves forward, so the frame file is
            // read straight through — no seeking, no keeping it all in memory.
            for _ in (held + 1)..=want {
                src.read_exact(&mut frame).expect("read frame");
            }
            held = want;
            upscaler.upscale(&frame, &mut upscaled);
            upscales += 1;
        }
        vin.write_all(&upscaled).expect("write frame");
        if (n + 1).is_multiple_of(300) {
            let done = (n + 1) as f64 / total as f64;
            eprintln!(
                "  {:.0}% ({}/{total} frames, {upscales} upscaled, {:.0}s elapsed)",
                done * 100.0,
                n + 1,
                started.elapsed().as_secs_f64(),
            );
        }
    }
    drop(vin);
    let status = ffmpeg.wait().expect("ffmpeg");
    assert!(
        status.success(),
        "ffmpeg failed: {status}, see {}",
        paths.log
    );

    let mux = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            &paths.video,
            "-f",
            "s16le",
            "-ar",
            &AUDIO_RATE.to_string(),
            "-ac",
            "2",
            "-i",
            &paths.audio,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-shortest",
            &paths.out,
        ])
        .stdout(Stdio::null())
        .stderr(paths.log_file(false))
        .status()
        .expect("mux");
    assert!(mux.success(), "mux failed: {mux}, see {}", paths.log);

    println!("wrote {}", paths.out);
    let intermediates = [&paths.frames, &paths.times, &paths.audio, &paths.video];
    let bytes: u64 = intermediates
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    // Kept, not deleted: re-running `encode` on them is far cheaper than
    // replaying the game, and it is the only way to change fps or codec.
    println!(
        "intermediates in {}/ are {:.1} GiB — `autoplay encode {} <fps>` reuses them, \
         delete them when done",
        paths.dir,
        bytes as f64 / (1u64 << 30) as f64,
        paths.dir,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let num = |i: usize, default: u64| arg(i).and_then(|s| s.parse().ok()).unwrap_or(default);

    let (do_record, do_encode, dir, seconds, fps) = match arg(0) {
        Some("record") => (true, false, arg(1).unwrap_or("."), num(2, 300), 30),
        Some("encode") => (false, true, arg(1).unwrap_or("."), 0, num(2, 30)),
        Some(dir) => (true, true, dir, num(1, 300), num(2, 30)),
        None => (true, true, ".", 300, 30),
    };

    std::fs::create_dir_all(dir).expect("mkdir");
    let paths = Paths::new(dir);
    if do_record {
        record(&paths, seconds);
    }
    if do_encode {
        encode(&paths, fps);
    }
}
