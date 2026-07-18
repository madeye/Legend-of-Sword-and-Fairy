//! Voice manifest extractor: statically walks every scene's scripts to find
//! dialog lines shown with a character portrait (face), and dumps
//! `tools/voice/voice_manifest.json` (msg id, face, Big5 hex, scenes, kind)
//! plus `tools/voice/faces/face_NNN.png` (RGM.MKF portraits, palette 0) for
//! the offline TTS pipeline (tools/voice/gen_voices.py).
//!
//! Usage: voicedump [output-dir]   (default: tools/voice)

use rustpal::game_loop::Engine;
use rustpal::global::ScriptEntry;
use rustpal::surface::{self, Surface};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;

/// interpret_instruction opcodes that may jump to an operand (script.rs);
/// value = operand indices that hold jump targets.
fn jump_table() -> HashMap<u16, &'static [usize]> {
    let t: &[(u16, &[usize])] = &[
        (0x001E, &[1]),
        (0x0020, &[2]),
        (0x002E, &[2]),
        (0x0033, &[0]),
        (0x0034, &[0]),
        (0x0038, &[0]),
        (0x003A, &[0]),
        (0x0058, &[2]),
        (0x005D, &[1]),
        (0x005E, &[1]),
        (0x0061, &[0]),
        (0x0064, &[1]),
        (0x0068, &[0]),
        (0x0074, &[0]),
        (0x0079, &[1]),
        (0x0081, &[2]), // op_face_event_object
        (0x0083, &[2]),
        (0x0084, &[2]), // op_place_item
        (0x0086, &[2]),
        (0x0091, &[0]),
        (0x0094, &[2]),
        (0x0095, &[1]),
        (0x009C, &[1]), // op_enemy_division
        (0x009E, &[2]), // op_enemy_summon
    ];
    t.iter().copied().collect()
}

/// Walk all script paths reachable from `seed`, carrying the current dialog
/// face (set by 0x003C/0x003D, cleared by anything that redraws the screen),
/// and record every 0xFFFF message shown while a face is up.
fn walk(
    entries: &[ScriptEntry],
    table: &HashMap<u16, &'static [usize]>,
    seed: u16,
    scene: u16,
    visited: &mut HashSet<(u16, u16)>,
    out: &mut BTreeMap<(u16, u16), BTreeSet<u16>>,
) {
    let mut stack = vec![(seed, 0u16)];
    while let Some((entry, face)) = stack.pop() {
        if entry == 0 || entry as usize >= entries.len() || !visited.insert((entry, face)) {
            continue;
        }
        let s = entries[entry as usize];
        let op = s.operand;
        let next = entry + 1;
        match s.operation {
            // Stop running (0x004E reloads the last save).
            0x0000 | 0x004E => {}
            // Stop; the entry is replaced, so future triggers start fresh.
            0x0001 => stack.push((next, 0)),
            0x0002 => {
                stack.push((op[0], 0));
                stack.push((next, face));
            }
            // Jumps/calls within a run keep the on-screen face.
            0x0003 | 0x0004 | 0x000A => {
                if op[0] != 0 {
                    stack.push((op[0], face));
                }
                stack.push((next, face));
            }
            0x0006 => {
                if op[1] != 0 {
                    stack.push((op[1], face));
                }
                stack.push((next, face));
            }
            // Battle redraws the screen; the portrait is gone afterwards.
            0x0007 => {
                if op[1] != 0 {
                    stack.push((op[1], 0));
                }
                if op[2] != 0 {
                    stack.push((op[2], 0));
                }
                stack.push((next, 0));
            }
            0x0008 => stack.push((next, face)),
            // Screen redraws clear the portrait.
            0x0005 | 0x0009 | 0x008E => stack.push((next, 0)),
            // Dialogs without a face.
            0x003B | 0x003E => stack.push((next, 0)),
            // Dialogs with a face.
            0x003C | 0x003D => stack.push((next, op[0])),
            // Show one message line.
            0xFFFF => {
                if face > 0 {
                    out.entry((op[0], face)).or_default().insert(scene);
                }
                stack.push((next, face));
            }
            // Random forward jump over the next op[0] instructions.
            0x00A2 => {
                for k in 1..=op[0].max(1) {
                    stack.push((entry.wrapping_add(k), face));
                }
            }
            other => {
                stack.push((next, face));
                if let Some(idxs) = table.get(&other) {
                    for &i in *idxs {
                        if op[i] != 0 {
                            stack.push((op[i], face));
                        }
                    }
                }
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Speaker-caption lines ("李逍遥:") render as dialog titles, not speech.
fn is_name_line(msg: &[u8]) -> bool {
    msg.ends_with(&[0xA1, 0x47]) || msg.ends_with(b":")
}

fn dump_face_png(e: &Engine, dir: &str, face: u16) -> bool {
    let Ok(buf) = e.globals.files.rgm.chunk_decompressed(face as usize) else {
        return false;
    };
    let (fw, fh) = (surface::rle_width(&buf), surface::rle_height(&buf));
    if fw == 0 || fh == 0 {
        return false;
    }
    let mut surf = Surface::new(fw, fh);
    surf.blit_rle(&buf, 0, 0);
    let pal = e.get_palette(0, false).expect("palette 0");
    let mut rgb = Vec::with_capacity(fw * fh * 3);
    for &px in surf.pixels.iter() {
        rgb.extend_from_slice(&pal[px as usize]);
    }
    let file = std::fs::File::create(format!("{dir}/face_{face:03}.png")).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), fw as u32, fh as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().expect("png header");
    w.write_image_data(&rgb).expect("png data");
    true
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tools/voice".into());
    std::fs::create_dir_all(format!("{dir}/faces")).expect("mkdir");

    let mut e = Engine::new(true).expect("engine");
    e.globals.load_default_game().expect("default game");

    let table = jump_table();
    let entries = &e.globals.game.script_entries;
    let scenes = &e.globals.game.scenes;
    let objects = &e.globals.game.event_objects;

    // (msg id, face) -> scenes that can show it.
    let mut out: BTreeMap<(u16, u16), BTreeSet<u16>> = BTreeMap::new();

    for s in 1..scenes.len() as u16 {
        let scene = &scenes[s as usize - 1];
        let mut visited = HashSet::new();
        let mut seeds = vec![scene.script_on_enter, scene.script_on_teleport];
        let start = (scene.event_object_index as usize).min(objects.len());
        let end = if (s as usize) < scenes.len() {
            (scenes[s as usize].event_object_index as usize).min(objects.len())
        } else {
            objects.len()
        };
        for obj in &objects[start..end.max(start)] {
            seeds.push(obj.trigger_script);
            seeds.push(obj.auto_script);
        }
        for seed in seeds {
            walk(entries, &table, seed, s, &mut visited, &mut out);
        }
    }

    // Manifest.
    let mut faces_seen: BTreeSet<u16> = BTreeSet::new();
    let mut json = String::from("{\n  \"messages\": [\n");
    let mut n_name = 0usize;
    for (i, ((msg, face), scene_set)) in out.iter().enumerate() {
        let bytes = e.texts.msg(*msg as usize);
        let kind = if is_name_line(&bytes) {
            n_name += 1;
            "name"
        } else {
            "dialog"
        };
        faces_seen.insert(*face);
        let scenes_s = scene_set
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        json.push_str(&format!(
            "    {{\"msg\": {msg}, \"face\": {face}, \"big5_hex\": \"{}\", \"scenes\": [{scenes_s}], \"kind\": \"{kind}\"}}{}\n",
            hex(&bytes),
            if i + 1 == out.len() { "" } else { "," },
        ));
    }
    json.push_str("  ],\n  \"faces_seen\": [");
    json.push_str(
        &faces_seen
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
    json.push_str("]\n}\n");
    let mut f = std::fs::File::create(format!("{dir}/voice_manifest.json")).expect("manifest");
    f.write_all(json.as_bytes()).expect("write manifest");

    // Portraits.
    let mut n_png = 0;
    for &face in &faces_seen {
        if dump_face_png(&e, &format!("{dir}/faces"), face) {
            n_png += 1;
        }
    }

    let uniq_msgs: BTreeSet<u16> = out.keys().map(|(m, _)| *m).collect();
    println!(
        "voiced (msg,face) pairs: {} ({} unique msgs, {} name lines), faces: {} ({} PNGs), scenes touched: {}",
        out.len(),
        uniq_msgs.len(),
        n_name,
        faces_seen.len(),
        n_png,
        out.values().flatten().collect::<BTreeSet<_>>().len(),
    );
}
