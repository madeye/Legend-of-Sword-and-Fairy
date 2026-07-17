//! Audio mixer: RIX music + VOC sound effects -> stereo output
//! (the AUDIO_* layer of SDLPAL audio.c, simplified to the DOS feature set:
//! RIX music via the OPL emulator and VOC sound effects).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::sync::{Arc, Mutex};

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

struct Shared {
    music: Option<RixPlayer>,
    /// Current music volume in [0, 1] and per-frame fade step.
    music_volume: f32,
    music_fade_step: f32,
    sounds: Vec<SoundInstance>,
}

/// Software mixer. Everything is rendered at the output device's rate.
pub struct Mixer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    out_rate: u32,
}

impl Mixer {
    /// Open the default output device. Returns None if no device is
    /// available (headless CI, tests).
    pub fn new() -> Option<Mixer> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let out_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let shared = Arc::new(Mutex::new(Shared {
            music: None,
            music_volume: 1.0,
            music_fade_step: 0.0,
            sounds: Vec::new(),
        }));

        let cb_shared = shared.clone();
        let mut music_buf: Vec<[i16; 2]> = Vec::new();
        let stream = device
            .build_output_stream(
                &config.config(),
                move |out: &mut [f32], _| {
                    let frames = out.len() / channels;
                    let mut shared = cb_shared.lock().unwrap();
                    if music_buf.len() < frames {
                        music_buf.resize(frames, [0, 0]);
                    }
                    let have_music = shared.music.is_some();
                    if let Some(m) = shared.music.as_mut() {
                        m.render(&mut music_buf[..frames]);
                    } else {
                        music_buf[..frames].fill([0, 0]);
                    }
                    for (i, frame) in out.chunks_mut(channels).enumerate() {
                        // Music fade.
                        if have_music && shared.music_fade_step != 0.0 {
                            shared.music_volume =
                                (shared.music_volume + shared.music_fade_step).clamp(0.0, 1.0);
                            if shared.music_volume == 0.0 && shared.music_fade_step < 0.0 {
                                shared.music = None;
                                shared.music_fade_step = 0.0;
                                music_buf[i..frames].fill([0, 0]);
                            } else if shared.music_volume == 1.0 && shared.music_fade_step > 0.0 {
                                shared.music_fade_step = 0.0;
                            }
                        }
                        let mv = shared.music_volume;
                        let mut l = music_buf[i][0] as f32 / 32768.0 * mv;
                        let mut r = music_buf[i][1] as f32 / 32768.0 * mv;
                        // Mix sound effects.
                        for s in shared.sounds.iter_mut() {
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
                    shared
                        .sounds
                        .retain(|s| ((s.pos >> 16) as usize) < s.samples.len());
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
        let mut s = self.shared.lock().unwrap();
        s.music = Some(rix);
        if fade_time > 0.0 {
            s.music_volume = 0.0;
            s.music_fade_step = 1.0 / (fade_time * self.out_rate as f32);
        } else {
            s.music_volume = 1.0;
            s.music_fade_step = 0.0;
        }
    }

    /// Stop music, fading out over `fade_time` seconds (0 = immediate).
    pub fn stop_music(&self, fade_time: f32) {
        let mut s = self.shared.lock().unwrap();
        if fade_time > 0.0 && s.music.is_some() {
            s.music_fade_step = -1.0 / (fade_time * self.out_rate as f32);
        } else {
            s.music = None;
            s.music_fade_step = 0.0;
        }
    }

    /// Fire-and-forget playback of a decoded sound effect.
    pub fn play_sound(&self, voc: VocSound) {
        let samples: Vec<i16> = voc
            .samples
            .iter()
            .map(|&b| ((b as i16) - 0x80) << 8)
            .collect();
        let step = ((voc.rate as u64) << 16) / self.out_rate.max(1) as u64;
        let mut s = self.shared.lock().unwrap();
        s.sounds.push(SoundInstance {
            samples,
            pos: 0,
            step,
        });
    }
}
