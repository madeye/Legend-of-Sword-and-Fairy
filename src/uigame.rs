//! In-game menus, shops, status/equipment screens (port of SDLPAL
//! `uigame.c`, DOS paths: `fIsWIN95 == FALSE`, `PAL_CLASSIC`,
//! `fUseCustomScreenLayout == FALSE`). Fixed screen positions come from the
//! default `SCREENLAYOUT` in palcfg.c.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::global::{
    ITEMFLAG_EQUIPABLE_BY_FIRST, ITEMFLAG_SELLABLE, MAGICFLAG_APPLY_TO_ALL, MAX_INVENTORY,
    MAX_PLAYER_EQUIPMENTS, MAX_POISONS, MAX_STORE_ITEM,
};
use crate::magicmenu::TIMEMETER_COLOR_DEFAULT;
use crate::ui::{
    BoxHandle, MenuItem, NumAlign, NumColor, ITEMUSEMENU_COLOR_STATLABEL, MENUITEM_COLOR,
    MENUITEM_COLOR_CONFIRMED, MENUITEM_COLOR_INACTIVE, MENUITEM_COLOR_SELECTED_FIRST,
    MENUITEM_COLOR_SELECTED_INACTIVE, MENUITEM_COLOR_SELECTED_TOTALNUM, MENUITEM_VALUE_CANCELLED,
    SPRITENUM_CURSOR_UP, SPRITENUM_ITEMBOX, SPRITENUM_SLASH, STATUS_COLOR_EQUIPMENT,
};

// ---- Label word numbers (ui.h) ----
const MAINMENU_BACKGROUND_FBPNUM: usize = 60; // DOS (fIsWIN95 ? 2 : 60)
const RIX_NUM_OPENINGMENU: i32 = 4;
const MAINMENU_LABEL_NEWGAME: u16 = 7;
const MAINMENU_LABEL_LOADGAME: u16 = 8;
const LOADMENU_LABEL_SLOT_FIRST: u16 = 43;
const CONFIRMMENU_LABEL_NO: u16 = 19;
const CONFIRMMENU_LABEL_YES: u16 = 20;
const CASH_LABEL: u16 = 21;
const SWITCHMENU_LABEL_DISABLE: u16 = 17;
const SWITCHMENU_LABEL_ENABLE: u16 = 18;
const GAMEMENU_LABEL_STATUS: u16 = 3;
const GAMEMENU_LABEL_MAGIC: u16 = 4;
const GAMEMENU_LABEL_INVENTORY: u16 = 5;
const GAMEMENU_LABEL_SYSTEM: u16 = 6;
const SYSMENU_LABEL_SAVE: u16 = 11;
const SYSMENU_LABEL_LOAD: u16 = 12;
const SYSMENU_LABEL_MUSIC: u16 = 13;
const SYSMENU_LABEL_SOUND: u16 = 14;
const SYSMENU_LABEL_QUIT: u16 = 15;
const INVMENU_LABEL_USE: u16 = 23;
const INVMENU_LABEL_EQUIP: u16 = 22;
const BUYMENU_LABEL_CURRENT: u16 = 35;
const SELLMENU_LABEL_PRICE: u16 = 25;

const STATUS_BACKGROUND_FBPNUM: usize = 0;
const STATUS_LABEL_EXP: u16 = 2;
const STATUS_LABEL_LEVEL: u16 = 48;
const STATUS_LABEL_HP: u16 = 49;
const STATUS_LABEL_MP: u16 = 50;
const STATUS_LABEL_ATTACKPOWER: u16 = 51;
const STATUS_LABEL_MAGICPOWER: u16 = 52;
const STATUS_LABEL_RESISTANCE: u16 = 53;
const STATUS_LABEL_DEXTERITY: u16 = 54;
const STATUS_LABEL_FLEERATE: u16 = 55;

const EQUIPMENU_BACKGROUND_FBPNUM: usize = 1;

// ---- Default SCREENLAYOUT positions (palcfg.c) ----
const ROLE_NAME: (i32, i32) = (110, 8);
const ROLE_IMAGE: (i32, i32) = (110, 30);
// RoleExpLabel, RoleLevelLabel, RoleHPLabel, RoleMPLabel (labels0 order).
const ROLE_MISC_LABELS: [(i32, i32); 4] = [(6, 6), (6, 32), (6, 54), (6, 76)];
const ROLE_STATUS_LABELS: [(i32, i32); 5] = [(6, 98), (6, 118), (6, 138), (6, 158), (6, 178)];
const ROLE_CURR_EXP: (i32, i32) = (58, 6);
const ROLE_NEXT_EXP: (i32, i32) = (58, 15);
const ROLE_HP_SLASH: (i32, i32) = (65, 58);
const ROLE_MP_SLASH: (i32, i32) = (65, 80);
const ROLE_LEVEL: (i32, i32) = (54, 35);
const ROLE_CUR_HP: (i32, i32) = (42, 56);
const ROLE_MAX_HP: (i32, i32) = (63, 61);
const ROLE_CUR_MP: (i32, i32) = (42, 78);
const ROLE_MAX_MP: (i32, i32) = (63, 83);
const ROLE_STATUS_VALUES: [(i32, i32); 5] = [(42, 102), (42, 122), (42, 142), (42, 162), (42, 182)];
const ROLE_EQUIP_IMAGE_BOXES: [(i32, i32); 6] = [
    (189, -1),
    (247, 39),
    (251, 101),
    (201, 133),
    (141, 141),
    (81, 125),
];
const ROLE_EQUIP_NAMES: [(i32, i32); 6] = [
    (195, 38),
    (253, 78),
    (257, 140),
    (207, 172),
    (147, 180),
    (87, 164),
];
const ROLE_POISON_NAMES: [(i32, i32); 10] = [
    (185, 58),
    (185, 76),
    (185, 94),
    (185, 112),
    (185, 130),
    (185, 148),
    (185, 166),
    (185, 184),
    (185, 184),
    (185, 184),
];

const EQUIP_IMAGE_BOX: (i32, i32) = (8, 8);
const EQUIP_ROLE_LIST_BOX: (i32, i32) = (2, 95);
const EQUIP_ITEM_NAME: (i32, i32) = (5, 70);
const EQUIP_ITEM_AMOUNT: (i32, i32) = (51, 57);
const EQUIP_NAMES: [(i32, i32); 6] = [
    (130, 11),
    (130, 33),
    (130, 55),
    (130, 77),
    (130, 99),
    (130, 121),
];
const EQUIP_STATUS_VALUES: [(i32, i32); 5] =
    [(260, 14), (260, 36), (260, 58), (260, 80), (260, 102)];

/// Add a signed offset to a PAL_XY position (PAL_XY_OFFSET).
#[inline]
fn xy_off(pos: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (pos.0 + dx, pos.1 + dy)
}

impl Engine {
    // =======================================================================
    // Small local helpers.
    // =======================================================================

    /// PAL_PlayAVI (aviplay.c).  FMV playback is out of scope for this engine
    /// port; opening/closing videos is a no-op so the calling flows proceed.
    fn play_avi(&mut self, _filename: &str) {}

    /// Read the "saved times" counter from a save slot file (GetSavedTimes).
    fn get_saved_times(&self, slot: i32) -> u16 {
        let path = self.globals.save_dir.join(format!("{slot}.rpg"));
        match std::fs::read(path) {
            Ok(buf) if buf.len() >= 2 => u16::from_le_bytes([buf[0], buf[1]]),
            _ => 0,
        }
    }

    // =======================================================================
    // Opening menu (uigame.c).
    // =======================================================================

    /// PAL_DrawOpeningMenuBackground.
    pub fn draw_opening_menu_background(&mut self) {
        if let Ok(buf) = self
            .globals
            .files
            .fbp
            .chunk_decompressed(MAINMENU_BACKGROUND_FBPNUM)
        {
            self.screen.blit_fbp(&buf);
            self.video_update();
        }
    }

    /// PAL_OpeningMenu: returns the save slot to load (1-5), or 0 for a new
    /// game.
    pub fn opening_menu(&mut self) -> i32 {
        let w = [
            self.word_width(MAINMENU_LABEL_NEWGAME),
            self.word_width(MAINMENU_LABEL_LOADGAME),
        ];
        let items = [
            MenuItem {
                value: 0,
                num_word: MAINMENU_LABEL_NEWGAME,
                enabled: true,
                pos: (125 - if w[0] > 4 { (w[0] - 4) * 8 } else { 0 }, 95),
            },
            MenuItem {
                value: 1,
                num_word: MAINMENU_LABEL_LOADGAME,
                enabled: true,
                pos: (125 - if w[1] > 4 { (w[1] - 4) * 8 } else { 0 }, 112),
            },
        ];

        self.play_music(RIX_NUM_OPENINGMENU, true, 1.0);
        self.draw_opening_menu_background();
        self.fade_in(0, false, 1);

        let mut default_item = 0u16;
        let selected;
        loop {
            let r = self
                .read_menu_default(&items, default_item, MENUITEM_COLOR, None)
                .unwrap_or(MENUITEM_VALUE_CANCELLED);

            if r == 0 || r == MENUITEM_VALUE_CANCELLED {
                selected = 0;
                break;
            }
            // Load game.
            self.backup_screen();
            let slot = self.save_slot_menu(1);
            self.restore_screen();
            self.video_update();
            if slot != MENUITEM_VALUE_CANCELLED {
                selected = slot as i32;
                break;
            }
            default_item = 0;
            if self.ui.auto_confirm || self.quit_requested {
                selected = 0;
                break;
            }
        }

        self.play_music(0, false, 1.0);
        self.fade_out(1);

        if selected == 0 {
            self.play_avi("3.avi");
        }
        selected
    }

    /// PAL_SaveSlotMenu: returns the chosen slot (1-5) or
    /// MENUITEM_VALUE_CANCELLED.
    pub fn save_slot_menu(&mut self, default_slot: u16) -> u16 {
        let w = self.word_max_width(LOADMENU_LABEL_SLOT_FIRST, 5);
        let dx = if w > 4 { (w - 4) * 16 } else { 0 };

        let mut boxes = [0usize; 5];
        let mut items = [MenuItem {
            value: 0,
            num_word: 0,
            enabled: true,
            pos: (0, 0),
        }; 5];

        for i in 0..5 {
            boxes[i] = self.create_single_line_box(
                (195 - dx, 7 + 38 * i as i32),
                6 + if w > 4 { w - 4 } else { 0 },
                false,
            );
            items[i] = MenuItem {
                value: i as u16 + 1,
                num_word: LOADMENU_LABEL_SLOT_FIRST + i as u16,
                enabled: true,
                pos: (210 - dx, 17 + 38 * i as i32),
            };
        }

        for i in 1..=5 {
            let times = self.get_saved_times(i) as u32;
            self.draw_number(
                times,
                4,
                (270, 38 * i - 17),
                NumColor::Yellow,
                NumAlign::Right,
            );
        }

        let selected = self
            .read_menu_default(&items, default_slot - 1, MENUITEM_COLOR, None)
            .unwrap_or(MENUITEM_VALUE_CANCELLED);

        for b in boxes {
            self.delete_box(b);
        }
        self.video_update();
        selected
    }

    /// PAL_SelectionMenu: a common 1-4 item selection box.
    fn selection_menu(&mut self, n_words: usize, n_default: u16, w_items: &[u16]) -> u16 {
        let width = |i: usize| {
            if i < n_words && w_items[i] != 0 {
                self.word_width(w_items[i])
            } else {
                1
            }
        };
        let w = [width(0), width(1), width(2), width(3)];
        let mut dx = [
            (w[0] - 1) * 16,
            (w[1] - 1) * 16,
            (w[2] - 1) * 16,
            (w[3] - 1) * 16,
        ];
        let pos = [
            (145, 110),
            (220 + dx[0], 110),
            (145, 160),
            (220 + dx[2], 160),
        ];

        for &item in w_items.iter().take(n_words) {
            if item == 0 {
                return MENUITEM_VALUE_CANCELLED;
            }
        }

        let mut items = [MenuItem {
            value: 0,
            num_word: 0,
            enabled: true,
            pos: (0, 0),
        }; 4];
        for i in 0..n_words {
            items[i] = MenuItem {
                value: i as u16,
                num_word: w_items[i],
                enabled: true,
                pos: pos[i],
            };
        }

        // Box x-offsets (the C reshuffles dx before creating boxes).
        dx[1] = dx[0];
        dx[3] = dx[2];
        dx[0] = 0;
        dx[2] = 0;

        let mut boxes = [0usize; 4];
        for i in 0..n_words {
            boxes[i] = self.create_single_line_box(
                (130 + 75 * (i as i32 % 2) + dx[i], 100 + 50 * (i as i32 / 2)),
                w[i] + 1,
                true,
            );
        }

        let ret = self
            .read_menu_default(&items[..n_words], n_default, MENUITEM_COLOR, None)
            .unwrap_or(MENUITEM_VALUE_CANCELLED);

        for &b in boxes.iter().take(n_words) {
            self.delete_box(b);
        }
        self.video_update();
        ret
    }

    /// PAL_TripleMenu.
    pub fn triple_menu(&mut self, third_word: u16) -> u16 {
        let items = [CONFIRMMENU_LABEL_NO, CONFIRMMENU_LABEL_YES, third_word];
        self.selection_menu(3, 0, &items)
    }

    /// PAL_ConfirmMenu.
    pub fn confirm_menu(&mut self) -> bool {
        let items = [CONFIRMMENU_LABEL_NO, CONFIRMMENU_LABEL_YES];
        let r = self.selection_menu(2, 0, &items);
        !(r == MENUITEM_VALUE_CANCELLED || r == 0)
    }

    /// PAL_SwitchMenu.
    pub fn switch_menu(&mut self, enabled: bool) -> bool {
        let items = [SWITCHMENU_LABEL_DISABLE, SWITCHMENU_LABEL_ENABLE];
        let r = self.selection_menu(2, if enabled { 1 } else { 0 }, &items);
        if r == MENUITEM_VALUE_CANCELLED {
            enabled
        } else {
            r != 0
        }
    }

    // =======================================================================
    // Cash display (uigame.c).
    // =======================================================================

    /// PAL_ShowCash: draw the cash box at the top-left, returning its handle.
    pub fn show_cash(&mut self, cash: u32) -> BoxHandle {
        let lp_box = self.create_single_line_box((0, 0), 5, true);
        if lp_box == 0 {
            return 0;
        }
        let label = self.texts.word(CASH_LABEL as usize);
        self.draw_text(&label, (10, 10), 0, false, false);
        self.draw_number(cash, 6, (49, 14), NumColor::Yellow, NumAlign::Right);
        lp_box
    }

    // =======================================================================
    // System / in-game / inventory menus (uigame.c).
    // =======================================================================

    /// PAL_SystemMenu: returns true if the user performed an operation.
    pub fn system_menu(&mut self) -> bool {
        let items = [
            MenuItem {
                value: 1,
                num_word: SYSMENU_LABEL_SAVE,
                enabled: true,
                pos: (53, 72),
            },
            MenuItem {
                value: 2,
                num_word: SYSMENU_LABEL_LOAD,
                enabled: true,
                pos: (53, 90),
            },
            MenuItem {
                value: 3,
                num_word: SYSMENU_LABEL_MUSIC,
                enabled: true,
                pos: (53, 108),
            },
            MenuItem {
                value: 4,
                num_word: SYSMENU_LABEL_SOUND,
                enabled: true,
                pos: (53, 126),
            },
            MenuItem {
                value: 5,
                num_word: SYSMENU_LABEL_QUIT,
                enabled: true,
                pos: (53, 144),
            },
        ];
        let width = self.menu_text_max_width(&items) - 1;
        let menu_box = self.create_box((40, 60), items.len() as i32 - 1, width, 0, true);

        let default = self.globals.cur_system_menu_item as u16;
        let mut cb = |eng: &mut Engine, item: u16| {
            eng.globals.cur_system_menu_item = item as i32 - 1;
        };
        let r = self
            .read_menu_default(&items, default, MENUITEM_COLOR, Some(&mut cb))
            .unwrap_or(MENUITEM_VALUE_CANCELLED);

        if r == MENUITEM_VALUE_CANCELLED {
            self.delete_box(menu_box);
            self.video_update();
            return false;
        }

        match r {
            1 => {
                // Save game.
                let slot = self.save_slot_menu(self.globals.current_save_slot as u16);
                if slot != MENUITEM_VALUE_CANCELLED {
                    self.globals.current_save_slot = slot as u8;
                    let mut times = 0u16;
                    for i in 1..=5 {
                        times = times.max(self.get_saved_times(i));
                    }
                    let _ = self.globals.save_game(slot as i32, times + 1);
                }
            }
            2 => {
                // Load game.
                let slot = self.save_slot_menu(self.globals.current_save_slot as u16);
                if slot != MENUITEM_VALUE_CANCELLED {
                    self.play_music(0, false, 1.0);
                    self.fade_out(1);
                    self.globals.reload_in_next_tick(slot as i32);
                }
            }
            3 => {
                // Music toggle. XXX audio backend not wired: toggle is a stub.
                let _ = self.switch_menu(true);
            }
            4 => {
                // Sound toggle. XXX audio backend not wired: toggle is a stub.
                let _ = self.switch_menu(true);
            }
            5 => {
                self.quit_game();
            }
            _ => {}
        }

        self.delete_box(menu_box);
        true
    }

    /// PAL_InventoryMenu.
    fn inventory_menu(&mut self) {
        let items = [
            MenuItem {
                value: 1,
                num_word: INVMENU_LABEL_EQUIP,
                enabled: true,
                pos: (43, 73),
            },
            MenuItem {
                value: 2,
                num_word: INVMENU_LABEL_USE,
                enabled: true,
                pos: (43, 91),
            },
        ];
        let width = self.menu_text_max_width(&items) - 1;
        self.create_box((30, 60), 1, width, 0, false);
        let r = self
            .read_menu_default(&items, 0, MENUITEM_COLOR, None)
            .unwrap_or(MENUITEM_VALUE_CANCELLED);
        match r {
            1 => self.game_equip_item(),
            2 => self.game_use_item(),
            _ => {}
        }
    }

    /// PAL_InGameMenu: the main in-game menu (status / magic / inventory /
    /// system).
    pub fn in_game_menu(&mut self) {
        self.backup_screen();

        let items = [
            MenuItem {
                value: 1,
                num_word: GAMEMENU_LABEL_STATUS,
                enabled: true,
                pos: (16, 50),
            },
            MenuItem {
                value: 2,
                num_word: GAMEMENU_LABEL_MAGIC,
                enabled: true,
                pos: (16, 68),
            },
            MenuItem {
                value: 3,
                num_word: GAMEMENU_LABEL_INVENTORY,
                enabled: true,
                pos: (16, 86),
            },
            MenuItem {
                value: 4,
                num_word: GAMEMENU_LABEL_SYSTEM,
                enabled: true,
                pos: (16, 104),
            },
        ];

        let cash_box = self.show_cash(self.globals.cash);
        let width = self.menu_text_max_width(&items) - 1;
        let menu_box = self.create_box((3, 37), 3, width, 0, false);

        loop {
            let default = self.globals.cur_main_menu_item as u16;
            let mut cb = |eng: &mut Engine, item: u16| {
                eng.globals.cur_main_menu_item = item as i32 - 1;
            };
            let r = self
                .read_menu_default(&items, default, MENUITEM_COLOR, Some(&mut cb))
                .unwrap_or(MENUITEM_VALUE_CANCELLED);

            if r == MENUITEM_VALUE_CANCELLED {
                break;
            }
            match r {
                1 => {
                    self.player_status();
                    break;
                }
                2 => {
                    self.in_game_magic_menu();
                    break;
                }
                3 => {
                    self.inventory_menu();
                    break;
                }
                4 if self.system_menu() => break,
                _ => {}
            }
            if self.ui.auto_confirm || self.quit_requested {
                break;
            }
        }

        self.delete_box(cash_box);
        self.delete_box(menu_box);
        self.restore_screen();
    }

    /// PAL_InGameMagicMenu.
    pub fn in_game_magic_menu(&mut self) {
        let w: u16;

        if self.globals.max_party_member_index == 0 {
            w = 0;
        } else {
            // Player info boxes.
            let mut y = 45;
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role;
                crate::uibattle::player_info_box(
                    self,
                    (y, 165),
                    role as usize,
                    100,
                    TIMEMETER_COLOR_DEFAULT,
                    true,
                );
                y += 78;
            }

            // Menu items, one per party member.
            let mut menu_items: Vec<MenuItem> = Vec::new();
            y = 75;
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role as usize;
                menu_items.push(MenuItem {
                    value: i as u16,
                    num_word: self.globals.game.player_roles.name[role],
                    enabled: self.globals.game.player_roles.hp[role] > 0,
                    pos: (48, y),
                });
                y += 18;
            }

            let width = self.menu_text_max_width(&menu_items) - 1;
            self.create_box(
                (35, 62),
                self.globals.max_party_member_index as i32,
                width,
                0,
                false,
            );
            w = self
                .read_menu_default(&menu_items, 0, MENUITEM_COLOR, None)
                .unwrap_or(MENUITEM_VALUE_CANCELLED);
            if w == MENUITEM_VALUE_CANCELLED {
                return;
            }
        }

        let mut magic = 0u16;
        loop {
            let role = self.globals.party[w as usize].player_role;
            magic = self.magic_selection_menu(role, false, magic);
            if magic == 0 {
                break;
            }

            self.backup_screen();

            if self.globals.game.objects[magic as usize].magic_flags() & MAGICFLAG_APPLY_TO_ALL != 0
            {
                let s = self.globals.game.objects[magic as usize].magic_script_on_use();
                let ns = self.run_trigger_script(s, 0);
                self.globals.game.objects[magic as usize].set_magic_script_on_use(ns);
                if self.script.script_success {
                    let ss = self.globals.game.objects[magic as usize].magic_script_on_success();
                    let nss = self.run_trigger_script(ss, 0);
                    self.globals.game.objects[magic as usize].set_magic_script_on_success(nss);
                    if self.script.script_success {
                        self.spend_magic_mp(w, magic);
                    }
                }
                if self.globals.need_to_fade_in {
                    self.fade_in(
                        self.globals.num_palette as usize,
                        self.globals.night_palette,
                        1,
                    );
                    self.globals.need_to_fade_in = false;
                }
            } else {
                self.magic_apply_to_player(w, magic);
            }

            // Redraw the player info boxes.
            let mut y = 45;
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role;
                crate::uibattle::player_info_box(
                    self,
                    (y, 165),
                    role as usize,
                    100,
                    TIMEMETER_COLOR_DEFAULT,
                    true,
                );
                y += 78;
            }

            if self.ui.auto_confirm || self.quit_requested {
                break;
            }
        }
    }

    /// Deduct the MP cost of `magic` from party member `w`'s role.
    fn spend_magic_mp(&mut self, w: u16, magic: u16) {
        let role = self.globals.party[w as usize].player_role as usize;
        let magic_num = self.globals.game.objects[magic as usize].magic_number() as usize;
        let cost = self.globals.game.magics[magic_num].cost_mp;
        let mp = &mut self.globals.game.player_roles.mp[role];
        *mp = mp.wrapping_sub(cost);
    }

    /// The "select a player to apply the magic on" sub-loop of
    /// PAL_InGameMagicMenu (non apply-to-all magic).
    fn magic_apply_to_player(&mut self, w: u16, magic: u16) {
        use crate::input::{KEY_DOWN, KEY_LEFT, KEY_MENU, KEY_RIGHT, KEY_SEARCH, KEY_UP};
        let mut player: u16 = 0;

        while player != MENUITEM_VALUE_CANCELLED {
            // Redraw player info boxes.
            let mut y = 45;
            for i in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[i].player_role;
                crate::uibattle::player_info_box(
                    self,
                    (y, 165),
                    role as usize,
                    100,
                    TIMEMETER_COLOR_DEFAULT,
                    true,
                );
                y += 78;
            }

            self.restore_screen();
            self.draw_ui_sprite(SPRITENUM_CURSOR_UP, 75 + 78 * player as i32, 158);
            self.video_update();

            loop {
                if self.ui.auto_confirm {
                    player = MENUITEM_VALUE_CANCELLED;
                    break;
                }
                self.input.clear_key_state();
                self.process_event();
                if self.quit_requested {
                    player = MENUITEM_VALUE_CANCELLED;
                    break;
                }

                if self.input.pressed(KEY_MENU) {
                    player = MENUITEM_VALUE_CANCELLED;
                    break;
                } else if self.input.pressed(KEY_SEARCH) {
                    let target = self.globals.party[player as usize].player_role;
                    let s = self.globals.game.objects[magic as usize].magic_script_on_use();
                    let ns = self.run_trigger_script(s, target);
                    self.globals.game.objects[magic as usize].set_magic_script_on_use(ns);
                    if self.script.script_success {
                        let ss =
                            self.globals.game.objects[magic as usize].magic_script_on_success();
                        let nss = self.run_trigger_script(ss, target);
                        self.globals.game.objects[magic as usize].set_magic_script_on_success(nss);
                        if self.script.script_success {
                            self.spend_magic_mp(w, magic);
                            let role = self.globals.party[w as usize].player_role as usize;
                            let magic_num =
                                self.globals.game.objects[magic as usize].magic_number() as usize;
                            let cost = self.globals.game.magics[magic_num].cost_mp;
                            if self.globals.game.player_roles.mp[role] < cost {
                                player = MENUITEM_VALUE_CANCELLED;
                            }
                        }
                    }
                    break;
                } else if self.input.pressed(KEY_LEFT | KEY_UP) {
                    if player > 0 {
                        player -= 1;
                        break;
                    }
                } else if self.input.pressed(KEY_RIGHT | KEY_DOWN)
                    && player < self.globals.max_party_member_index
                {
                    player += 1;
                    break;
                }
                self.delay(1);
            }
        }
    }

    // =======================================================================
    // Player status screen (uigame.c PAL_PlayerStatus).
    // =======================================================================

    /// PAL_PlayerStatus: the full character status screen.
    pub fn player_status(&mut self) {
        let background = match self
            .globals
            .files
            .fbp
            .chunk_decompressed(STATUS_BACKGROUND_FBPNUM)
        {
            Ok(b) => b,
            Err(_) => return,
        };

        let misc_labels = [
            STATUS_LABEL_EXP,
            STATUS_LABEL_LEVEL,
            STATUS_LABEL_HP,
            STATUS_LABEL_MP,
        ];
        let stat_labels = [
            STATUS_LABEL_ATTACKPOWER,
            STATUS_LABEL_MAGICPOWER,
            STATUS_LABEL_RESISTANCE,
            STATUS_LABEL_DEXTERITY,
            STATUS_LABEL_FLEERATE,
        ];

        let mut current: i32 = 0;
        while current >= 0 && current <= self.globals.max_party_member_index as i32 {
            let role = self.globals.party[current as usize].player_role as usize;

            // Background.
            self.screen.blit_fbp(&background);

            // Avatar image.
            let avatar = self.globals.game.player_roles.avatar[role];
            if let Ok(img) = self.globals.files.rgm.chunk_decompressed(avatar as usize) {
                self.screen.blit_rle(&img, ROLE_IMAGE.0, ROLE_IMAGE.1);
            }

            // Equipment images + names.
            for i in 0..MAX_PLAYER_EQUIPMENTS {
                let w = self.globals.game.player_roles.equipment[i][role];
                if w == 0 {
                    continue;
                }
                let bmp = self.globals.game.objects[w as usize].item_bitmap();
                if let Ok(img) = self.globals.files.ball.chunk_decompressed(bmp as usize) {
                    let p = xy_off(ROLE_EQUIP_IMAGE_BOXES[i], 1, 1);
                    self.screen.blit_rle(&img, p.0, p.1);
                }
                let mut offset = self.word_width(w) * 16;
                offset = if ROLE_EQUIP_NAMES[i].0 + offset > 320 {
                    320 - ROLE_EQUIP_NAMES[i].0 - offset
                } else {
                    0
                };
                let word = self.texts.word(w as usize);
                self.draw_text(
                    &word,
                    xy_off(ROLE_EQUIP_NAMES[i], offset, 0),
                    STATUS_COLOR_EQUIPMENT,
                    true,
                    false,
                );
            }

            // Text labels.
            for (i, &lbl) in misc_labels.iter().enumerate() {
                let word = self.texts.word(lbl as usize);
                self.draw_text(&word, ROLE_MISC_LABELS[i], MENUITEM_COLOR, true, false);
            }
            for (i, &lbl) in stat_labels.iter().enumerate() {
                let word = self.texts.word(lbl as usize);
                self.draw_text(&word, ROLE_STATUS_LABELS[i], MENUITEM_COLOR, true, false);
            }

            let name = self
                .texts
                .word(self.globals.game.player_roles.name[role] as usize);
            self.draw_text(&name, ROLE_NAME, MENUITEM_COLOR_CONFIRMED, true, false);

            // HP/MP slashes.
            self.draw_ui_sprite(SPRITENUM_SLASH, ROLE_HP_SLASH.0, ROLE_HP_SLASH.1);
            self.draw_ui_sprite(SPRITENUM_SLASH, ROLE_MP_SLASH.0, ROLE_MP_SLASH.1);

            // Stats numbers.
            let exp = self.globals.exp.primary_exp[role].exp as u32;
            let level = self.globals.game.player_roles.level[role];
            let next_exp = self.globals.game.level_up_exp[level as usize] as u32;
            let hp = self.globals.game.player_roles.hp[role] as u32;
            let max_hp = self.globals.game.player_roles.max_hp[role] as u32;
            let mp = self.globals.game.player_roles.mp[role] as u32;
            let max_mp = self.globals.game.player_roles.max_mp[role] as u32;
            self.draw_number(exp, 5, ROLE_CURR_EXP, NumColor::Yellow, NumAlign::Right);
            self.draw_number(next_exp, 5, ROLE_NEXT_EXP, NumColor::Cyan, NumAlign::Right);
            self.draw_number(
                level as u32,
                2,
                ROLE_LEVEL,
                NumColor::Yellow,
                NumAlign::Right,
            );
            self.draw_number(hp, 4, ROLE_CUR_HP, NumColor::Yellow, NumAlign::Right);
            self.draw_number(max_hp, 4, ROLE_MAX_HP, NumColor::Blue, NumAlign::Right);
            self.draw_number(mp, 4, ROLE_CUR_MP, NumColor::Yellow, NumAlign::Right);
            self.draw_number(max_mp, 4, ROLE_MAX_MP, NumColor::Blue, NumAlign::Right);

            let attack = self.globals.player_attack_strength(role) as u32;
            let magic_str = self.globals.player_magic_strength(role) as u32;
            let defense = self.globals.player_defense(role) as u32;
            let dexterity = self.globals.player_dexterity(role) as u32;
            let flee = self.globals.player_flee_rate(role) as u32;
            self.draw_number(
                attack,
                4,
                ROLE_STATUS_VALUES[0],
                NumColor::Yellow,
                NumAlign::Right,
            );
            self.draw_number(
                magic_str,
                4,
                ROLE_STATUS_VALUES[1],
                NumColor::Yellow,
                NumAlign::Right,
            );
            self.draw_number(
                defense,
                4,
                ROLE_STATUS_VALUES[2],
                NumColor::Yellow,
                NumAlign::Right,
            );
            self.draw_number(
                dexterity,
                4,
                ROLE_STATUS_VALUES[3],
                NumColor::Yellow,
                NumAlign::Right,
            );
            self.draw_number(
                flee,
                4,
                ROLE_STATUS_VALUES[4],
                NumColor::Yellow,
                NumAlign::Right,
            );

            // Poisons.
            let mut j = 0;
            for i in 0..MAX_POISONS {
                let w = self.globals.poison_status[i][current as usize].poison_id;
                if w != 0 && self.globals.game.objects[w as usize].poison_level() <= 3 && j < 10 {
                    let color = (self.globals.game.objects[w as usize].poison_color() + 10) as u8;
                    let word = self.texts.word(w as usize);
                    self.draw_text(&word, ROLE_POISON_NAMES[j], color, true, false);
                    j += 1;
                }
            }

            self.video_update();
            self.input.clear_key_state();

            // Wait for input.
            use crate::input::{KEY_DOWN, KEY_LEFT, KEY_MENU, KEY_RIGHT, KEY_SEARCH, KEY_UP};
            loop {
                if self.ui.auto_confirm {
                    current = -1;
                    break;
                }
                self.delay(1);
                if self.quit_requested {
                    current = -1;
                    break;
                }
                if self.input.pressed(KEY_MENU) {
                    current = -1;
                    break;
                } else if self.input.pressed(KEY_LEFT | KEY_UP) {
                    current -= 1;
                    break;
                } else if self.input.pressed(KEY_RIGHT | KEY_DOWN | KEY_SEARCH) {
                    current += 1;
                    break;
                }
            }
        }
    }

    // =======================================================================
    // Item use / equip menus (uigame.c + the play.c wrappers).
    // =======================================================================

    /// PAL_ItemUseMenu: choose a player to use `item_to_use` on. Returns the
    /// selected player role, or MENUITEM_VALUE_CANCELLED.
    pub fn item_use_menu(&mut self, item_to_use: u16) -> u16 {
        use crate::input::{KEY_DOWN, KEY_LEFT, KEY_MENU, KEY_RIGHT, KEY_SEARCH, KEY_UP};

        let mut selected_player: i32 = 0;
        let mut selected_color = MENUITEM_COLOR_SELECTED_FIRST;
        let mut color_change_time = 0u64;

        loop {
            if selected_player > self.globals.max_party_member_index as i32 {
                selected_player = 0;
            }

            self.create_box((110, 2), 7, 9, 0, false);

            // Stat labels.
            let labels = [
                (STATUS_LABEL_LEVEL, 16),
                (STATUS_LABEL_HP, 34),
                (STATUS_LABEL_MP, 52),
                (STATUS_LABEL_ATTACKPOWER, 70),
                (STATUS_LABEL_MAGICPOWER, 88),
                (STATUS_LABEL_RESISTANCE, 106),
                (STATUS_LABEL_DEXTERITY, 124),
                (STATUS_LABEL_FLEERATE, 142),
            ];
            for (word_num, y) in labels {
                let word = self.texts.word(word_num as usize);
                self.draw_text(&word, (200, y), ITEMUSEMENU_COLOR_STATLABEL, true, false);
            }

            let role = self.globals.party[selected_player as usize].player_role as usize;
            let level = self.globals.game.player_roles.level[role] as u32;
            let hp = self.globals.game.player_roles.hp[role] as u32;
            let max_hp = self.globals.game.player_roles.max_hp[role] as u32;
            let mp = self.globals.game.player_roles.mp[role] as u32;
            let max_mp = self.globals.game.player_roles.max_mp[role] as u32;

            self.draw_number(level, 4, (240, 20), NumColor::Yellow, NumAlign::Right);
            self.draw_ui_sprite(SPRITENUM_SLASH, 263, 38);
            self.draw_number(max_hp, 4, (261, 40), NumColor::Blue, NumAlign::Right);
            self.draw_number(hp, 4, (240, 37), NumColor::Yellow, NumAlign::Right);
            self.draw_ui_sprite(SPRITENUM_SLASH, 263, 56);
            self.draw_number(max_mp, 4, (261, 58), NumColor::Blue, NumAlign::Right);
            self.draw_number(mp, 4, (240, 55), NumColor::Yellow, NumAlign::Right);

            let attack = self.globals.player_attack_strength(role) as u32;
            let magic = self.globals.player_magic_strength(role) as u32;
            let defense = self.globals.player_defense(role) as u32;
            let dexterity = self.globals.player_dexterity(role) as u32;
            let flee = self.globals.player_flee_rate(role) as u32;
            self.draw_number(attack, 4, (240, 74), NumColor::Yellow, NumAlign::Right);
            self.draw_number(magic, 4, (240, 92), NumColor::Yellow, NumAlign::Right);
            self.draw_number(defense, 4, (240, 110), NumColor::Yellow, NumAlign::Right);
            self.draw_number(dexterity, 4, (240, 128), NumColor::Yellow, NumAlign::Right);
            self.draw_number(flee, 4, (240, 146), NumColor::Yellow, NumAlign::Right);

            // Party member names.
            for i in 0..=self.globals.max_party_member_index as usize {
                let color = if i as i32 == selected_player {
                    selected_color
                } else {
                    MENUITEM_COLOR
                };
                let role = self.globals.party[i].player_role as usize;
                let name = self
                    .texts
                    .word(self.globals.game.player_roles.name[role] as usize);
                self.draw_text(&name, (125, 16 + 20 * i as i32), color, true, false);
            }

            self.draw_ui_sprite(SPRITENUM_ITEMBOX, 120, 80);

            let amount = self.globals.get_item_amount(item_to_use);
            if amount > 0 {
                let bmp = self.globals.game.objects[item_to_use as usize].item_bitmap();
                if let Ok(img) = self.globals.files.ball.chunk_decompressed(bmp as usize) {
                    self.screen.blit_rle(&img, 127, 88);
                }
                let word = self.texts.word(item_to_use as usize);
                self.draw_text(&word, (116, 143), STATUS_COLOR_EQUIPMENT, true, false);
                self.draw_number(
                    amount as u32,
                    2,
                    (170, 133),
                    NumColor::Cyan,
                    NumAlign::Right,
                );
            }

            self.video_update();
            self.input.clear_key_state();

            // Wait for input (with the highlight-color animation).
            loop {
                if self.ui.auto_confirm {
                    return MENUITEM_VALUE_CANCELLED;
                }
                if self.ticks() >= color_change_time {
                    selected_color = if selected_color as u32 + 1
                        >= MENUITEM_COLOR_SELECTED_FIRST as u32 + MENUITEM_COLOR_SELECTED_TOTALNUM
                    {
                        MENUITEM_COLOR_SELECTED_FIRST
                    } else {
                        selected_color + 1
                    };
                    color_change_time =
                        self.ticks() + (600 / MENUITEM_COLOR_SELECTED_TOTALNUM as u64);
                    let role = self.globals.party[selected_player as usize].player_role as usize;
                    let name = self
                        .texts
                        .word(self.globals.game.player_roles.name[role] as usize);
                    self.draw_text(
                        &name,
                        (125, 16 + 20 * selected_player),
                        selected_color,
                        false,
                        true,
                    );
                }
                self.process_event();
                if self.input.key_press != 0 || self.quit_requested {
                    break;
                }
                self.delay(1);
            }

            if amount <= 0 {
                return MENUITEM_VALUE_CANCELLED;
            }

            if self.input.pressed(KEY_UP | KEY_LEFT) {
                selected_player -= 1;
                if selected_player < 0 {
                    selected_player = self.globals.max_party_member_index as i32;
                }
            } else if self.input.pressed(KEY_DOWN | KEY_RIGHT) {
                selected_player += 1;
                if selected_player > self.globals.max_party_member_index as i32 {
                    selected_player = 0;
                }
            } else if self.input.pressed(KEY_MENU) {
                break;
            } else if self.input.pressed(KEY_SEARCH) {
                return self.globals.party[selected_player as usize].player_role;
            }
        }

        MENUITEM_VALUE_CANCELLED
    }

    // =======================================================================
    // Shops (uigame.c PAL_BuyMenu / PAL_SellMenu).
    // =======================================================================

    /// PAL_BuyMenu_OnItemChange.
    fn buy_menu_on_item_change(&mut self, current_item: u16, firsttime: &mut bool) {
        // Item box.
        if *firsttime {
            self.draw_ui_sprite_shadow(SPRITENUM_ITEMBOX, 46, 14);
        }
        self.draw_ui_sprite(SPRITENUM_ITEMBOX, 40, 8);

        // Item picture.
        let bmp = self.globals.game.objects[current_item as usize].item_bitmap();
        if let Ok(img) = self.globals.files.ball.chunk_decompressed(bmp as usize) {
            self.screen.blit_rle(&img, 48, 15);
        }

        // Owned count (inventory + equipped).
        let mut n = 0i32;
        for i in 0..MAX_INVENTORY {
            if self.globals.inventory[i].item == 0 {
                break;
            }
            if self.globals.inventory[i].item == current_item {
                n = self.globals.inventory[i].amount as i32;
                break;
            }
        }
        for i in 0..MAX_PLAYER_EQUIPMENTS {
            for j in 0..=self.globals.max_party_member_index as usize {
                let role = self.globals.party[j].player_role as usize;
                if self.globals.game.player_roles.equipment[i][role] == current_item {
                    n += 1;
                }
            }
        }

        // Inventory count box.
        if *firsttime {
            self.create_single_line_box_with_shadow((20, 100), 5, false, 6);
        } else {
            self.create_single_line_box_with_shadow((20, 100), 5, false, 0);
        }
        let cur_label = self.texts.word(BUYMENU_LABEL_CURRENT as usize);
        self.draw_text(&cur_label, (30, 110), 0, false, false);
        self.draw_number(n as u32, 6, (69, 115), NumColor::Yellow, NumAlign::Right);

        // Cash box.
        if *firsttime {
            self.create_single_line_box_with_shadow((20, 141), 5, false, 6);
        } else {
            self.create_single_line_box_with_shadow((20, 141), 5, false, 0);
        }
        let cash_label = self.texts.word(CASH_LABEL as usize);
        self.draw_text(&cash_label, (30, 151), 0, false, false);
        self.draw_number(
            self.globals.cash,
            6,
            (69, 156),
            NumColor::Yellow,
            NumAlign::Right,
        );

        self.video_update();
        *firsttime = false;
    }

    /// PAL_BuyMenu.
    pub fn buy_menu(&mut self, store_num: u16) {
        let mut items: Vec<MenuItem> = Vec::new();
        let mut y = 21;
        for i in 0..MAX_STORE_ITEM {
            let obj = self.globals.game.stores[store_num as usize].items[i];
            if obj == 0 {
                break;
            }
            items.push(MenuItem {
                value: obj,
                num_word: obj,
                enabled: true,
                pos: (150, y),
            });
            y += 18;
        }
        let count = items.len();

        self.create_box((122, 8), 8, 8, 1, false);

        for (idx, it) in items.iter().enumerate() {
            let price = self.globals.game.objects[it.value as usize].item_price() as u32;
            self.draw_number(
                price,
                6,
                (238, 26 + idx as i32 * 18),
                NumColor::Yellow,
                NumAlign::Right,
            );
        }

        let mut w = 0u16;
        let mut firsttime = true;

        loop {
            let sel = {
                let mut cb = |eng: &mut Engine, item: u16| {
                    eng.buy_menu_on_item_change(item, &mut firsttime);
                };
                self.read_menu_default(&items[..count], w, MENUITEM_COLOR, Some(&mut cb))
                    .unwrap_or(MENUITEM_VALUE_CANCELLED)
            };

            if sel == MENUITEM_VALUE_CANCELLED {
                break;
            }

            let price = self.globals.game.objects[sel as usize].item_price() as u32;
            if price <= self.globals.cash && self.confirm_menu() {
                self.globals.cash -= price;
                self.globals.add_item_to_inventory(sel, 1);
            }

            // Place the cursor on the current item next loop.
            w = items.iter().position(|it| it.value == sel).unwrap_or(0) as u16;

            if self.ui.auto_confirm || self.quit_requested {
                break;
            }
        }
    }

    /// PAL_SellMenu_OnItemChange.
    fn sell_menu_on_item_change(&mut self, current_item: u16) {
        let (x, y) = (100, 150);

        // Cash box.
        self.create_single_line_box_with_shadow((x, y), 5, false, 0);
        let cash_label = self.texts.word(CASH_LABEL as usize);
        self.draw_text(&cash_label, (x + 10, y + 10), 0, false, false);
        self.draw_number(
            self.globals.cash,
            6,
            (x + 48, y + 15),
            NumColor::Yellow,
            NumAlign::Right,
        );

        // Price box.
        let x = x + 124;
        self.create_single_line_box_with_shadow((x, y), 5, false, 0);
        if self.globals.game.objects[current_item as usize].item_flags() & ITEMFLAG_SELLABLE != 0 {
            let price_label = self.texts.word(SELLMENU_LABEL_PRICE as usize);
            self.draw_text(&price_label, (x + 10, y + 10), 0, false, false);
            let price = self.globals.game.objects[current_item as usize].item_price() as u32 / 2;
            self.draw_number(
                price,
                6,
                (x + 48, y + 15),
                NumColor::Yellow,
                NumAlign::Right,
            );
        }
    }

    /// PAL_SellMenu.
    pub fn sell_menu(&mut self) {
        loop {
            let w = {
                let mut cb = |eng: &mut Engine, item: u16| eng.sell_menu_on_item_change(item);
                self.item_select_menu(Some(&mut cb), ITEMFLAG_SELLABLE)
            };
            if w == 0 {
                break;
            }
            if self.confirm_menu() && self.globals.add_item_to_inventory(w, -1) != 0 {
                let price = self.globals.game.objects[w as usize].item_price() as u32 / 2;
                self.globals.cash += price;
            }
            if self.ui.auto_confirm || self.quit_requested {
                break;
            }
        }
    }

    // =======================================================================
    // Equipment menu (uigame.c PAL_EquipItemMenu).
    // =======================================================================

    /// PAL_EquipItemMenu.
    pub fn equip_item_menu(&mut self, item: u16) {
        use crate::input::{KEY_DOWN, KEY_LEFT, KEY_MENU, KEY_RIGHT, KEY_SEARCH, KEY_UP};

        self.globals.last_unequipped_item = item;

        let background = match self
            .globals
            .files
            .fbp
            .chunk_decompressed(EQUIPMENU_BACKGROUND_FBPNUM)
        {
            Ok(b) => b,
            Err(_) => return,
        };

        let mut current_player: i32 = 0;
        let mut selected_color = MENUITEM_COLOR_SELECTED_FIRST;
        let mut color_change_time = self.ticks() + (600 / MENUITEM_COLOR_SELECTED_TOTALNUM as u64);

        loop {
            let item = self.globals.last_unequipped_item;

            self.screen.blit_fbp(&background);

            // Item picture.
            let bmp = self.globals.game.objects[item as usize].item_bitmap();
            if let Ok(img) = self.globals.files.ball.chunk_decompressed(bmp as usize) {
                let p = xy_off(EQUIP_IMAGE_BOX, 8, 8);
                self.screen.blit_rle(&img, p.0, p.1);
            }

            // Current equipment of the selected player.
            let role = self.globals.party[current_player as usize].player_role as usize;
            #[allow(clippy::needless_range_loop)]
            for i in 0..MAX_PLAYER_EQUIPMENTS {
                let eq = self.globals.game.player_roles.equipment[i][role];
                if eq != 0 {
                    let word = self.texts.word(eq as usize);
                    self.draw_text(&word, EQUIP_NAMES[i], MENUITEM_COLOR, true, false);
                }
            }

            // Stats of the selected player.
            let attack = self.globals.player_attack_strength(role) as u32;
            let magic = self.globals.player_magic_strength(role) as u32;
            let defense = self.globals.player_defense(role) as u32;
            let dexterity = self.globals.player_dexterity(role) as u32;
            let flee = self.globals.player_flee_rate(role) as u32;
            self.draw_number(
                attack,
                4,
                EQUIP_STATUS_VALUES[0],
                NumColor::Cyan,
                NumAlign::Right,
            );
            self.draw_number(
                magic,
                4,
                EQUIP_STATUS_VALUES[1],
                NumColor::Cyan,
                NumAlign::Right,
            );
            self.draw_number(
                defense,
                4,
                EQUIP_STATUS_VALUES[2],
                NumColor::Cyan,
                NumAlign::Right,
            );
            self.draw_number(
                dexterity,
                4,
                EQUIP_STATUS_VALUES[3],
                NumColor::Cyan,
                NumAlign::Right,
            );
            self.draw_number(
                flee,
                4,
                EQUIP_STATUS_VALUES[4],
                NumColor::Cyan,
                NumAlign::Right,
            );

            // Player selection box.
            let width = self.word_max_width(36, 4) - 1;
            self.create_box(
                EQUIP_ROLE_LIST_BOX,
                self.globals.max_party_member_index as i32,
                width,
                0,
                false,
            );

            let equip_flags = self.globals.game.objects[item as usize].item_flags();
            for i in 0..=self.globals.max_party_member_index as usize {
                let prole = self.globals.party[i].player_role;
                let equipable = equip_flags & (ITEMFLAG_EQUIPABLE_BY_FIRST << prole) != 0;
                let color = if current_player == i as i32 {
                    if equipable {
                        selected_color
                    } else {
                        MENUITEM_COLOR_SELECTED_INACTIVE
                    }
                } else if equipable {
                    MENUITEM_COLOR
                } else {
                    MENUITEM_COLOR_INACTIVE
                };
                let name = self
                    .texts
                    .word(self.globals.game.player_roles.name[prole as usize] as usize);
                self.draw_text(
                    &name,
                    xy_off(EQUIP_ROLE_LIST_BOX, 13, 13 + 18 * i as i32),
                    color,
                    true,
                    false,
                );
            }

            // Item label and amount.
            if item != 0 {
                let word = self.texts.word(item as usize);
                self.draw_text(
                    &word,
                    EQUIP_ITEM_NAME,
                    MENUITEM_COLOR_CONFIRMED,
                    true,
                    false,
                );
                let amount = self.globals.get_item_amount(item) as u32;
                self.draw_number(
                    amount,
                    2,
                    EQUIP_ITEM_AMOUNT,
                    NumColor::Cyan,
                    NumAlign::Right,
                );
            }

            self.video_update();
            self.input.clear_key_state();

            // Wait for input.
            loop {
                if self.ui.auto_confirm {
                    return;
                }
                self.process_event();
                if self.quit_requested {
                    return;
                }
                if self.ticks() >= color_change_time {
                    selected_color = if selected_color as u32 + 1
                        >= MENUITEM_COLOR_SELECTED_FIRST as u32 + MENUITEM_COLOR_SELECTED_TOTALNUM
                    {
                        MENUITEM_COLOR_SELECTED_FIRST
                    } else {
                        selected_color + 1
                    };
                    color_change_time =
                        self.ticks() + (600 / MENUITEM_COLOR_SELECTED_TOTALNUM as u64);
                    let prole = self.globals.party[current_player as usize].player_role;
                    if equip_flags & (ITEMFLAG_EQUIPABLE_BY_FIRST << prole) != 0 {
                        let name = self
                            .texts
                            .word(self.globals.game.player_roles.name[prole as usize] as usize);
                        self.draw_text(
                            &name,
                            xy_off(EQUIP_ROLE_LIST_BOX, 13, 13 + 18 * current_player),
                            selected_color,
                            true,
                            true,
                        );
                    }
                }
                if self.input.key_press != 0 {
                    break;
                }
                self.delay(1);
            }

            if item == 0 {
                return;
            }

            if self.input.pressed(KEY_UP | KEY_LEFT) {
                current_player -= 1;
                if current_player < 0 {
                    current_player = self.globals.max_party_member_index as i32;
                }
            } else if self.input.pressed(KEY_DOWN | KEY_RIGHT) {
                current_player += 1;
                if current_player > self.globals.max_party_member_index as i32 {
                    current_player = 0;
                }
            } else if self.input.pressed(KEY_MENU) {
                return;
            } else if self.input.pressed(KEY_SEARCH) {
                let prole = self.globals.party[current_player as usize].player_role;
                if equip_flags & (ITEMFLAG_EQUIPABLE_BY_FIRST << prole) != 0 {
                    let s = self.globals.game.objects[item as usize].item_script_on_equip();
                    let ns = self.run_trigger_script(s, prole);
                    self.globals.game.objects[item as usize].set_item_script_on_equip(ns);
                }
            }
        }
    }

    // =======================================================================
    // Quit (uigame.c PAL_QuitGame).
    // =======================================================================

    /// PAL_QuitGame.
    pub fn quit_game(&mut self) {
        // DOS build has no config page -> a plain Yes/No confirmation.
        if self.confirm_menu() {
            self.play_music(0, false, 2.0);
            self.fade_out(2);
            self.quit_requested = true;
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
        e.globals.max_party_member_index = 0;
        e.globals.party[0].player_role = 0;
        e
    }

    fn nonzero(e: &Engine) -> usize {
        e.screen.pixels.iter().filter(|&&p| p != 0).count()
    }

    #[test]
    fn opening_background_draws() {
        let mut e = engine();
        e.screen.clear(0);
        e.draw_opening_menu_background();
        assert!(nonzero(&e) > 1000);
    }

    #[test]
    fn player_status_draws_role_zero() {
        let mut e = engine();
        e.screen.clear(0);
        e.player_status();
        assert!(nonzero(&e) > 1000);
    }

    #[test]
    fn item_use_menu_draws_and_cancels() {
        let mut e = engine();
        e.screen.clear(0);
        let r = e.item_use_menu(1);
        assert_eq!(r, MENUITEM_VALUE_CANCELLED);
        assert!(nonzero(&e) > 0);
    }

    #[test]
    fn show_cash_box() {
        let mut e = engine();
        e.screen.clear(0);
        let h = e.show_cash(999);
        assert_ne!(h, 0);
        assert!(nonzero(&e) > 0);
    }
}
