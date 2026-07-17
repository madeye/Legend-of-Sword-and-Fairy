//! Inventory item selection menu (port of SDLPAL `itemmenu.c`, DOS paths:
//! `fIsWIN95 == FALSE`, `PAL_CLASSIC`, no `desc.dat` so `lpObjectDesc` is
//! NULL and object descriptions are never drawn).
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::{Engine, FRAME_TIME};
use crate::global::{ITEMFLAG_USABLE, MAX_INVENTORY, MAX_PLAYER_EQUIPMENTS};
use crate::ui::{
    MenuItemChanged, NumAlign, NumColor, MENUITEM_COLOR, MENUITEM_COLOR_CONFIRMED,
    MENUITEM_COLOR_EQUIPPEDITEM, MENUITEM_COLOR_INACTIVE, MENUITEM_COLOR_SELECTED_INACTIVE,
    SPRITENUM_CURSOR, SPRITENUM_ITEMBOX,
};

/// Bytes per WORD.DAT record for the Chinese DOS data (gConfig.dwWordLength).
const WORD_LENGTH: i32 = 10;

/// Item-menu context (itemmenu.c file statics g_iNumInventory / g_wItemFlags /
/// g_fNoDesc), owned locally so the menu is re-entrant.
struct ItemMenuCtx {
    num_inventory: i32,
    item_flags: u16,
}

impl Engine {
    /// PAL_ItemSelectMenuInit.
    fn item_select_menu_init(&mut self, item_flags: u16) -> ItemMenuCtx {
        self.globals.compress_inventory();

        // Count items currently in the inventory.
        let mut num = 0i32;
        while (num as usize) < MAX_INVENTORY && self.globals.inventory[num as usize].item != 0 {
            num += 1;
        }

        // Also add usable equipped items to the list.
        if item_flags & ITEMFLAG_USABLE != 0 && !self.globals.in_battle {
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role as usize;
                for j in 0..MAX_PLAYER_EQUIPMENTS {
                    let equip = self.globals.game.player_roles.equipment[j][role];
                    if self.globals.game.objects[equip as usize].item_flags() & ITEMFLAG_USABLE != 0
                        && (num as usize) < MAX_INVENTORY
                    {
                        let inv = &mut self.globals.inventory[num as usize];
                        inv.item = equip;
                        inv.amount = 0;
                        inv.amount_in_use = 0xFFFF;
                        num += 1;
                    }
                }
            }
        }

        ItemMenuCtx {
            num_inventory: num,
            item_flags,
        }
    }

    /// PAL_ItemSelectMenuUpdate: draw one frame, returning the selected
    /// object ID (0 = cancelled, 0xFFFF = not yet confirmed).
    fn item_select_menu_update(&mut self, ctx: &ItemMenuCtx) -> u16 {
        use crate::input::{
            KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_MENU, KEY_PGDN, KEY_PGUP, KEY_RIGHT,
            KEY_SEARCH, KEY_UP,
        };

        let items_per_line = 32 / WORD_LENGTH;
        let item_text_width = 8 * WORD_LENGTH + 20;
        let lines_per_page = 7; // 7 - ExtraItemDescLines(0)
        let cursor_x_offset = WORD_LENGTH * 5 / 2;
        let amount_x_offset = WORD_LENGTH * 8 + 1;
        let page_line_offset = (lines_per_page + 1) / 2;
        let picture_y_offset = 0; // ExtraItemDescLines <= 1
        let mut cursor_pos = (15 + cursor_x_offset, 22);

        // Input -> movement delta.
        let cur = self.globals.cur_inv_menu_item;
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
            -cur
        } else if self.input.pressed(KEY_END) {
            ctx.num_inventory - cur - 1
        } else if self.input.pressed(KEY_MENU) {
            return 0;
        } else {
            0
        };

        self.globals.cur_inv_menu_item = if cur + item_delta < 0 {
            0
        } else if cur + item_delta >= ctx.num_inventory {
            (ctx.num_inventory - 1).max(0)
        } else {
            cur + item_delta
        };
        let cur = self.globals.cur_inv_menu_item;

        // Redraw the box.
        self.create_box_with_shadow((2, 0), lines_per_page - 1, 17, 1, false, 0);

        // Draw the texts of the current page.
        let mut i = cur / items_per_line * items_per_line - items_per_line * page_line_offset;
        if i < 0 {
            i = 0;
        }
        let x_base = 0;
        let y_base = 140;

        'outer: for j in 0..lines_per_page {
            for k in 0..items_per_line {
                let idx = i as usize;
                if idx >= MAX_INVENTORY || self.globals.inventory[idx].item == 0 {
                    break 'outer;
                }
                let object = self.globals.inventory[idx].item;
                let amount = self.globals.inventory[idx].amount as i16;
                let in_use = self.globals.inventory[idx].amount_in_use as i16;
                let selectable =
                    self.globals.game.objects[object as usize].item_flags() & ctx.item_flags != 0
                        && amount > in_use;

                let color = if i == cur {
                    if !selectable {
                        MENUITEM_COLOR_SELECTED_INACTIVE
                    } else if amount == 0 {
                        MENUITEM_COLOR_EQUIPPEDITEM
                    } else {
                        self.menuitem_color_selected()
                    }
                } else if !selectable {
                    MENUITEM_COLOR_INACTIVE
                } else if amount == 0 {
                    MENUITEM_COLOR_EQUIPPEDITEM
                } else {
                    MENUITEM_COLOR
                };

                let word = self.texts.word(object as usize);
                self.draw_text(
                    &word,
                    (15 + k * item_text_width, 12 + j * 18),
                    color,
                    true,
                    false,
                );

                if i == cur {
                    cursor_pos = (15 + cursor_x_offset + k * item_text_width, 22 + j * 18);

                    // Item picture box + item bitmap.
                    self.draw_ui_sprite_shadow(
                        SPRITENUM_ITEMBOX,
                        x_base + 5,
                        y_base + 5 - picture_y_offset,
                    );
                    self.draw_ui_sprite(SPRITENUM_ITEMBOX, x_base, y_base - picture_y_offset);

                    let bmp = self.globals.game.objects[object as usize].item_bitmap();
                    if let Ok(image) = self.globals.files.ball.chunk_decompressed(bmp as usize) {
                        self.screen
                            .blit_rle(&image, x_base + 8, y_base + 7 - picture_y_offset);
                    }
                }

                // Amount of this item.
                if amount as i32 - in_use as i32 > 1 {
                    self.draw_number(
                        (amount - in_use) as u32,
                        2,
                        (15 + amount_x_offset + k * item_text_width, 17 + j * 18),
                        NumColor::Cyan,
                        NumAlign::Right,
                    );
                }

                i += 1;
            }
        }

        // Cursor on the selected item.
        self.draw_ui_sprite(SPRITENUM_CURSOR, cursor_pos.0, cursor_pos.1);

        let object = self.globals.inventory[cur.max(0) as usize].item;

        // (Object descriptions are absent in the DOS data set — nothing to
        // draw here; lpObjectDesc is always NULL.)

        if self.input.pressed(KEY_SEARCH) {
            let amount = self.globals.inventory[cur as usize].amount as i16;
            let in_use = self.globals.inventory[cur as usize].amount_in_use as i16;
            if self.globals.game.objects[object as usize].item_flags() & ctx.item_flags != 0
                && amount > in_use
            {
                if amount > 0 {
                    let line = if cur < items_per_line * page_line_offset {
                        cur / items_per_line
                    } else {
                        page_line_offset
                    };
                    let col = cur % items_per_line;
                    let word = self.texts.word(object as usize);
                    self.draw_text(
                        &word,
                        (15 + col * item_text_width, 12 + line * 18),
                        MENUITEM_COLOR_CONFIRMED,
                        false,
                        false,
                    );
                    self.draw_ui_sprite(SPRITENUM_CURSOR, cursor_pos.0, cursor_pos.1);
                }
                return object;
            }
        }

        0xFFFF
    }

    /// PAL_ItemSelectMenu: run the menu loop, returning the selected object ID
    /// (0 if cancelled). `on_change` is invoked when the highlighted item
    /// changes (used by the sell menu to show the price).
    pub fn item_select_menu(
        &mut self,
        mut on_change: Option<MenuItemChanged<'_>>,
        item_flags: u16,
    ) -> u16 {
        let ctx = self.item_select_menu_init(item_flags);
        let mut prev_index = self.globals.cur_inv_menu_item;

        self.input.clear_key_state();

        if let Some(cb) = on_change.as_mut() {
            let item = self.globals.inventory[self.globals.cur_inv_menu_item.max(0) as usize].item;
            cb(self, item);
        }

        let mut dw_time = self.ticks();

        loop {
            if on_change.is_none() {
                self.make_scene();
            }

            let w = self.item_select_menu_update(&ctx);
            self.video_update();
            self.input.clear_key_state();

            if w != 0xFFFF {
                return w;
            }

            // Headless: don't spin waiting for input.
            if self.ui.auto_confirm {
                return 0;
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
            dw_time = self.ticks() + FRAME_TIME;

            if prev_index != self.globals.cur_inv_menu_item {
                let cur = self.globals.cur_inv_menu_item;
                if cur >= 0 && (cur as usize) < MAX_INVENTORY {
                    if let Some(cb) = on_change.as_mut() {
                        let item = self.globals.inventory[cur as usize].item;
                        cb(self, item);
                    }
                }
                prev_index = self.globals.cur_inv_menu_item;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        std::env::set_var(
            "PAL_DATA_DIR",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../pal"),
        );
        let mut e = Engine::new(true).expect("headless engine");
        e.ui.auto_confirm = true;
        e.init_ui().expect("init ui");
        e.globals.load_default_game().unwrap();
        e
    }

    #[test]
    fn item_menu_update_draws_populated_inventory() {
        let mut e = engine();
        // Populate a few usable items.
        let mut usable = None;
        for obj_id in 1..crate::global::MAX_OBJECTS as u16 {
            if e.globals.game.objects[obj_id as usize].item_flags() & ITEMFLAG_USABLE != 0 {
                usable = Some(obj_id);
                break;
            }
        }
        let obj = usable.expect("a usable item exists");
        e.globals.add_item_to_inventory(obj, 5);

        let ctx = e.item_select_menu_init(ITEMFLAG_USABLE);
        assert!(ctx.num_inventory >= 1);
        e.screen.clear(0);
        let r = e.item_select_menu_update(&ctx);
        assert_eq!(r, 0xFFFF); // not confirmed, but drew
        assert!(e.screen.pixels.iter().any(|&p| p != 0));
    }
}
