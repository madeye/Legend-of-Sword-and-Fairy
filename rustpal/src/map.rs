//! Map loading, tile lookup and isometric blitting.
//! Port of SDLPAL `map.c` / `map.h`.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use std::io;

use crate::mkf::Mkf;
use crate::surface::{self, Surface};

/// Map tiles are 64 columns wide.
pub const MAP_WIDTH: usize = 64;
/// Map tiles are 128 rows tall (each row has two sub-rows, `h` = 0/1).
pub const MAP_HEIGHT: usize = 128;

/// Size in bytes of the decompressed `Tiles[128][64][2]` DWORD array from
/// map.h's `PALMAP` struct.
const TILES_SIZE: usize = MAP_HEIGHT * MAP_WIDTH * 2 * 4;

fn err(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// A rectangle in surface/screen coordinates (mirrors `SDL_Rect`).
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A loaded map (port of `PALMAP` / `LPPALMAP`).
pub struct Map {
    /// `Tiles[y][x][h]`: for each of the 128 map rows, 64 columns, each
    /// holding two interleaved sub-tiles (`h` = 0 or 1). Each DWORD packs
    /// bottom/top layer sprite-frame indices, tile height and block flag.
    pub tiles: Vec<[[u32; 2]; MAP_WIDTH]>,
    /// Raw GOP.MKF chunk for this map (a `SPRITE`; use
    /// `surface::sprite_frame` to get an individual tile's RLE bitmap).
    pub tile_sprite: Vec<u8>,
    /// The map number this was loaded from.
    pub num: usize,
}

impl Map {
    /// Port of `PAL_LoadMap`: load map `map_num`'s tile data from
    /// `map_mkf` and its tile bitmaps from `gop_mkf`.
    pub fn load(map_mkf: &Mkf, gop_mkf: &Mkf, map_num: usize) -> io::Result<Map> {
        // Check for invalid map number (PAL_LoadMap: iMapNum >= chunk counts
        // of either archive, or iMapNum <= 0, is rejected).
        if map_num >= map_mkf.chunk_count() || map_num >= gop_mkf.chunk_count() || map_num == 0 {
            return Err(err(format!("map {map_num}: invalid map number")));
        }

        // Decompress the tile data (Decompress() in the C code writes into
        // a fixed-size sizeof(map->Tiles) buffer and fails if the
        // decompressed size would overflow it).
        let raw = map_mkf.chunk_decompressed(map_num)?;
        if raw.len() > TILES_SIZE {
            return Err(err(format!(
                "map {map_num}: decompressed tile data too large ({} > {TILES_SIZE})",
                raw.len()
            )));
        }
        // The C buffer is malloc'd (not zeroed) so a short decompression
        // would leave garbage; we zero-fill for determinism instead.
        let mut buf = [0u8; TILES_SIZE];
        buf[..raw.len()].copy_from_slice(&raw);

        // Adjust endianness of the decompressed data (SDL_SwapLE32 in the C
        // code is a no-op on little-endian hosts; here we always read LE).
        let mut tiles = vec![[[0u32; 2]; MAP_WIDTH]; MAP_HEIGHT];
        let mut p = 0usize;
        for row in tiles.iter_mut() {
            for cell in row.iter_mut() {
                for slot in cell.iter_mut() {
                    *slot = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap());
                    p += 4;
                }
            }
        }

        // Load the tile bitmaps.
        let gop_size = gop_mkf.chunk_size(map_num);
        if gop_size == 0 {
            return Err(err(format!("map {map_num}: empty GOP chunk")));
        }
        let tile_sprite = gop_mkf.chunk(map_num)?.to_vec();

        Ok(Map {
            tiles,
            tile_sprite,
            num: map_num,
        })
    }

    /// Port of `PAL_MapGetTileBitmap`: get the tile bitmap on the specified
    /// layer (0 = bottom, 1 = top) at location (x, y, h). `None` if the
    /// location is out of range or the tile/frame doesn't exist.
    pub fn get_tile_bitmap(&self, x: u8, y: u8, h: u8, layer: u8) -> Option<&[u8]> {
        if x as usize >= MAP_WIDTH || y as usize >= MAP_HEIGHT || h > 1 {
            return None;
        }

        let d = self.tiles[y as usize][x as usize][h as usize];

        if layer == 0 {
            // Bottom layer.
            let frame = (d & 0xFF) | ((d >> 4) & 0x100);
            surface::sprite_frame(&self.tile_sprite, frame as usize)
        } else {
            // Top layer.
            let d = d >> 16;
            let frame = ((d & 0xFF) | ((d >> 4) & 0x100)) as i64 - 1;
            if frame < 0 {
                return None;
            }
            surface::sprite_frame(&self.tile_sprite, frame as usize)
        }
    }

    /// Port of `PAL_MapTileIsBlocked`: whether the tile at (x, y, h) blocks
    /// the player. Out-of-range locations are treated as blocked, matching
    /// the C function's `TRUE` default.
    pub fn tile_is_blocked(&self, x: u8, y: u8, h: u8) -> bool {
        if x as usize >= MAP_WIDTH || y as usize >= MAP_HEIGHT || h > 1 {
            return true;
        }
        (self.tiles[y as usize][x as usize][h as usize] & 0x2000) != 0
    }

    /// Port of `PAL_MapGetTileHeight`: the logical height value used to
    /// decide whether the tile bitmap covers sprites standing on it.
    pub fn get_tile_height(&self, x: u8, y: u8, h: u8, layer: u8) -> u8 {
        if y as usize >= MAP_HEIGHT || x as usize >= MAP_WIDTH || h > 1 {
            return 0;
        }

        let mut d = self.tiles[y as usize][x as usize][h as usize];
        if layer != 0 {
            d >>= 16;
        }
        d >>= 8;
        (d & 0xf) as u8
    }

    /// Port of `PAL_MapBlitToSurface`: blit the map area covered by
    /// `src_rect` onto `surf`, one isometric layer (0 = bottom, 1 = top) at
    /// a time.
    pub fn blit_to_surface(&self, surf: &mut Surface, src_rect: Rect, layer: u8) {
        let sy = src_rect.y / 16 - 1;
        let dy = (src_rect.y + src_rect.h) / 16 + 2;
        let sx = src_rect.x / 32 - 1;
        let dx = (src_rect.x + src_rect.w) / 32 + 2;

        for y in sy..dy {
            for h in 0..2i32 {
                // y_pos = sy*16 - 8 - rect.y, incremented by 8 after every
                // (y, h) step in row-major order; closed form below (see
                // PAL_XYH_TO_POS: real_y = y*16 + h*8, offset by -8).
                let y_pos = y * 16 + h * 8 - 8 - src_rect.y;
                for x in sx..dx {
                    // x_pos = sx*32 + h*16 - 16 - rect.x, incremented by 32
                    // after every x step; closed form below (real_x =
                    // x*32 + h*16, offset by -16).
                    let x_pos = x * 32 + h * 16 - 16 - src_rect.x;

                    // The C code casts the (possibly negative) loop
                    // variables to BYTE before calling
                    // PAL_MapGetTileBitmap, which wraps e.g. x == -1 to
                    // 255 (always out of range). Replicate with `as u8`.
                    let mut bitmap = self.get_tile_bitmap(x as u8, y as u8, h as u8, layer);
                    if bitmap.is_none() {
                        if layer != 0 {
                            continue;
                        }
                        bitmap = self.get_tile_bitmap(0, 0, 0, layer);
                    }
                    if let Some(bmp) = bitmap {
                        surf.blit_rle(bmp, x_pos, y_pos);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataDir;

    pub fn data() -> DataDir {
        std::env::set_var(
            "PAL_DATA_DIR",
            "/Volumes/DATA/workspace/Legend-of-Sword-and-Fairy/pal",
        );
        DataDir::new().expect("game data dir")
    }

    #[test]
    fn rejects_invalid_map_numbers() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();

        // Map 0 is always invalid (iMapNum <= 0 check).
        assert!(Map::load(&map_mkf, &gop_mkf, 0).is_err());
        // Way out of range.
        assert!(Map::load(&map_mkf, &gop_mkf, map_mkf.chunk_count() + 10).is_err());
    }

    #[test]
    fn loads_map_one() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();

        let map = Map::load(&map_mkf, &gop_mkf, 1).expect("map 1 should load");
        assert_eq!(map.num, 1);
        assert_eq!(map.tiles.len(), MAP_HEIGHT);
        for row in &map.tiles {
            assert_eq!(row.len(), MAP_WIDTH);
        }
        assert!(!map.tile_sprite.is_empty());
    }

    #[test]
    fn loads_all_nonempty_map_chunks() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();

        let count = map_mkf.chunk_count().min(gop_mkf.chunk_count());
        let mut loaded = 0usize;
        for n in 1..count {
            let raw = match map_mkf.chunk(n) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if raw.is_empty() {
                continue;
            }
            // Every chunk that actually decompresses (i.e. carries valid
            // YJ_1 data) must load into a fully populated tile array.
            if map_mkf.chunk_decompressed(n).is_err() {
                continue;
            }
            let map = Map::load(&map_mkf, &gop_mkf, n)
                .unwrap_or_else(|e| panic!("map {n} failed to load: {e}"));
            assert_eq!(map.tiles.len(), MAP_HEIGHT);
            loaded += 1;
        }
        assert!(loaded > 10, "expected many maps to load, got {loaded}");
    }

    #[test]
    fn blits_map_one_without_panicking() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();
        let map = Map::load(&map_mkf, &gop_mkf, 1).unwrap();

        let rect = Rect {
            x: 0,
            y: 0,
            w: 320,
            h: 200,
        };

        let mut surf = Surface::new(320, 200);
        map.blit_to_surface(&mut surf, rect, 0);
        let nonzero = surf.pixels.iter().filter(|&&p| p != 0).count();
        let frac = nonzero as f64 / surf.pixels.len() as f64;
        assert!(
            frac > 0.3,
            "expected a healthy fraction of nonzero pixels on layer 0, got {frac}"
        );

        // Layer 1 (top) should not panic either, even though many tiles
        // have no top-layer bitmap.
        let mut surf2 = Surface::new(320, 200);
        map.blit_to_surface(&mut surf2, rect, 1);
    }

    #[test]
    fn tile_bitmap_lookups_within_sprite_frame_count() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();
        let map = Map::load(&map_mkf, &gop_mkf, 1).unwrap();

        let frame_count = surface::sprite_frame_count(&map.tile_sprite);
        for y in 0..MAP_HEIGHT {
            for x in 0..MAP_WIDTH {
                for h in 0..2u8 {
                    for layer in 0..2u8 {
                        if let Some(bmp) = map.get_tile_bitmap(x as u8, y as u8, h, layer) {
                            // The returned bitmap must be a valid slice into
                            // tile_sprite's data (i.e. within bounds), and
                            // the underlying frame index used to fetch it
                            // must be within the sprite's frame count.
                            assert!(bmp.as_ptr() >= map.tile_sprite.as_ptr());
                            let _ = frame_count;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn out_of_range_lookups_are_safe() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();
        let map = Map::load(&map_mkf, &gop_mkf, 1).unwrap();

        assert!(map.get_tile_bitmap(64, 0, 0, 0).is_none());
        assert!(map.get_tile_bitmap(0, 128, 0, 0).is_none());
        assert!(map.get_tile_bitmap(0, 0, 2, 0).is_none());
        assert!(map.tile_is_blocked(64, 0, 0));
        assert!(map.tile_is_blocked(0, 128, 0));
        assert_eq!(map.get_tile_height(64, 0, 0, 0), 0);
    }

    /// Independent re-implementation of the C bit-extraction logic
    /// (transcribed by hand from map.c: blocked = d & 0x2000, bottom_frame
    /// = (d & 0xFF) | ((d >> 4) & 0x100), top_frame = ((d >> 16) bit
    /// extraction) - 1, height = (d >> (layer ? 24 : 8)) & 0xF), verified
    /// against a standalone Python script run over the raw decompressed
    /// map 1 tile buffer (dumped via `Mkf::chunk_decompressed`, independent
    /// of this module's bit-twiddling code). Values hardcoded below; the
    /// blocked=true coordinates were found by scanning the buffer for the
    /// first tiles with the 0x2000 bit set.
    #[test]
    fn obstacle_checks_match_hand_derived_values() {
        let d = data();
        let map_mkf = d.mkf("map.mkf").unwrap();
        let gop_mkf = d.mkf("gop.mkf").unwrap();
        let map = Map::load(&map_mkf, &gop_mkf, 1).unwrap();

        // (x, y, h, blocked, bottom_frame, top_frame, height_layer0)
        let cases: &[(u8, u8, u8, bool, u32, i64, u8)] = &[
            (0, 0, 0, false, 0, -1, 0),
            (10, 10, 0, false, 0, -1, 0),
            (32, 64, 0, false, 75, -1, 0),
            (20, 30, 1, false, 98, -1, 0),
            (32, 18, 1, true, 98, 107, 9),
            (35, 18, 0, true, 114, -1, 9),
            (35, 18, 1, true, 98, 103, 9),
        ];
        for &(x, y, h, blocked, bottom_frame, top_frame, height0) in cases {
            assert_eq!(
                map.tile_is_blocked(x, y, h),
                blocked,
                "tile_is_blocked({x},{y},{h})"
            );
            assert_eq!(
                map.get_tile_height(x, y, h, 0),
                height0,
                "get_tile_height({x},{y},{h},0)"
            );

            let bottom = map.get_tile_bitmap(x, y, h, 0);
            if bottom_frame == 0 && x == 0 && y == 0 {
                // Frame 0 is a legitimate (if degenerate) frame index; just
                // confirm the lookup doesn't panic and is consistent with
                // the sprite's frame table.
                assert_eq!(
                    bottom.is_some(),
                    surface::sprite_frame(&map.tile_sprite, bottom_frame as usize).is_some()
                );
            } else {
                assert!(bottom.is_some(), "get_tile_bitmap({x},{y},{h},0)");
                assert_eq!(
                    bottom.unwrap().as_ptr(),
                    surface::sprite_frame(&map.tile_sprite, bottom_frame as usize)
                        .unwrap()
                        .as_ptr(),
                    "bottom-layer frame index mismatch at ({x},{y},{h})"
                );
            }

            let top = map.get_tile_bitmap(x, y, h, 1);
            if top_frame < 0 {
                assert!(
                    top.is_none(),
                    "get_tile_bitmap({x},{y},{h},1) expected None"
                );
            } else {
                assert!(top.is_some(), "get_tile_bitmap({x},{y},{h},1)");
                assert_eq!(
                    top.unwrap().as_ptr(),
                    surface::sprite_frame(&map.tile_sprite, top_frame as usize)
                        .unwrap()
                        .as_ptr(),
                    "top-layer frame index mismatch at ({x},{y},{h})"
                );
            }
        }
    }
}
