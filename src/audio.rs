//! Audio mixer: RIX music + VOC sound effects -> stereo output
//! (the AUDIO_* layer of SDLPAL audio.c, simplified to the DOS feature set:
//! RIX music via the OPL emulator and VOC sound effects).
//!
//! The mixing core (`Shared::mix_into`) is platform-independent. Natively it
//! runs inside the cpal output-stream callback; on the web the blocked
//! engine worker renders ahead into a SharedArrayBuffer ring
//! (`Mixer::pump`, called from `Engine::process_event`) that an
//! AudioWorkletProcessor (web/worklet.js) drains on the audio thread.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::rix::RixPlayer;
use crate::voc::VocSound;

/// A playing sound effect instance.
struct SoundInstance {
    /// Mono samples, already centered (i16).
    samples: Vec<i16>,
    /// Playback position over the source in 16.16 fixed point.
    pos: u64,
    /// Source-step per output frame in 16.16 fixed point.
    step: u64,
}

impl SoundInstance {
    fn from_samples(samples: Vec<i16>, src_rate: u32, out_rate: u32) -> SoundInstance {
        let step = ((src_rate as u64) << 16) / out_rate.max(1) as u64;
        SoundInstance {
            samples,
            pos: 0,
            step,
        }
    }

    fn from_voc(voc: VocSound, out_rate: u32) -> SoundInstance {
        let samples: Vec<i16> = voc
            .samples
            .iter()
            .map(|&b| ((b as i16) - 0x80) << 8)
            .collect();
        SoundInstance::from_samples(samples, voc.rate, out_rate)
    }
}

/// Music volume multiplier while dialog voice is playing.
const VOICE_DUCK: f32 = 0.6;

struct Shared {
    music: Option<RixPlayer>,
    /// Current music volume in [0, 1] and per-frame fade step.
    music_volume: f32,
    music_fade_step: f32,
    sounds: Vec<SoundInstance>,
    /// Dialog voice queue: only the front plays; finished clips pop so
    /// consecutive dialog lines run back-to-back.
    voice: std::collections::VecDeque<SoundInstance>,
    /// Persistent scratch buffer for the music renderer.
    music_buf: Vec<[i16; 2]>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            music: None,
            music_volume: 1.0,
            music_fade_step: 0.0,
            sounds: Vec::new(),
            voice: std::collections::VecDeque::new(),
            music_buf: Vec::new(),
        }
    }

    fn start_music(&mut self, rix: RixPlayer, fade_time: f32, out_rate: u32) {
        self.music = Some(rix);
        if fade_time > 0.0 {
            self.music_volume = 0.0;
            self.music_fade_step = 1.0 / (fade_time * out_rate as f32);
        } else {
            self.music_volume = 1.0;
            self.music_fade_step = 0.0;
        }
    }

    fn stop_music(&mut self, fade_time: f32, out_rate: u32) {
        if fade_time > 0.0 && self.music.is_some() {
            self.music_fade_step = -1.0 / (fade_time * out_rate as f32);
        } else {
            self.music = None;
            self.music_fade_step = 0.0;
        }
    }

    /// Render `out.len() / channels` frames of mixed audio into the
    /// interleaved output buffer.
    fn mix_into(&mut self, out: &mut [f32], channels: usize) {
        let frames = out.len() / channels;
        if self.music_buf.len() < frames {
            self.music_buf.resize(frames, [0, 0]);
        }
        let mut music_buf = std::mem::take(&mut self.music_buf);
        let have_music = self.music.is_some();
        if let Some(m) = self.music.as_mut() {
            m.render(&mut music_buf[..frames]);
        } else {
            music_buf[..frames].fill([0, 0]);
        }
        for (i, frame) in out.chunks_mut(channels).enumerate() {
            // Music fade.
            if have_music && self.music_fade_step != 0.0 {
                self.music_volume = (self.music_volume + self.music_fade_step).clamp(0.0, 1.0);
                if self.music_volume == 0.0 && self.music_fade_step < 0.0 {
                    self.music = None;
                    self.music_fade_step = 0.0;
                    music_buf[i..frames].fill([0, 0]);
                } else if self.music_volume == 1.0 && self.music_fade_step > 0.0 {
                    self.music_fade_step = 0.0;
                }
            }
            // Dialog voice: play the queue front, popping finished clips so
            // the next line follows gaplessly. Ducks the music below speech.
            let mut voice = 0.0f32;
            while let Some(s) = self.voice.front_mut() {
                let idx = (s.pos >> 16) as usize;
                if idx < s.samples.len() {
                    voice = s.samples[idx] as f32 / 32768.0;
                    s.pos += s.step;
                    break;
                }
                self.voice.pop_front();
            }
            let mut mv = self.music_volume;
            if !self.voice.is_empty() {
                mv *= VOICE_DUCK;
            }
            let mut l = music_buf[i][0] as f32 / 32768.0 * mv + voice;
            let mut r = music_buf[i][1] as f32 / 32768.0 * mv + voice;
            // Mix sound effects.
            for s in self.sounds.iter_mut() {
                let idx = (s.pos >> 16) as usize;
                if idx < s.samples.len() {
                    let v = s.samples[idx] as f32 / 32768.0;
                    l += v;
                    r += v;
                    s.pos += s.step;
                }
            }
            frame[0] = l.clamp(-1.0, 1.0);
            if channels > 1 {
                frame[1] = r.clamp(-1.0, 1.0);
            }
            for c in frame.iter_mut().skip(2) {
                *c = 0.0;
            }
        }
        self.music_buf = music_buf;
        self.sounds
            .retain(|s| ((s.pos >> 16) as usize) < s.samples.len());
    }
}

/// Software mixer. Everything is rendered at the output device's rate.
#[cfg(not(target_arch = "wasm32"))]
pub struct Mixer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    out_rate: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl Mixer {
    /// Open the default output device. Returns None if no device is
    /// available (headless CI, tests).
    pub fn new() -> Option<Mixer> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let out_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let shared = Arc::new(Mutex::new(Shared::new()));

        let cb_shared = shared.clone();
        let stream = device
            .build_output_stream(
                &config.config(),
                move |out: &mut [f32], _| {
                    cb_shared.lock().unwrap().mix_into(out, channels);
                },
                |e| eprintln!("audio stream error: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;

        Some(Mixer {
            _stream: stream,
            shared,
            out_rate,
        })
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// Start playing a RIX song (replaces any current song). `fade_time` is
    /// in seconds; the new song fades in over it (0 = immediate).
    pub fn play_music(&self, rix: RixPlayer, fade_time: f32) {
        self.shared
            .lock()
            .unwrap()
            .start_music(rix, fade_time, self.out_rate);
    }

    /// Stop music, fading out over `fade_time` seconds (0 = immediate).
    pub fn stop_music(&self, fade_time: f32) {
        self.shared
            .lock()
            .unwrap()
            .stop_music(fade_time, self.out_rate);
    }

    /// Fire-and-forget playback of a decoded sound effect.
    pub fn play_sound(&self, voc: VocSound) {
        self.shared
            .lock()
            .unwrap()
            .sounds
            .push(SoundInstance::from_voc(voc, self.out_rate));
    }

    /// Queue a dialog voice clip after any already-playing ones.
    pub fn play_voice(&self, samples: Vec<i16>, rate: u32) {
        self.shared
            .lock()
            .unwrap()
            .voice
            .push_back(SoundInstance::from_samples(samples, rate, self.out_rate));
    }

    /// Cut dialog voice playback (player advanced the dialog).
    pub fn stop_voice(&self) {
        self.shared.lock().unwrap().voice.clear();
    }

    /// Native audio runs in the cpal callback; nothing to pump.
    pub fn pump(&self) {}
}

/// Web mixer: renders ahead into the `PAL_AUDIO` SharedArrayBuffer ring that
/// web/worklet.js drains. Layout: two i32 header cells (monotonic write /
/// read frame counters, wrapping) followed by an f32 ring of interleaved
/// stereo frames whose length is a power of two.
#[cfg(target_arch = "wasm32")]
pub struct Mixer {
    shared: std::cell::RefCell<(Shared, Vec<f32>)>,
    header: js_sys::Int32Array,
    ring: js_sys::Float32Array,
    /// Ring capacity in frames (power of two).
    ring_frames: u32,
    /// Keep this many frames rendered ahead (~250 ms).
    target_ahead: u32,
    out_rate: u32,
}

#[cfg(target_arch = "wasm32")]
impl Mixer {
    /// Attach to the audio ring installed by web/worker.js. Returns None
    /// (silent engine) when the page couldn't set up an AudioContext.
    pub fn new() -> Option<Mixer> {
        let g = js_sys::global();
        let sab = js_sys::Reflect::get(&g, &"PAL_AUDIO".into()).ok()?;
        if sab.is_undefined() || sab.is_null() {
            web_sys::console::log_1(&"rustpal: no PAL_AUDIO, running silent".into());
            return None;
        }
        let out_rate = js_sys::Reflect::get(&g, &"PAL_AUDIO_RATE".into())
            .ok()?
            .as_f64()? as u32;
        let header = js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 0, 2);
        let ring = js_sys::Float32Array::new_with_byte_offset(&sab, 8);
        let ring_frames = ring.length() / 2;
        if ring_frames == 0 || !ring_frames.is_power_of_two() {
            return None;
        }
        Some(Mixer {
            shared: std::cell::RefCell::new((Shared::new(), Vec::new())),
            header,
            ring,
            ring_frames,
            target_ahead: (out_rate / 4).min(ring_frames - 256),
            out_rate,
        })
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn play_music(&self, rix: RixPlayer, fade_time: f32) {
        self.shared
            .borrow_mut()
            .0
            .start_music(rix, fade_time, self.out_rate);
    }

    pub fn stop_music(&self, fade_time: f32) {
        self.shared
            .borrow_mut()
            .0
            .stop_music(fade_time, self.out_rate);
    }

    pub fn play_sound(&self, voc: VocSound) {
        self.shared
            .borrow_mut()
            .0
            .sounds
            .push(SoundInstance::from_voc(voc, self.out_rate));
    }

    /// Queue a dialog voice clip after any already-playing ones.
    pub fn play_voice(&self, samples: Vec<i16>, rate: u32) {
        self.shared
            .borrow_mut()
            .0
            .voice
            .push_back(SoundInstance::from_samples(samples, rate, self.out_rate));
    }

    /// Cut dialog voice playback (player advanced the dialog).
    pub fn stop_voice(&self) {
        self.shared.borrow_mut().0.voice.clear();
    }

    /// Top the ring buffer up to `target_ahead` frames. Called from
    /// `Engine::process_event`, i.e. every few ms whenever the engine pumps
    /// events (delays, menus, frame waits).
    pub fn pump(&self) {
        let write = js_sys::Atomics::load(&self.header, 0).unwrap_or(0);
        let read = js_sys::Atomics::load(&self.header, 1).unwrap_or(0);
        let ahead = write.wrapping_sub(read) as u32;
        if ahead >= self.target_ahead {
            return;
        }
        let need = self.target_ahead - ahead;

        let mut guard = self.shared.borrow_mut();
        let (shared, scratch) = &mut *guard;
        scratch.clear();
        scratch.resize(need as usize * 2, 0.0);
        shared.mix_into(scratch, 2);

        // Copy into the ring (at most two contiguous segments).
        let src = js_sys::Float32Array::from(&scratch[..]);
        let mask = self.ring_frames - 1;
        let w0 = (write as u32) & mask;
        let first = (self.ring_frames - w0).min(need);
        self.ring.set(&src.subarray(0, first * 2), w0 as u32 * 2);
        if need > first {
            self.ring.set(&src.subarray(first * 2, need * 2), 0);
        }
        let _ = js_sys::Atomics::store(&self.header, 0, write.wrapping_add(need as i32));
    }
}
