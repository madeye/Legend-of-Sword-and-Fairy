//! The engine core: the `Engine` struct that owns all game state (the Rust
//! equivalent of SDLPAL's globals), the winit/pixels video shell (video.c),
//! frame timing (UTIL_Delay / PAL_ProcessEvent), and the palette effects of
//! palette.c plus the screen transitions of video.c.
//!
//! Other engine modules (scene.rs, script.rs, ui.rs, play.rs, battle.rs)
//! extend `Engine` with their own `impl` blocks.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Cursor;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use web_time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use pixels::{Pixels, SurfaceTexture};
#[cfg(not(target_arch = "wasm32"))]
use winit::application::ApplicationHandler;
#[cfg(not(target_arch = "wasm32"))]
use winit::event::{ElementState, WindowEvent};
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(not(target_arch = "wasm32"))]
use winit::keyboard::PhysicalKey;
#[cfg(not(target_arch = "wasm32"))]
use winit::platform::pump_events::EventLoopExtPumpEvents;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::{Window, WindowId};

#[cfg(target_arch = "wasm32")]
use crate::web::Video;

use crate::data::DataDir;
use crate::font::Font;
use crate::global::Globals;
use crate::input::InputState;
use crate::mkf::Mkf;
use crate::res::Resources;
use crate::surface::{Surface, SCREEN_H, SCREEN_W};
use crate::text::Texts;

/// Scene frame time (game.h: FPS = 10).
pub const FRAME_TIME: u64 = 100;
/// Battle frame time (battle.h: BATTLE_FPS = 25).
pub const BATTLE_FRAME_TIME: u64 = 40;

/// Native presentation size. The original 8:5 framebuffer is rendered into a
/// 1152x720 viewport with narrow pillarboxes, preserving its geometry instead
/// of stretching it to 16:9. (The web build presents the raw 320x200 frame
/// and lets CSS scale it.)
#[cfg(not(target_arch = "wasm32"))]
pub const DISPLAY_W: usize = 1280;
#[cfg(not(target_arch = "wasm32"))]
pub const DISPLAY_H: usize = 720;
#[cfg(not(target_arch = "wasm32"))]
const VIEW_W: usize = 1152;
#[cfg(not(target_arch = "wasm32"))]
const VIEW_X: usize = (DISPLAY_W - VIEW_W) / 2;

#[cfg(not(target_arch = "wasm32"))]
const OPENING_MENU_HD_PNG: &[u8] = include_bytes!("../resources/hd/opening-menu-720p.png");

/// One entry of the hardware palette.
pub type PalColor = [u8; 3];

/// Dialog voice-over kill switch (`PAL_VOICE=0`; native only).
fn voice_enabled() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var("PAL_VOICE").map_or(true, |v| v != "0");
    #[cfg(target_arch = "wasm32")]
    true
}

/// UTIL_Delay's underlying sleep. On the web the engine runs inside a Web
/// Worker where blocking is allowed but `std::thread::sleep` panics, so it
/// parks on `Atomics.wait` instead.
pub(crate) fn sleep_ms(ms: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(std::time::Duration::from_millis(ms));
    #[cfg(target_arch = "wasm32")]
    crate::web::sleep_ms(ms);
}

/// Convert an indexed surface + palette to RGBA, applying the
/// VIDEO_UpdateScreen shake effect (shift up/down by `level` lines depending
/// on frame parity; blank the rest). Shared by the native and web backends.
pub(crate) fn render_rgba(
    surf: &Surface,
    palette: &[PalColor; 256],
    shake: Option<(u16, u16)>,
    rgba: &mut [u8],
) {
    match shake {
        None => {
            for (i, &px) in surf.pixels.iter().enumerate() {
                let c = palette[px as usize];
                let o = i * 4;
                rgba[o] = c[0];
                rgba[o + 1] = c[1];
                rgba[o + 2] = c[2];
                rgba[o + 3] = 0xff;
            }
        }
        Some((time, level)) => {
            rgba.fill(0);
            let level = level as usize % SCREEN_H;
            let h = SCREEN_H - level;
            let (src_y0, dst_y0) = if time & 1 != 0 {
                (level, 0)
            } else {
                (0, level)
            };
            for y in 0..h {
                let sy = src_y0 + y;
                let dy = dst_y0 + y;
                for x in 0..SCREEN_W {
                    let c = palette[surf.pixels[sy * SCREEN_W + x] as usize];
                    let o = (dy * SCREEN_W + x) * 4;
                    rgba[o] = c[0];
                    rgba[o + 1] = c[1];
                    rgba[o + 2] = c[2];
                    rgba[o + 3] = 0xff;
                }
            }
        }
    }
}

// ===========================================================================
// Video shell (video.c): winit window + pixels framebuffer, pumped
// synchronously so the imperative game flow of the original engine works.
// ===========================================================================

#[cfg(not(target_arch = "wasm32"))]
struct VideoApp {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    key_events: Vec<(winit::keyboard::KeyCode, bool)>,
    close_requested: bool,
    /// RGBA staging buffer for the native 720p presentation surface.
    rgba: Vec<u8>,
    /// Decoded ImageGen-enhanced opening-menu resource.
    opening_menu_hd: Option<Vec<u8>>,
    /// Original indexed menu background, used as an overlay mask so the live
    /// engine-rendered labels remain interactive over the enhanced artwork.
    enhanced_baseline: Option<Vec<u8>>,
    enhanced_target_palette: Option<[PalColor; 256]>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ApplicationHandler for VideoApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("rustpal — 仙劍奇俠傳")
            .with_inner_size(winit::dpi::LogicalSize::new(
                DISPLAY_W as f64,
                DISPLAY_H as f64,
            ));
        let window = Arc::new(event_loop.create_window(attrs).expect("create game window"));
        let size = window.inner_size();
        let texture = SurfaceTexture::new(size.width, size.height, window.clone());
        let pixels = Pixels::new(DISPLAY_W as u32, DISPLAY_H as u32, texture)
            .expect("create pixel framebuffer");
        self.window = Some(window);
        self.pixels = Some(pixels);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close_requested = true,
            WindowEvent::Resized(size) => {
                if let Some(p) = self.pixels.as_mut() {
                    let _ = p.resize_surface(size.width.max(1), size.height.max(1));
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.key_events
                        .push((code, event.state == ElementState::Pressed));
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(p) = self.pixels.as_mut() {
                    let _ = p.render();
                }
            }
            _ => {}
        }
    }
}

/// The window + framebuffer (None in headless mode, e.g. tests).
#[cfg(not(target_arch = "wasm32"))]
pub struct Video {
    event_loop: EventLoop<()>,
    app: VideoApp,
}

#[cfg(not(target_arch = "wasm32"))]
impl Video {
    pub fn new() -> io::Result<Video> {
        let event_loop =
            EventLoop::new().map_err(|e| io::Error::other(format!("winit event loop: {e}")))?;
        Ok(Video {
            event_loop,
            app: VideoApp {
                window: None,
                pixels: None,
                key_events: Vec::new(),
                close_requested: false,
                rgba: vec![0; DISPLAY_W * DISPLAY_H * 4],
                opening_menu_hd: decode_opening_menu_hd().ok(),
                enhanced_baseline: None,
                enhanced_target_palette: None,
            },
        })
    }

    /// Pump pending window events; returns collected key transitions.
    fn pump(&mut self) -> Vec<(winit::keyboard::KeyCode, bool)> {
        self.event_loop
            .pump_app_events(Some(Duration::ZERO), &mut self.app);
        std::mem::take(&mut self.app.key_events)
    }

    /// Present an indexed surface with the given palette.
    fn present(&mut self, surf: &Surface, palette: &[PalColor; 256], shake: Option<(u16, u16)>) {
        let Some(pixels) = self.app.pixels.as_mut() else {
            return;
        };
        render_720p(
            surf,
            palette,
            shake,
            self.app.opening_menu_hd.as_deref(),
            self.app.enhanced_baseline.as_deref(),
            self.app.enhanced_target_palette.as_ref(),
            &mut self.app.rgba,
        );
        let rgba = &self.app.rgba;
        pixels.frame_mut().copy_from_slice(rgba);
        let _ = pixels.render();
    }

    fn enable_enhanced_opening_menu(&mut self, baseline: Vec<u8>, target_palette: [PalColor; 256]) {
        if self.app.opening_menu_hd.is_some() {
            self.app.enhanced_baseline = Some(baseline);
            self.app.enhanced_target_palette = Some(target_palette);
        }
    }

    fn disable_enhanced_background(&mut self) {
        self.app.enhanced_baseline = None;
        self.app.enhanced_target_palette = None;
    }

    /// Whether the user asked to close the window.
    fn close_requested(&self) -> bool {
        self.app.close_requested
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_opening_menu_hd() -> io::Result<Vec<u8>> {
    let decoder = png::Decoder::new(Cursor::new(OPENING_MENU_HD_PNG));
    let mut reader = decoder
        .read_info()
        .map_err(|e| io::Error::other(format!("decode enhanced menu PNG: {e}")))?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| io::Error::other(format!("read enhanced menu PNG: {e}")))?;
    if info.width as usize != DISPLAY_W || info.height as usize != DISPLAY_H {
        return Err(io::Error::other(format!(
            "enhanced menu must be {DISPLAY_W}x{DISPLAY_H}, got {}x{}",
            info.width, info.height
        )));
    }
    let bytes = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgb => Ok(bytes.to_vec()),
        png::ColorType::Rgba => Ok(bytes
            .chunks_exact(4)
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect()),
        other => Err(io::Error::other(format!(
            "enhanced menu must be RGB/RGBA, got {other:?}"
        ))),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn render_720p(
    surf: &Surface,
    palette: &[PalColor; 256],
    shake: Option<(u16, u16)>,
    enhanced_rgb: Option<&[u8]>,
    enhanced_baseline: Option<&[u8]>,
    enhanced_target_palette: Option<&[PalColor; 256]>,
    out: &mut [u8],
) {
    assert_eq!(out.len(), DISPLAY_W * DISPLAY_H * 4);
    let enhanced = enhanced_rgb
        .zip(enhanced_baseline)
        .zip(enhanced_target_palette)
        .filter(|((rgb, baseline), _)| {
            rgb.len() == DISPLAY_W * DISPLAY_H * 3 && baseline.len() == SCREEN_W * SCREEN_H
        });

    if let Some(((rgb, baseline), target_palette)) = enhanced {
        let current_sum: u64 = palette.iter().flatten().map(|&v| u64::from(v)).sum();
        let target_sum: u64 = target_palette.iter().flatten().map(|&v| u64::from(v)).sum();
        let fade = current_sum
            .saturating_mul(255)
            .checked_div(target_sum)
            .unwrap_or(255)
            .min(255) as u16;
        for (src, dst) in rgb.chunks_exact(3).zip(out.chunks_exact_mut(4)) {
            dst[0] = (u16::from(src[0]) * fade / 255) as u8;
            dst[1] = (u16::from(src[1]) * fade / 255) as u8;
            dst[2] = (u16::from(src[2]) * fade / 255) as u8;
            dst[3] = 0xff;
        }

        // Composite only pixels changed from the packed background. This
        // keeps menu labels and their selection colors live and crisp while
        // the ImageGen resource supplies the high-resolution base artwork.
        for y in 0..DISPLAY_H {
            let sy = y * SCREEN_H / DISPLAY_H;
            for x in 0..DISPLAY_W {
                let sx = x * SCREEN_W / DISPLAY_W;
                let source = sy * SCREEN_W + sx;
                if surf.pixels[source] != baseline[source] {
                    let color = palette[surf.pixels[source] as usize];
                    let dst = (y * DISPLAY_W + x) * 4;
                    out[dst..dst + 3].copy_from_slice(&color);
                }
            }
        }
        return;
    }

    out.fill(0);
    for pixel in out.chunks_exact_mut(4) {
        pixel[3] = 0xff;
    }
    let (src_y0, dst_y0, visible_h) = match shake {
        None => (0, 0, SCREEN_H),
        Some((time, level)) => {
            let level = level as usize % SCREEN_H;
            if time & 1 != 0 {
                (level, 0, SCREEN_H - level)
            } else {
                (0, level, SCREEN_H - level)
            }
        }
    };
    for y in 0..DISPLAY_H {
        let logical_y = y * SCREEN_H / DISPLAY_H;
        if logical_y < dst_y0 || logical_y >= dst_y0 + visible_h {
            continue;
        }
        let sy = src_y0 + logical_y - dst_y0;
        for x in 0..VIEW_W {
            let sx = x * SCREEN_W / VIEW_W;
            let color = palette[surf.pixels[sy * SCREEN_W + sx] as usize];
            let dst = (y * DISPLAY_W + VIEW_X + x) * 4;
            out[dst..dst + 3].copy_from_slice(&color);
            out[dst + 3] = 0xff;
        }
    }
}

// ===========================================================================
// Engine.
// ===========================================================================

/// Everything the running game owns.
pub struct Engine {
    pub globals: Globals,
    pub res: Resources,
    pub texts: Texts,
    pub font: Font,

    /// The 320x200 work surface (gpScreen).
    pub screen: Surface,
    /// The backup surface (gpScreenBak).
    pub screen_bak: Surface,

    /// Current hardware palette (VIDEO_GetPalette()).
    pub palette: [PalColor; 256],
    /// PAT.MKF archive for palette loading.
    pat: Mkf,

    /// Screen shake state (video.c g_wShakeTime/g_wShakeLevel).
    pub shake_time: u16,
    pub shake_level: u16,

    /// Audio mixer (None when no output device / headless).
    pub audio: Option<crate::audio::Mixer>,
    /// MUS.MKF (RIX songs) and VOC.MKF (sound effects).
    mus: Mkf,
    voc: Mkf,
    /// Lazily loaded per-scene dialog voice banks (TTS voice-over).
    pub voice: crate::voice::VoiceState,
    /// Currently playing music number (AUDIO layer bookkeeping).
    pub cur_music: i32,

    pub input: InputState,
    video: Option<Video>,
    start: Instant,

    /// Set when the user asked to quit (window close / Alt+F4).
    pub quit_requested: bool,

    /// Ending effect sprite number (ending.c g_wCurEffectSprite).
    pub ending_effect_sprite: u16,

    /// The transient battle state (`g_Battle`).  `Some` only while a battle is
    /// running; `None` at all other times.  The battle port threads this as an
    /// explicit `&mut Battle` argument internally, but it is homed here so that
    /// script opcodes running *during* a battle (enemy turn scripts, etc.) can
    /// reach the live battle — see `Engine::run_trigger_script_in_battle`.
    pub battle: Option<Box<crate::battle::Battle>>,

    /// Headless acceleration: when set, every battle (including those started
    /// from a script opcode, which cannot pass the flag explicitly) runs with
    /// the `instant` fast path — all game logic runs, rendering/waiting is
    /// skipped.  Off in normal play; tests set it to fight real battles fast.
    pub battle_instant: bool,

    // Per-module state (owned by the respective module files).
    pub script: crate::script::ScriptState,
    pub ui: crate::ui::UiState,
    pub scene: crate::scene::SceneState,
    pub play: crate::play::PlayState,
}

impl Engine {
    /// Initialize the engine. `headless` skips window creation (tests).
    pub fn new(headless: bool) -> io::Result<Engine> {
        let data_dir = DataDir::new()?;
        let pat = data_dir.mkf("pat.mkf")?;
        let mus = data_dir.mkf("mus.mkf")?;
        let voc = data_dir.mkf("voc.mkf")?;
        let texts = Texts::load(&data_dir)?;
        let font = Font::load(&data_dir)?;
        let globals = Globals::init(data_dir)?;
        let video = if headless { None } else { Some(Video::new()?) };
        let audio = if headless {
            None
        } else {
            crate::audio::Mixer::new()
        };

        let mut engine = Engine {
            globals,
            res: Resources::new(),
            texts,
            font,
            screen: Surface::screen(),
            screen_bak: Surface::screen(),
            palette: [[0; 3]; 256],
            pat,
            shake_time: 0,
            shake_level: 0,
            audio,
            mus,
            voc,
            voice: crate::voice::VoiceState::new(),
            cur_music: 0,
            input: InputState::new(),
            video,
            start: Instant::now(),
            quit_requested: false,
            ending_effect_sprite: 0,
            battle: None,
            battle_instant: false,
            script: Default::default(),
            ui: Default::default(),
            scene: Default::default(),
            play: Default::default(),
        };
        // Headless engines (tests, tools) must never block on input.
        engine.ui.auto_confirm = headless;
        // Create the window right away so the first present works.
        engine.process_event();
        Ok(engine)
    }

    /// Milliseconds since engine start (SDL_GetTicks equivalent).
    pub fn ticks(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// PAL_ProcessEvent: pump window events and update the input state.
    pub fn process_event(&mut self) {
        if let Some(video) = self.video.as_mut() {
            for (code, pressed) in video.pump() {
                self.input.handle_key_event(code, pressed);
            }
            if video.close_requested() {
                self.quit_requested = true;
            }
        }
        // Keep the web audio ring topped up (no-op natively: cpal renders
        // in its own callback thread).
        if let Some(audio) = self.audio.as_ref() {
            audio.pump();
        }
        let now = self.ticks();
        self.input.update_keyboard_state(now);
    }

    /// UTIL_Delay: wait while still pumping events.
    pub fn delay(&mut self, ms: u64) {
        let end = self.ticks() + ms;
        loop {
            self.process_event();
            if self.ticks() >= end {
                break;
            }
            sleep_ms(5.min(end - self.ticks()));
        }
    }

    /// Wait until the given tick deadline, pumping events (the common
    /// `while (!SDL_TICKS_PASSED(...)) { PAL_ProcessEvent(); SDL_Delay(5); }`
    /// pattern).
    pub fn delay_until(&mut self, deadline: u64) {
        self.process_event();
        while self.ticks() < deadline {
            self.process_event();
            sleep_ms(5);
        }
    }

    /// VIDEO_UpdateScreen(NULL): present the work surface.
    pub fn video_update(&mut self) {
        let shake = if self.shake_time != 0 {
            let s = Some((self.shake_time, self.shake_level));
            self.shake_time -= 1;
            s
        } else {
            None
        };
        if let Some(video) = self.video.as_mut() {
            video.present(&self.screen, &self.palette, shake);
        }
    }

    /// Use the generated 720p opening artwork as the current presentation
    /// background while retaining the original indexed surface as a mask for
    /// live menu text.
    pub(crate) fn enable_enhanced_opening_menu(&mut self, target_palette: [PalColor; 256]) {
        if let Some(video) = self.video.as_mut() {
            video.enable_enhanced_opening_menu(self.screen.pixels.clone(), target_palette);
        }
    }

    pub(crate) fn disable_enhanced_background(&mut self) {
        if let Some(video) = self.video.as_mut() {
            video.disable_enhanced_background();
        }
    }

    /// Present an arbitrary surface (used by transitions on gpScreenBak).
    fn video_present_surface(&mut self, which_bak: bool) {
        let shake = if self.shake_time != 0 {
            let s = Some((self.shake_time, self.shake_level));
            self.shake_time -= 1;
            s
        } else {
            None
        };
        if let Some(video) = self.video.as_mut() {
            let surf = if which_bak {
                &self.screen_bak
            } else {
                &self.screen
            };
            video.present(surf, &self.palette, shake);
        }
    }

    /// AUDIO_PlayMusic: play RIX song `num` from MUS.MKF; num <= 0 stops
    /// the music.
    pub fn play_music(&mut self, num: i32, _looping: bool, fade_time: f32) {
        self.cur_music = num;
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        if num <= 0 {
            audio.stop_music(fade_time);
            return;
        }
        let Ok(chunk) = self.mus.chunk(num as usize) else {
            return;
        };
        if let Some(rix) = crate::rix::RixPlayer::new(chunk, audio.out_rate()) {
            audio.play_music(rix, fade_time);
        }
    }

    /// AUDIO_PlaySound: play VOC sound `num`; non-positive numbers are
    /// ignored like the C code.
    pub fn play_sound(&mut self, num: i32) {
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        if num <= 0 {
            return;
        }
        let Ok(chunk) = self.voc.chunk(num as usize) else {
            return;
        };
        if let Some(voc) = crate::voc::decode_voc(chunk) {
            audio.play_sound(voc);
        }
    }

    /// Load the current scene's dialog voice bank (lazy, LRU-cached). Called
    /// wherever a `LOAD_SCENE` resource load completes. No audio device (or
    /// `PAL_VOICE=0`) disables voice entirely.
    pub fn load_voice_bank(&mut self) {
        if self.audio.is_none() || !voice_enabled() {
            return;
        }
        self.voice
            .ensure_scene(self.globals.num_scene, &self.globals.data_dir);
    }

    /// Queue the voice clip for one dialog line (0xFFFF script op). Silent
    /// no-op unless a portrait dialog is up and the line is in the bank.
    pub fn play_dialog_voice(&mut self, msg_id: u16) {
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        if self.ui.dialog_face <= 0 {
            return;
        }
        if let Some((clip, rate)) = self.voice.get(self.globals.num_scene, msg_id as u32) {
            audio.play_voice(clip.to_vec(), rate);
        }
    }

    /// VIDEO_ShakeScreen.
    pub fn shake_screen(&mut self, time: u16, level: u16) {
        self.shake_time = time;
        self.shake_level = level;
    }

    /// VIDEO_BackupScreen (screen -> backup).
    pub fn backup_screen(&mut self) {
        self.screen_bak.pixels.copy_from_slice(&self.screen.pixels);
    }

    /// VIDEO_RestoreScreen (backup -> screen).
    pub fn restore_screen(&mut self) {
        self.screen.pixels.copy_from_slice(&self.screen_bak.pixels);
    }

    // =======================================================================
    // Palette effects (palette.c).
    // =======================================================================

    /// PAL_GetPalette.
    pub fn get_palette(&self, num: usize, night: bool) -> io::Result<[PalColor; 256]> {
        let p = crate::palette::Palette::from_mkf(&self.pat, num, night)?;
        Ok(p.colors)
    }

    /// PAL_SetPalette.
    pub fn set_palette(&mut self, num: usize, night: bool) {
        if let Ok(p) = self.get_palette(num, night) {
            self.palette = p;
            self.video_update();
        }
    }

    /// Set the raw hardware palette (VIDEO_SetPalette) and refresh.
    pub fn set_raw_palette(&mut self, palette: [PalColor; 256]) {
        self.palette = palette;
        self.video_update();
    }

    /// PAL_FadeOut.
    pub fn fade_out(&mut self, delay: u64) {
        let palette = self.palette;
        let delay = delay.max(1);
        let time = self.ticks() + delay * 10 * 60;
        loop {
            let now = self.ticks();
            if now > time {
                break;
            }
            let j = ((time - now) / delay / 10) as i64;
            if j < 0 {
                break;
            }
            let mut newpal = [[0u8; 3]; 256];
            for i in 0..256 {
                for c in 0..3 {
                    newpal[i][c] = ((palette[i][c] as i64 * j) >> 6) as u8;
                }
            }
            self.set_raw_palette(newpal);
            self.delay(10);
        }
        self.set_raw_palette([[0; 3]; 256]);
    }

    /// PAL_FadeIn.
    pub fn fade_in(&mut self, num: usize, night: bool, delay: u64) {
        let Ok(palette) = self.get_palette(num, night) else {
            return;
        };
        let delay = delay.max(1);
        let time = self.ticks() + delay * 10 * 60;
        loop {
            let now = self.ticks();
            if now > time {
                break;
            }
            let j = 60 - ((time - now) / delay / 10) as i64;
            if j > 60 {
                break;
            }
            let j = j.max(0);
            let mut newpal = [[0u8; 3]; 256];
            for i in 0..256 {
                for c in 0..3 {
                    newpal[i][c] = ((palette[i][c] as i64 * j) >> 6) as u8;
                }
            }
            self.set_raw_palette(newpal);
            self.delay(10);
        }
        self.set_raw_palette(palette);
    }

    /// PAL_SceneFade: fade in (step > 0) or out (step < 0), updating the
    /// scene during the process.
    pub fn scene_fade(&mut self, num: usize, night: bool, step: i32) {
        let Ok(palette) = self.get_palette(num, night) else {
            return;
        };
        let step = if step == 0 { 1 } else { step };
        self.globals.need_to_fade_in = false;

        let apply = |eng: &mut Engine, i: i32| {
            let deadline = eng.ticks() + 100;
            eng.input.clear_key_state();
            eng.input.reset_dir();
            eng.game_update(false);
            eng.make_scene();
            eng.video_update();
            let mut newpal = [[0u8; 3]; 256];
            for j in 0..256 {
                for c in 0..3 {
                    newpal[j][c] = ((palette[j][c] as i32 * i) >> 6) as u8;
                }
            }
            eng.palette = newpal;
            eng.video_update();
            eng.delay_until(deadline);
        };

        if step > 0 {
            let mut i = 0;
            while i < 64 {
                apply(self, i);
                i += step;
            }
        } else {
            let mut i = 63;
            while i >= 0 {
                apply(self, i);
                i += step;
            }
        }
    }

    /// PAL_PaletteFade: fade from the current palette to the given one.
    pub fn palette_fade(&mut self, num: usize, night: bool, update_scene: bool) {
        let Ok(newpalette) = self.get_palette(num, night) else {
            return;
        };
        let palette = self.palette;
        for i in 0..32u32 {
            let deadline = self.ticks()
                + if update_scene {
                    FRAME_TIME
                } else {
                    FRAME_TIME / 4
                };
            let mut t = [[0u8; 3]; 256];
            for j in 0..256 {
                for c in 0..3 {
                    t[j][c] = ((palette[j][c] as u32 * (31 - i) + newpalette[j][c] as u32 * i) / 31)
                        as u8;
                }
            }
            self.palette = t;
            if update_scene {
                self.input.clear_key_state();
                self.input.reset_dir();
                self.game_update(false);
                self.make_scene();
            }
            self.video_update();
            self.delay_until(deadline);
        }
    }

    /// PAL_ColorFade: fade the palette from/to a single palette color.
    pub fn color_fade(&mut self, delay: u64, color: u8, from: bool) {
        let Ok(palette) = self.get_palette(
            self.globals.num_palette as usize,
            self.globals.night_palette,
        ) else {
            return;
        };
        let delay = (delay * 10).max(10);

        let step_channel = |cur: &mut u8, target: u8| {
            if *cur > target {
                *cur -= 4.min(*cur - target);
            } else if *cur < target {
                *cur += 4.min(target - *cur);
            }
        };

        if from {
            let mut newpal = [palette[color as usize]; 256];
            for _ in 0..64 {
                for j in 0..256 {
                    for c in 0..3 {
                        step_channel(&mut newpal[j][c], palette[j][c]);
                    }
                }
                self.set_raw_palette(newpal);
                self.delay(delay);
            }
            self.set_raw_palette(palette);
        } else {
            let mut newpal = palette;
            let target = palette[color as usize];
            for _ in 0..64 {
                for row in newpal.iter_mut() {
                    for c in 0..3 {
                        step_channel(&mut row[c], target[c]);
                    }
                }
                self.set_raw_palette(newpal);
                self.delay(delay);
            }
            self.set_raw_palette([target; 256]);
        }
    }

    /// PAL_FadeToRed.
    pub fn fade_to_red(&mut self) {
        let Ok(palette) = self.get_palette(
            self.globals.num_palette as usize,
            self.globals.night_palette,
        ) else {
            return;
        };
        let mut newpalette = palette;

        // HACKHACK from the C code: color 0x4F -> 0x4E on the screen so
        // dialog text is not affected.
        for px in self.screen.pixels.iter_mut() {
            if *px == 0x4F {
                *px = 0x4E;
            }
        }
        self.video_update();

        for _ in 0..32 {
            for j in 0..256 {
                if j == 0x4F {
                    continue;
                }
                let color = ((palette[j][0] as i32 + palette[j][1] as i32 + palette[j][2] as i32)
                    / 4
                    + 64) as u8;
                for cur in newpalette[j].iter_mut() {
                    if *cur > color {
                        *cur -= 8.min(*cur - color);
                    } else if *cur < color {
                        *cur += 8.min(color - *cur);
                    }
                }
            }
            self.set_raw_palette(newpalette);
            self.delay(75);
        }
    }

    // =======================================================================
    // Screen transitions (video.c).
    // =======================================================================

    /// VIDEO_SwitchScreen: interleaved-pixel switch from backup to screen.
    pub fn switch_screen(&mut self, speed: u16) {
        const RG_INDEX: [usize; 6] = [0, 3, 1, 5, 2, 4];
        let speed = (speed as u64 + 1) * 10;

        for &start in RG_INDEX.iter() {
            let mut j = start;
            while j < SCREEN_W * SCREEN_H {
                self.screen_bak.pixels[j] = self.screen.pixels[j];
                j += 6;
            }
            self.video_present_surface(true);
            self.delay(speed);
        }
    }

    // =======================================================================
    // Boot flow (main.c / game.c).
    // =======================================================================

    /// PAL_TrademarkScreen.
    pub fn trademark_screen(&mut self) {
        self.set_palette(3, false);
        self.rng_play(6, 0, -1, 25);
        self.delay(1000);
        self.fade_out(1);
    }

    /// PAL_SplashScreen: the scrolling title screen with cranes.
    pub fn splash_screen(&mut self) {
        // DOS chunk numbers (main.c).
        const BITMAPNUM_SPLASH_UP: usize = 0x26;
        const BITMAPNUM_SPLASH_DOWN: usize = 0x27;
        const SPRITENUM_SPLASH_TITLE: usize = 0x47;
        const SPRITENUM_SPLASH_CRANE: usize = 0x49;
        const NUM_RIX_TITLE: i32 = 0x05;

        let Ok(palette) = self.get_palette(1, false) else {
            return;
        };
        let Ok(bitmap_up) = self
            .globals
            .files
            .fbp
            .chunk_decompressed(BITMAPNUM_SPLASH_UP)
        else {
            return;
        };
        let Ok(bitmap_down) = self
            .globals
            .files
            .fbp
            .chunk_decompressed(BITMAPNUM_SPLASH_DOWN)
        else {
            return;
        };
        let Ok(title_sprite) = self
            .globals
            .files
            .mgo
            .chunk_decompressed(SPRITENUM_SPLASH_TITLE)
        else {
            return;
        };
        let Ok(crane_sprite) = self
            .globals
            .files
            .mgo
            .chunk_decompressed(SPRITENUM_SPLASH_CRANE)
        else {
            return;
        };

        // The title RLE frame is mutated to animate its height (HACKHACK in
        // the C code): copy it out so we can modify the height field.
        let mut title: Vec<u8> = match crate::surface::sprite_frame(&title_sprite, 0) {
            Some(f) => f.to_vec(),
            None => return,
        };
        // Height field offset: after the optional 0x00000002 header.
        let title_h_off = if title.len() >= 4 && title[0] == 0x02 && title[1] == 0 && title[2] == 0
        {
            6
        } else {
            2
        };
        let title_height = crate::surface::rle_height(&title);
        title[title_h_off] = 0;
        title[title_h_off + 1] = 0;

        // Generate the positions of the cranes.
        let mut cranepos = [[0i32; 3]; 9];
        for pos in cranepos.iter_mut() {
            pos[0] = crate::global::random_long(300, 600);
            pos[1] = crate::global::random_long(0, 80);
            pos[2] = crate::global::random_long(0, 8);
        }

        self.play_music(NUM_RIX_TITLE, true, 2.0);

        self.process_event();
        self.input.clear_key_state();

        let begin_time = self.ticks();
        let mut img_pos = 200usize;
        let mut crane_frame = 0u32;
        let crane_count = crate::surface::sprite_frame_count(&crane_sprite).max(1);

        loop {
            self.process_event();
            if self.quit_requested {
                return;
            }
            let mut time = self.ticks() - begin_time;

            // Fade the palette in over 15 seconds.
            if time < 15000 {
                let mut pal = [[0u8; 3]; 256];
                for i in 0..256 {
                    for c in 0..3 {
                        pal[i][c] = (palette[i][c] as f32 * (time as f32 / 15000.0)) as u8;
                    }
                }
                self.palette = pal;
            } else {
                self.palette = palette;
            }

            if img_pos > 1 {
                img_pos -= 1;
            }

            // Upper part scrolling up, lower part scrolling in from below.
            crate::surface::copy_rows(&bitmap_up, img_pos, &mut self.screen, 0, 200 - img_pos);
            crate::surface::copy_rows(&bitmap_down, 0, &mut self.screen, 200 - img_pos, img_pos);

            // The cranes.
            for pos in cranepos.iter_mut() {
                pos[2] = (pos[2] + (crane_frame & 1) as i32) % crane_count.min(8) as i32;
                if img_pos > 1 && (img_pos & 1) != 0 {
                    pos[1] += 1;
                }
                if let Some(f) = crate::surface::sprite_frame(&crane_sprite, pos[2] as usize) {
                    self.screen.blit_rle(f, pos[0], pos[1]);
                }
                pos[0] -= 1;
            }
            crane_frame += 1;

            // The title, growing taller each frame.
            if crate::surface::rle_height(&title) < title_height {
                let w = (title[title_h_off] as u16 | ((title[title_h_off + 1] as u16) << 8)) + 1;
                title[title_h_off] = (w & 0xff) as u8;
                title[title_h_off + 1] = (w >> 8) as u8;
            }
            self.screen.blit_rle(&title, 255, 10);
            self.video_update();

            // Key press: complete the fade and leave.
            if self
                .input
                .pressed(crate::input::KEY_MENU | crate::input::KEY_SEARCH)
            {
                title[title_h_off] = (title_height & 0xff) as u8;
                title[title_h_off + 1] = (title_height >> 8) as u8;
                self.screen.blit_rle(&title, 255, 10);
                self.video_update();

                if time < 15000 {
                    while time < 15000 {
                        let mut pal = [[0u8; 3]; 256];
                        for i in 0..256 {
                            for c in 0..3 {
                                pal[i][c] = (palette[i][c] as f32 * (time as f32 / 15000.0)) as u8;
                            }
                        }
                        self.set_raw_palette(pal);
                        self.delay(8);
                        time += 250;
                    }
                    self.delay(500);
                }
                break;
            }

            let deadline = begin_time + time + 85;
            self.delay_until(deadline);
        }

        self.play_music(0, false, 1.0);
        self.fade_out(1);
    }

    /// PAL_GameMain: opening menu, then the main frame loop.
    pub fn game_main(&mut self) {
        let slot = self.opening_menu();
        self.globals.current_save_slot = slot as u8;
        self.globals.in_main_game = true;

        self.globals.reload_in_next_tick(slot);

        let mut time = self.ticks();
        loop {
            // Load the game resources if needed.
            match self.res.load_resources(&mut self.globals) {
                Ok(flags) => {
                    if flags.global_data {
                        self.update_equipments();
                        let music = self.globals.num_music as i32;
                        self.play_music(music, true, 1.0);
                    }
                    if flags.scene {
                        self.load_voice_bank();
                    }
                }
                Err(e) => {
                    eprintln!("failed to load resources: {e}");
                    return;
                }
            }

            self.input.clear_key_state();
            self.delay_until(time);
            time = self.ticks() + FRAME_TIME;

            self.start_frame();
            if self.quit_requested {
                return;
            }
        }
    }

    /// The full boot sequence (main.c main()).
    pub fn run(&mut self) {
        self.trademark_screen();
        self.splash_screen();
        if self.quit_requested {
            return;
        }
        self.game_main();
    }

    /// VIDEO_FadeScreen: blend from backup buffer to current screen with the
    /// nibble-stepping pattern of the C code.
    pub fn fade_screen(&mut self, speed: u16) {
        const RG_INDEX: [usize; 6] = [0, 3, 1, 5, 2, 4];
        let speed = (speed as u64 + 1) * 10;
        let mut time = self.ticks();

        for i in 0..12 {
            for &start in RG_INDEX.iter() {
                self.delay_until(time);
                time = self.ticks() + speed;

                let mut k = start;
                while k < SCREEN_W * SCREEN_H {
                    let a = self.screen.pixels[k];
                    let mut b = self.screen_bak.pixels[k];
                    if i > 0 {
                        if (a & 0x0F) > (b & 0x0F) {
                            b = b.wrapping_add(1);
                        } else if (a & 0x0F) < (b & 0x0F) {
                            b = b.wrapping_sub(1);
                        }
                    }
                    self.screen_bak.pixels[k] = (a & 0xF0) | (b & 0x0F);
                    k += 6;
                }
                self.video_present_surface(true);
            }
        }
        self.video_update();
    }
}

#[cfg(test)]
mod video_tests {
    use super::*;

    #[test]
    fn enhanced_menu_resource_is_exactly_720p_rgb() {
        let rgb = decode_opening_menu_hd().expect("embedded enhanced menu PNG");
        assert_eq!(rgb.len(), DISPLAY_W * DISPLAY_H * 3);
        assert!(rgb.iter().any(|&channel| channel != 0));
    }

    #[test]
    fn standard_frame_is_aspect_correct_and_pillarboxed() {
        let mut surf = Surface::screen();
        surf.clear(1);
        let mut palette = [[0u8; 3]; 256];
        palette[1] = [200, 40, 20];
        let mut out = vec![0; DISPLAY_W * DISPLAY_H * 4];
        render_720p(&surf, &palette, None, None, None, None, &mut out);

        assert_eq!(&out[..4], &[0, 0, 0, 255]);
        let first = VIEW_X * 4;
        assert_eq!(&out[first..first + 4], &[200, 40, 20, 255]);
        let last = (VIEW_X + VIEW_W - 1) * 4;
        assert_eq!(&out[last..last + 4], &[200, 40, 20, 255]);
        let right_bar = (VIEW_X + VIEW_W) * 4;
        assert_eq!(&out[right_bar..right_bar + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn enhanced_background_keeps_live_indexed_overlays() {
        let mut surf = Surface::screen();
        let baseline = surf.pixels.clone();
        surf.put_pixel(160, 100, 1);
        let mut palette = [[0u8; 3]; 256];
        palette[1] = [255, 32, 16];
        let enhanced = vec![80; DISPLAY_W * DISPLAY_H * 3];
        let mut out = vec![0; DISPLAY_W * DISPLAY_H * 4];

        render_720p(
            &surf,
            &palette,
            None,
            Some(&enhanced),
            Some(&baseline),
            Some(&palette),
            &mut out,
        );

        let center = (360 * DISPLAY_W + 640) * 4;
        assert_eq!(&out[center..center + 4], &[255, 32, 16, 255]);
        assert_eq!(&out[..4], &[80, 80, 80, 255]);
    }
}
