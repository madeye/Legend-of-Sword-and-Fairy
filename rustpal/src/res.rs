//! Resource manager (port of SDLPAL res.c): the currently loaded map,
//! event-object sprites and player sprites, reloaded according to the
//! load flags in `Globals`.
//!
//! Cross-layer effects of PAL_LoadResources (starting music after
//! kLoadGlobalData, running equipment scripts after PAL_InitGameData) are
//! signalled back to the caller via the returned `LoadedFlags`.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;

use crate::global::{Globals, LOAD_GLOBAL_DATA, LOAD_PLAYER_SPRITE, LOAD_SCENE};
use crate::global::{MAX_PLAYABLE_PLAYER_ROLES, MAX_PLAYER_ROLES};
use crate::map::Map;
use crate::surface;

/// RESOURCES.
#[derive(Default)]
pub struct Resources {
    /// Current loaded map (None until the first kLoadScene).
    pub map: Option<Map>,
    /// Sprites of the current scene's event objects (raw MGO sprite data);
    /// None for objects without a sprite.
    pub event_object_sprites: Vec<Option<Vec<u8>>>,
    /// Index of the first event object of the current scene.
    pub event_object_base: usize,
    /// Player (and follower) sprites.
    pub player_sprites: [Option<Vec<u8>>; MAX_PLAYABLE_PLAYER_ROLES],
}

/// Which load actions actually ran (so the caller can start music, run
/// equipment scripts, etc.).
#[derive(Default, Clone, Copy)]
pub struct LoadedFlags {
    pub global_data: bool,
    pub scene: bool,
    pub player_sprite: bool,
}

impl Resources {
    pub fn new() -> Resources {
        Resources::default()
    }

    /// PAL_LoadResources. Returns which parts were (re)loaded.
    pub fn load_resources(&mut self, globals: &mut Globals) -> io::Result<LoadedFlags> {
        let mut done = LoadedFlags::default();
        if globals.load_flags == 0 {
            return Ok(done);
        }

        // Load global data.
        if globals.load_flags & LOAD_GLOBAL_DATA != 0 {
            globals.init_game_data(globals.current_save_slot as i32)?;
            done.global_data = true;
            // Caller must: play music gpGlobals->wNumMusic (looping) and run
            // the equipment scripts (PAL_UpdateEquipments).
        }

        // Load scene.
        if globals.load_flags & LOAD_SCENE != 0 {
            let map_mkf = globals.data_dir.mkf("map.mkf")?;
            let gop_mkf = globals.data_dir.mkf("gop.mkf")?;

            if globals.entering_scene {
                globals.screen_wave = 0;
                globals.wave_progression = 0;
            }

            let i = globals.num_scene as usize - 1;
            let map_num = globals.game.scenes[i].map_num as usize;
            self.map = Some(Map::load(&map_mkf, &gop_mkf, map_num)?);

            // Load event-object sprites for this scene.
            let index = globals.game.scenes[i].event_object_index as usize;
            let count = globals.game.scenes[i + 1].event_object_index as usize - index;
            self.event_object_base = index;
            self.event_object_sprites.clear();
            for k in 0..count {
                let eo_index = index + k;
                let n = globals.game.event_objects[eo_index].sprite_num as usize;
                if n == 0 {
                    self.event_object_sprites.push(None);
                    continue;
                }
                let sprite = globals.files.mgo.chunk_decompressed(n)?;
                globals.game.event_objects[eo_index].sprite_frames_auto =
                    surface::sprite_frame_count(&sprite) as u16;
                self.event_object_sprites.push(Some(sprite));
            }

            globals.partyoffset = (160, 112);
            done.scene = true;
        }

        // Load player sprites.
        if globals.load_flags & LOAD_PLAYER_SPRITE != 0 {
            self.player_sprites = Default::default();

            for i in 0..=globals.max_party_member_index as usize {
                let player_id = globals.party[i].player_role as usize;
                debug_assert!(player_id < MAX_PLAYER_ROLES);
                let sprite_num = globals.game.player_roles.sprite_num[player_id] as usize;
                self.player_sprites[i] = Some(globals.files.mgo.chunk_decompressed(sprite_num)?);
            }

            for i in 1..=globals.follower_num as usize {
                // Followers store the MGO sprite number directly in
                // wPlayerRole.
                let idx = globals.max_party_member_index as usize + i;
                let sprite_num = globals.party[idx].player_role as usize;
                self.player_sprites[idx] = Some(globals.files.mgo.chunk_decompressed(sprite_num)?);
            }
            done.player_sprite = true;
        }

        globals.load_flags = 0;
        Ok(done)
    }

    /// PAL_GetPlayerSprite.
    pub fn player_sprite(&self, player_index: usize) -> Option<&[u8]> {
        self.player_sprites
            .get(player_index)?
            .as_ref()
            .map(|v| v.as_slice())
    }

    /// PAL_GetEventObjectSprite. `event_object_id` is the 1-based global
    /// event object ID.
    pub fn event_object_sprite(&self, event_object_id: usize) -> Option<&[u8]> {
        let idx = event_object_id.checked_sub(self.event_object_base + 1)?;
        self.event_object_sprites
            .get(idx)?
            .as_ref()
            .map(|v| v.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDir;

    fn globals() -> Globals {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        Globals::init(DataDir::new().expect("data dir")).expect("globals")
    }

    #[test]
    fn loads_scene_one_resources() {
        let mut g = globals();
        g.load_default_game().unwrap();
        g.max_party_member_index = 0;
        g.party[0].player_role = 0;
        g.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;

        let mut res = Resources::new();
        let done = res.load_resources(&mut g).unwrap();
        assert!(done.scene);
        assert!(done.player_sprite);
        assert!(!done.global_data);
        assert_eq!(g.load_flags, 0);

        let map = res.map.as_ref().expect("map loaded");
        assert_eq!(map.num, g.game.scenes[0].map_num as usize);

        // Scene 1 has event objects; sprites with wSpriteNum != 0 must be
        // loaded and their auto frame counts filled in.
        assert!(!res.event_object_sprites.is_empty());
        let base = res.event_object_base;
        let mut with_sprite = 0;
        for (k, spr) in res.event_object_sprites.iter().enumerate() {
            let eo = &g.game.event_objects[base + k];
            if eo.sprite_num != 0 {
                assert!(spr.is_some(), "event object {k} missing sprite");
                assert!(eo.sprite_frames_auto > 0);
                with_sprite += 1;
            } else {
                assert!(spr.is_none());
            }
        }
        assert!(with_sprite > 0, "no event object sprites in scene 1");

        // Player sprite for role 0 (Li Xiaoyao) must decode as a sprite.
        let ps = res.player_sprite(0).expect("player sprite");
        assert!(crate::surface::sprite_frame_count(ps) > 0);

        // 1-based event object sprite lookup matches direct indexing.
        let id = base + 1; // first object of the scene
        let via_id = res.event_object_sprite(id);
        assert_eq!(via_id.is_some(), res.event_object_sprites[0].is_some());
    }

    #[test]
    fn all_scenes_load() {
        const MAX_SCENES_TO_TEST: usize = 80;

        let mut g = globals();
        g.load_default_game().unwrap();
        let mut res = Resources::new();
        // Every scene with a nonzero map (and sane event object range) must
        // load without errors.
        let scene_count = g
            .game
            .scenes
            .iter()
            .take_while(|s| s.map_num != 0 || s.event_object_index != 0)
            .count();
        assert!(scene_count > 50, "unexpectedly few scenes: {scene_count}");
        for scene in 1..=scene_count.min(MAX_SCENES_TO_TEST) {
            g.num_scene = scene as u16;
            g.load_flags = LOAD_SCENE;
            if g.game.scenes[scene - 1].map_num == 0 {
                continue;
            }
            res.load_resources(&mut g)
                .unwrap_or_else(|e| panic!("scene {scene}: {e}"));
        }
    }
}
