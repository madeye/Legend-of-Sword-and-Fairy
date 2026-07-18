# NPC dialog voice-over pipeline

Generates TTS voice-over for every dialog line shown with a character
portrait, using **Qwen3-TTS on MLX** locally. The engine plays the clips
from lazily loaded per-scene banks (`pal/voice/NNN.vbk`, see
`src/voice.rs`); missing banks simply mean a silent game, so all of this is
optional.

Two-stage design for consistent per-character voices:

1. **Casting** — `voice_map.json` maps each portrait (RGM.MKF chunk) to a
   character with a Chinese voice description. Qwen3-TTS **VoiceDesign**
   (1.7B) turns each description into one reference clip `refs/<id>.wav`.
   Voice design is sampled, so re-run to re-roll any you dislike (delete
   the wav first). The curated `refs/` are committed — they ARE the cast.
2. **Synthesis** — every dialog line is synthesized with Qwen3-TTS **Base**
   (0.6B) by voice-cloning the character's reference clip, so a character
   sounds identical across all lines, scenes, and future re-runs.

## Prerequisites

- [uv](https://docs.astral.sh/uv/) (manages the Python env; `uv sync` runs
  automatically on first `uv run`)
- an ffmpeg with **libvorbis** (`brew install ffmpeg-full` provides
  `/opt/homebrew/opt/ffmpeg-full/bin/ffmpeg`; auto-detected, or set
  `$FFMPEG`)
- Qwen3-TTS models download to the HF cache on first run
  (`mlx-community/Qwen3-TTS-12Hz-1.7B-VoiceDesign-bf16` ~3.4 GB and
  `...-0.6B-Base-bf16` ~1.3 GB)

## Workflow

```sh
# 1. Extract the dialog manifest + portrait PNGs from the game data
#    (writes voice_manifest.json and faces/face_NNN.png here):
PAL_DATA_DIR=pal cargo run --release --example voicedump

# 2. Casting: create/inspect reference voices (listen to refs/*.wav,
#    delete + re-run to re-roll; tweak descriptions in voice_map.json):
cd tools/voice
uv run gen_voices.py --design-refs
uv run gen_voices.py --list-faces      # coverage check

# 3. Synthesize + pack banks (a few scenes first, or everything):
uv run gen_voices.py --scenes 1 2
uv run gen_voices.py                   # full run, ~2800 lines
uv run gen_voices.py --stats
```

Synthesis results are cached in `cache/` keyed on (character, reference
clip hash, text): tweaking one character's reference only re-synthesizes
that character's lines. Banks land in `pal/voice/` (gitignored;
regenerable).

Runtime kill switch: `PAL_VOICE=0` (native).
