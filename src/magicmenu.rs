//! In-game / battle magic selection menu (port of SDLPAL `magicmenu.c`,
//! DOS paths: `fIsWIN95 == FALSE`, `PAL_CLASSIC`, no `desc.dat` so
//! `lpObjectDesc == NULL`).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::global::{
    MAGICFLAG_USABLE_IN_BATTLE, MAGICFLAG_USABLE_OUTSIDE_BATTLE, MAX_PLAYER_MAGICS,
};
use crate::ui::{
    NumAlign, NumColor, MENUITEM_COLOR, MENUITEM_COLOR_CONFIRMED, MENUITEM_COLOR_INACTIVE,
    MENUITEM_COLOR_SELECTED_INACTIVE, SPRITENUM_CURSOR, SPRITENUM_SLASH,
};

/// Word number of the CASH label (ui.h CASH_LABEL).
const CASH_LABEL: u16 = 21;
/// Bytes per WORD.DAT record for the Chinese DOS data (gConfig.dwWordLength).
const WORD_LENGTH: i32 = 10;

/// TIMEMETER_COLOR_DEFAULT (uibattle.h) — used by the player info boxes.
pub const TIMEMETER_COLOR_DEFAULT: u8 = 0x1B;

/// One entry of the magic list (magicmenu.c `struct MAGICITEM`).
#[derive(Clone, Copy)]
struct MagicItem {
    magic: u16,
    mp: u16,
    enabled: bool,
}

/// The magic-menu context (magicmenu.c file statics rgMagicItem /
/// g_iNumMagic / g_iCurrentItem / g_wPlayerMP), owned locally so the menu is
/// re-entrant.
pub(crate) struct MagicMenuCtx {
    items: Vec<MagicItem>,
    current: usize,
    player_mp: u16,
}

impl Engine {
    /// PAL_MagicSelectionMenuInit.
    pub(crate) fn magic_selection_menu_init(
        &self,
        player_role: u16,
        in_battle: bool,
        default_magic: u16,
    ) -> MagicMenuCtx {
        let role = player_role as usize;
        let player_mp = self.globals.game.player_roles.mp[role];
        let mut items: Vec<MagicItem> = Vec::new();

        for i in 0..MAX_PLAYER_MAGICS {
            let w = self.globals.game.player_roles.magic[i][role];
            if w == 0 {
                continue;
            }
            let magic_num = self.globals.game.objects[w as usize].magic_number();
            let mp = self.globals.game.magics[magic_num as usize].cost_mp;
            let mut enabled = mp <= player_mp;

            let flags = self.globals.game.objects[w as usize].magic_flags();
            if in_battle {
                if flags & MAGICFLAG_USABLE_IN_BATTLE == 0 {
                    enabled = false;
                }
            } else if flags & MAGICFLAG_USABLE_OUTSIDE_BATTLE == 0 {
                enabled = false;
            }

            items.push(MagicItem {
                magic: w,
                mp,
                enabled,
            });
        }

        // Sort by magic object ID (bubble sort, matching the C code).
        items.sort_by_key(|it| it.magic);

        let current = items
            .iter()
            .position(|it| it.magic == default_magic)
            .unwrap_or(0);

        MagicMenuCtx {
            items,
            current,
            player_mp,
        }
    }

    /// PAL_MagicSelectionMenuUpdate: draw one frame, returning the selected
    /// magic (0 = cancelled, 0xFFFF = not yet confirmed).
    pub(crate) fn magic_selection_menu_update(&mut self, ctx: &mut MagicMenuCtx) -> u16 {
        use crate::input::{
            KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_MENU, KEY_PGDN, KEY_PGUP, KEY_RIGHT,
            KEY_SEARCH, KEY_UP,
        };

        let items_per_line = 32 / WORD_LENGTH;
        let item_text_width = 8 * WORD_LENGTH + 7;
        let lines_per_page = 5; // 5 - ExtraMagicDescLines(0)
        let box_y_offset = 0;
        let cursor_x_offset = WORD_LENGTH * 5 / 2;
        let page_line_offset = lines_per_page / 2;
        let num_magic = ctx.items.len() as i32;

        // Input -> movement delta.
        let item_delta: i32 = if self.input.pressed(KEY_UP) {
            -items_per_line
        } else if self.input.pressed(KEY_DOWN) {
            items_per_line
        } else if self.input.pressed(KEY_LEFT) {
            -1
        } else if self.input.pressed(KEY_RIGHT) {
            1
        } else if self.input.pressed(KEY_PGUP) {
            -(items_per_line * lines_per_page)
        } else if self.input.pressed(KEY_PGDN) {
            items_per_line * lines_per_page
        } else if self.input.pressed(KEY_HOME) {
            -(ctx.current as i32)
        } else if self.input.pressed(KEY_END) {
            num_magic - ctx.current as i32 - 1
        } else if self.input.pressed(KEY_MENU) {
            return 0;
        } else {
            0
        };

        let cur = ctx.current as i32;
        ctx.current = if cur + item_delta < 0 {
            0
        } else if cur + item_delta >= num_magic {
            (num_magic - 1).max(0) as usize
        } else {
            (cur + item_delta) as usize
        };

        // The magic list box.
        self.create_box_with_shadow((10, 42 + box_y_offset), lines_per_page - 1, 16, 1, false, 0);

        // Cash amount box.
        self.create_single_line_box((0, 0), 5, false);
        let cash_label = self.texts.word(CASH_LABEL as usize);
        self.draw_text(&cash_label, (10, 10), 0, false, false);
        self.draw_number(
            self.globals.cash,
            6,
            (49, 14),
            NumColor::Yellow,
            NumAlign::Right,
        );

        // MP of the selected magic.
        self.create_single_line_box((215, 0), 5, false);
        self.draw_ui_sprite(SPRITENUM_SLASH, 260, 14);
        let sel_mp = ctx.items.get(ctx.current).map(|it| it.mp).unwrap_or(0);
        self.draw_number(
            sel_mp as u32,
            4,
            (230, 14),
            NumColor::Yellow,
            NumAlign::Right,
        );
        self.draw_number(
            ctx.player_mp as u32,
            4,
            (265, 14),
            NumColor::Cyan,
            NumAlign::Right,
        );

        // The current page of magic names.
        let mut i = ctx.current as i32 / items_per_line * items_per_line
            - items_per_line * page_line_offset;
        if i < 0 {
            i = 0;
        }

        'outer: for j in 0..lines_per_page {
            for k in 0..items_per_line {
                if i >= num_magic {
                    break 'outer;
                }
                let idx = i as usize;
                let mut color = MENUITEM_COLOR;
                if idx == ctx.current {
                    color = if ctx.items[idx].enabled {
                        self.menuitem_color_selected()
                    } else {
                        MENUITEM_COLOR_SELECTED_INACTIVE
                    };
                } else if !ctx.items[idx].enabled {
                    color = MENUITEM_COLOR_INACTIVE;
                }

                let word = self.texts.word(ctx.items[idx].magic as usize);
                self.draw_text(
                    &word,
                    (35 + k * item_text_width, 54 + j * 18 + box_y_offset),
                    color,
                    true,
                    false,
                );

                if idx == ctx.current {
                    self.draw_ui_sprite(
                        SPRITENUM_CURSOR,
                        35 + cursor_x_offset + k * item_text_width,
                        64 + j * 18 + box_y_offset,
                    );
                }

                i += 1;
            }
        }

        // C's rgMagicItem is a fixed 32-slot array, so rgMagicItem[current] is
        // always an in-bounds (if possibly stale) read even when the role has
        // zero usable magics. Our items Vec can be empty, so guard the index —
        // KEY_MENU (handled above) still lets the player cancel out.
        if self.input.pressed(KEY_SEARCH) && ctx.items.get(ctx.current).is_some_and(|it| it.enabled)
        {
            let col = ctx.current as i32 % items_per_line;
            let line = if (ctx.current as i32) < items_per_line * page_line_offset {
                ctx.current as i32 / items_per_line
            } else {
                page_line_offset
            };
            let jx = 35 + col * item_text_width;
            let ky = 54 + line * 18 + box_y_offset;
            let word = self.texts.word(ctx.items[ctx.current].magic as usize);
            self.draw_text(&word, (jx, ky), MENUITEM_COLOR_CONFIRMED, false, true);
            self.draw_ui_sprite(SPRITENUM_CURSOR, jx + cursor_x_offset, ky + 10);
            return ctx.items[ctx.current].magic;
        }

        0xFFFF
    }

    /// PAL_MagicSelectionMenu: run the menu loop, returning the selected
    /// magic (0 if cancelled).
    pub fn magic_selection_menu(
        &mut self,
        player_role: u16,
        in_battle: bool,
        default_magic: u16,
    ) -> u16 {
        let mut ctx = self.magic_selection_menu_init(player_role, in_battle, default_magic);
        if ctx.items.is_empty() {
            return 0;
        }
        self.input.clear_key_state();
        let mut dw_time = self.ticks();

        loop {
            self.make_scene();

            let mut w = 45;
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role;
                crate::uibattle::player_info_box(
                    self,
                    (w, 165),
                    role as usize,
                    100,
                    TIMEMETER_COLOR_DEFAULT,
                    false,
                );
                w += 78;
            }

            let sel = self.magic_selection_menu_update(&mut ctx);
            self.video_update();
            self.input.clear_key_state();

            if sel != 0xFFFF {
                return sel;
            }

            // Headless: confirm the current (enabled) magic, or cancel.
            if self.ui.auto_confirm {
                let it = ctx.items[ctx.current];
                return if it.enabled { it.magic } else { 0 };
            }

            self.process_event();
            while self.ticks() < dw_time {
                self.process_event();
                if self.input.key_press != 0 || self.quit_requested {
                    break;
                }
                self.delay(5);
            }
            if self.quit_requested {
                return 0;
            }
            dw_time = self.ticks() + crate::game_loop::FRAME_TIME;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        let mut e = Engine::new(true).expect("headless engine");
        e.ui.auto_confirm = true;
        e.init_ui().expect("init ui");
        e.globals.load_default_game().unwrap();
        e
    }

    #[test]
    fn magic_menu_update_draws() {
        let mut e = engine();
        e.globals.game.player_roles.mp[0] = 999;
        e.globals.game.player_roles.max_mp[0] = 999;

        // Find a real magic object to add to role 0.
        let mut added = false;
        for obj_id in 1..crate::global::MAX_OBJECTS as u16 {
            let magic_num = e.globals.game.objects[obj_id as usize].magic_number();
            if magic_num != 0
                && (magic_num as usize) < e.globals.game.magics.len()
                && e.globals.add_magic(0, obj_id)
            {
                added = true;
                break;
            }
        }
        assert!(added, "no magic object found to add");

        let mut ctx = e.magic_selection_menu_init(0, false, 0);
        assert!(!ctx.items.is_empty());
        e.screen.clear(0);
        let r = e.magic_selection_menu_update(&mut ctx);
        assert_eq!(r, 0xFFFF); // not confirmed (no key), but drew
        assert!(e.screen.pixels.iter().any(|&p| p != 0));
    }

    #[test]
    fn magic_selection_update_survives_empty_items() {
        // A role with zero usable magics yields an empty items Vec. C reads its
        // fixed-size array safely; the Rust port must not panic-index the Vec
        // when confirm (KEY_SEARCH) is pressed on the empty submenu.
        let mut e = engine();
        let mut ctx = e.magic_selection_menu_init(0, false, 0);
        ctx.items.clear();
        ctx.current = 0;
        e.screen.clear(0);
        e.input.key_press = crate::input::KEY_SEARCH;
        let r = e.magic_selection_menu_update(&mut ctx);
        assert_eq!(r, 0xFFFF); // no selection, and crucially: no panic.
    }
}
