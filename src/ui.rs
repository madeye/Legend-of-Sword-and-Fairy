//! UI framework and dialog system (port of SDLPAL `ui.c` and the dialog part
//! of `text.c`). This is the real port; it keeps the public names/signatures
//! the bring-up stub exposed (other modules compile against them) and adds
//! richer sibling methods where the C needs more parameters.
//!
//! Text stays in the game's native Big5 byte encoding end-to-end, exactly as
//! `font.rs` / `text.rs` do. The dialog control codes the DOS engine
//! interprets are single ASCII bytes (`-`, `'`, `@`, `"`, `$`, `~`, `)`, `(`,
//! `\`); a Big5 double-byte character (lead byte >= 0x80) is always consumed
//! atomically before any control-code test, so a trail byte that happens to
//! equal a control byte is never misread.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;
use crate::surface::{self, SCREEN_W};

// ===========================================================================
// Constants (ui.h + text.c).
// ===========================================================================

/// DATA.MKF chunk holding the UI sprite (ui.h CHUNKNUM_SPRITEUI).
const CHUNKNUM_SPRITEUI: usize = 9;
/// DATA.MKF chunk holding the dialog waiting icons (text.c PAL_InitText).
const CHUNKNUM_DIALOGICONS: usize = 12;

pub const MENUITEM_COLOR: u8 = 0x4F;
pub const MENUITEM_COLOR_INACTIVE: u8 = 0x18;
pub const MENUITEM_COLOR_CONFIRMED: u8 = 0x2C;
pub const MENUITEM_COLOR_SELECTED_INACTIVE: u8 = 0x1C;
pub const MENUITEM_COLOR_SELECTED_FIRST: u8 = 0xF9;
pub const MENUITEM_COLOR_SELECTED_TOTALNUM: u32 = 6;
pub const MENUITEM_COLOR_EQUIPPEDITEM: u8 = 0xC8;
pub const DESCTEXT_COLOR: u8 = 0x3C;
pub const STATUS_COLOR_EQUIPMENT: u8 = 0xBE;
pub const ITEMUSEMENU_COLOR_STATLABEL: u8 = 0xBB;

/// MENUITEM_VALUE_CANCELLED (ui.h).
pub const MENUITEM_VALUE_CANCELLED: u16 = 0xFFFF;

// Font colors used by the dialog text interpreter (text.c).
const FONT_COLOR_DEFAULT: u8 = 0x4F;
const FONT_COLOR_YELLOW: u8 = 0x2D;
const FONT_COLOR_RED: u8 = 0x1A;
const FONT_COLOR_CYAN: u8 = 0x8D;
const FONT_COLOR_CYAN_ALT: u8 = 0x8C;
const FONT_COLOR_RED_ALT: u8 = 0x17;

// Dialog positions (text.h DIALOGLOCATION).
pub const DIALOG_UPPER: u8 = 0;
pub const DIALOG_CENTER: u8 = 1;
pub const DIALOG_LOWER: u8 = 2;
pub const DIALOG_CENTER_WINDOW: u8 = 3;

// Sprite numbers inside gpSpriteUI (ui.h).
pub const SPRITENUM_SLASH: usize = 39;
pub const SPRITENUM_ITEMBOX: usize = 70;
pub const SPRITENUM_CURSOR_YELLOW_UP: usize = 66;
pub const SPRITENUM_CURSOR_UP: usize = 67;
pub const SPRITENUM_CURSOR_YELLOW: usize = 68;
pub const SPRITENUM_CURSOR: usize = 69;

/// Colors for PAL_DrawNumber.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumColor {
    Yellow,
    Blue,
    Cyan,
}

/// Alignment for PAL_DrawNumber.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumAlign {
    Left,
    Mid,
    Right,
}

/// MENUITEM.
#[derive(Clone, Copy, Debug)]
pub struct MenuItem {
    pub value: u16,
    /// Word number of the label (WORD.DAT).
    pub num_word: u16,
    pub enabled: bool,
    pub pos: (i32, i32),
}

/// Handle for a created box (owns the saved background for PAL_DeleteBox).
/// 0 is the null handle (matching the C code that returns NULL when the box
/// did not save the screen).
pub type BoxHandle = usize;

/// Callback invoked when the highlighted menu item changes
/// (lpfnMenuItemChanged).
pub type MenuItemChanged<'a> = &'a mut dyn FnMut(&mut Engine, u16);

/// Saved screen background of a box (BOX::lpSavedArea).
struct BoxRecord {
    pos: (i32, i32),
    w: i32,
    h: i32,
    saved: Vec<u8>,
}

/// Module-private UI/dialog state (text.c `g_TextLib` and ui.c statics).
pub struct UiState {
    /// The UI sprite loaded from DATA.MKF #9 (ui.c gpSpriteUI).
    pub sprite_ui: Vec<u8>,
    /// Dialog waiting icons sprite (text.c bufDialogIcons, DATA.MKF #12).
    pub dialog_icons: Vec<u8>,
    /// Live saved-box backgrounds, indexed by `handle - 1`.
    boxes: Vec<Option<BoxRecord>>,

    // ---- g_TextLib dialog state ----
    /// nCurrentDialogLine (can be -1 after a `~` delay code).
    pub current_dialog_line: i32,
    /// bCurrentFontColor.
    pub current_font_color: u8,
    /// posIcon.
    pub pos_icon: (i32, i32),
    /// posDialogTitle.
    pub pos_dialog_title: (i32, i32),
    /// posDialogText.
    pub pos_dialog_text: (i32, i32),
    /// bDialogPosition.
    pub dialog_position: u8,
    /// bIcon (waiting-icon frame number).
    pub icon: u8,
    /// iDelayTime.
    pub delay_time: i32,
    /// iDialogShadow.
    pub dialog_shadow: i32,
    /// fUserSkip.
    pub user_skip: bool,
    /// fPlayingRNG.
    pub playing_rng: bool,
    /// RGM chunk of the portrait shown by the current dialog (0 = none).
    /// Gates dialog voice-over playback; not part of the original engine.
    pub dialog_face: i32,
    /// g_fUpdatedInBattle.
    pub updated_in_battle: bool,

    /// Kept for source compatibility with the bring-up stub; the authoritative
    /// state is `current_dialog_line`.
    pub in_dialog: bool,

    /// Headless test escape hatch: when set, interactive wait loops return
    /// their default immediately and dialog delays are skipped, so drawing
    /// code can be exercised without a window or real input. Set only by
    /// tests; always false in the running game.
    pub auto_confirm: bool,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState {
            sprite_ui: Vec::new(),
            dialog_icons: Vec::new(),
            boxes: Vec::new(),
            current_dialog_line: 0,
            current_font_color: FONT_COLOR_DEFAULT,
            pos_icon: (0, 0),
            pos_dialog_title: (12, 8),
            pos_dialog_text: (44, 26),
            dialog_position: DIALOG_UPPER,
            icon: 0,
            delay_time: 3,
            dialog_shadow: 0,
            user_skip: false,
            playing_rng: false,
            dialog_face: 0,
            updated_in_battle: false,
            in_dialog: false,
            auto_confirm: false,
        }
    }
}

/// PAL_CharWidth for the Big5 byte stream: full-width (lead byte >= 0x80) is
/// 16 pixels, single ASCII byte is 8.
#[inline]
fn byte_is_lead(b: u8) -> bool {
    b >= 0x80
}

/// Parse a leading (optionally signed) decimal integer, like `wcstol`.
fn parse_leading_int(bytes: &[u8]) -> i64 {
    let mut i = 0;
    let mut sign = 1i64;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        if bytes[i] == b'-' {
            sign = -1;
        }
        i += 1;
    }
    let mut v = 0i64;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        v = v * 10 + (bytes[i] - b'0') as i64;
        i += 1;
    }
    sign * v
}

impl Engine {
    // =======================================================================
    // Initialization (ui.c PAL_InitUI + text.c dialog-icon load).
    // =======================================================================

    /// PAL_InitUI: load the UI sprite and the dialog waiting icons.
    pub fn init_ui(&mut self) -> std::io::Result<()> {
        self.ui.sprite_ui = self
            .globals
            .files
            .data
            .chunk_decompressed(CHUNKNUM_SPRITEUI)?;
        self.ui.dialog_icons = self
            .globals
            .files
            .data
            .chunk_decompressed(CHUNKNUM_DIALOGICONS)?;
        Ok(())
    }

    // =======================================================================
    // Low-level UI sprite helpers.
    // =======================================================================

    /// Width/height of frame `n` of gpSpriteUI.
    fn ui_frame_size(&self, n: usize) -> (i32, i32) {
        surface::sprite_frame(&self.ui.sprite_ui, n)
            .map(|f| (surface::rle_width(f) as i32, surface::rle_height(f) as i32))
            .unwrap_or((0, 0))
    }

    /// Blit frame `n` of gpSpriteUI to the work surface (PAL_RLEBlitToSurface).
    fn blit_ui_frame(&mut self, n: usize, x: i32, y: i32) {
        if let Some(f) = surface::sprite_frame(&self.ui.sprite_ui, n) {
            self.screen.blit_rle(f, x, y);
        }
    }

    /// Blit the shadow of frame `n` of gpSpriteUI
    /// (PAL_RLEBlitToSurfaceWithShadow).
    fn blit_ui_frame_shadow(&mut self, n: usize, x: i32, y: i32) {
        if let Some(f) = surface::sprite_frame(&self.ui.sprite_ui, n) {
            self.screen.blit_rle_shadow(f, x, y);
        }
    }

    /// Blit a raw RLE bitmap (e.g. an item picture from BALL.MKF).
    pub fn blit_rle_bitmap(&mut self, rle: &[u8], x: i32, y: i32) {
        self.screen.blit_rle(rle, x, y);
    }

    /// Blit frame `n` of gpSpriteUI (public entry for the menu modules).
    pub fn draw_ui_sprite(&mut self, n: usize, x: i32, y: i32) {
        self.blit_ui_frame(n, x, y);
    }

    /// Blit the shadow of frame `n` of gpSpriteUI (public entry).
    pub fn draw_ui_sprite_shadow(&mut self, n: usize, x: i32, y: i32) {
        self.blit_ui_frame_shadow(n, x, y);
    }

    // =======================================================================
    // Text drawing (text.c PAL_DrawText / PAL_UnescapeText, ui.c widths).
    // =======================================================================

    /// PAL_UnescapeText: strip the dialog control bytes and `\` escapes while
    /// keeping Big5 double-byte characters intact.
    fn unescape_text(text: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let b = text[i];
            if byte_is_lead(b) && i + 1 < text.len() {
                out.push(b);
                out.push(text[i + 1]);
                i += 2;
                continue;
            }
            match b {
                b'-' | b'\'' | b'@' | b'"' | b'$' | b'~' | b')' | b'(' => {
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    if i < text.len() {
                        out.push(text[i]);
                        i += 1;
                    }
                }
                _ => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        out
    }

    /// PAL_DrawText (unescaping form). Draws Big5 text at `pos` in `color`,
    /// with the DOS triple-shadow when `shadow`, and optionally refreshes the
    /// screen.
    pub fn draw_text(
        &mut self,
        text: &[u8],
        pos: (i32, i32),
        color: u8,
        shadow: bool,
        update: bool,
    ) {
        if pos.0 >= SCREEN_W as i32 {
            return;
        }
        let unescaped = Self::unescape_text(text);
        self.font
            .draw_text(&mut self.screen, &unescaped, pos.0, pos.1, color, shadow);
        if update {
            self.video_update();
        }
    }

    /// PAL_TextWidth: pixel width of a Big5 string.
    pub fn text_width(&self, text: &[u8]) -> i32 {
        self.font.text_width(text)
    }

    /// PAL_WordWidth: width of a word in units of 16-pixel columns.
    pub fn word_width(&self, word_num: u16) -> i32 {
        let w = self.texts.word(word_num as usize);
        (self.font.text_width(&w) + 8) >> 4
    }

    /// PAL_WordMaxWidth: max column width over a range of words.
    pub fn word_max_width(&self, first_word: u16, count: u16) -> i32 {
        let mut r = 0;
        for i in 0..count {
            let w = (self.text_width(&self.texts.word((first_word + i) as usize)) + 8) >> 4;
            if r < w {
                r = w;
            }
        }
        r
    }

    /// PAL_MenuTextMaxWidth: max column width over a menu's labels.
    pub fn menu_text_max_width(&self, items: &[MenuItem]) -> i32 {
        let mut r = 0;
        for it in items {
            let raw = self.texts.word(it.num_word as usize);
            let unescaped = Self::unescape_text(&raw);
            let w = (self.font.text_width(&unescaped) + 8) >> 4;
            if r < w {
                r = w;
            }
        }
        r
    }

    // =======================================================================
    // Boxes (ui.c).
    // =======================================================================

    /// Save the screen area covered by a box (PAL_CreateBoxInternal /
    /// VIDEO_DuplicateSurface). Returns a non-null handle.
    fn create_box_internal(&mut self, x: i32, y: i32, w: i32, h: i32) -> BoxHandle {
        let mut saved = vec![0u8; (w.max(0) * h.max(0)) as usize];
        for row in 0..h {
            for col in 0..w {
                saved[(row * w + col) as usize] = self.screen.get_pixel(x + col, y + row);
            }
        }
        self.ui.boxes.push(Some(BoxRecord {
            pos: (x, y),
            w,
            h,
            saved,
        }));
        self.ui.boxes.len()
    }

    /// PAL_CreateBox.
    pub fn create_box(
        &mut self,
        pos: (i32, i32),
        rows: i32,
        columns: i32,
        style: i32,
        save_screen: bool,
    ) -> BoxHandle {
        self.create_box_with_shadow(pos, rows, columns, style, save_screen, 6)
    }

    /// PAL_CreateBoxWithShadow.
    pub fn create_box_with_shadow(
        &mut self,
        pos: (i32, i32),
        rows: i32,
        columns: i32,
        style: i32,
        save_screen: bool,
        shadow_offset: i32,
    ) -> BoxHandle {
        // Border bitmap frame numbers, indexed [row][col].
        let frame = |i: i32, j: i32| (i * 3 + j + style * 9) as usize;

        // Total width/height of the box (from the border bitmaps).
        let mut w = 0;
        let mut h = 0;
        for i in 0..3 {
            if i == 1 {
                w += self.ui_frame_size(frame(0, i)).0 * columns;
                h += self.ui_frame_size(frame(i, 0)).1 * rows;
            } else {
                w += self.ui_frame_size(frame(0, i)).0;
                h += self.ui_frame_size(frame(i, 0)).1;
            }
        }
        w += shadow_offset;
        h += shadow_offset;

        let handle = if save_screen {
            self.create_box_internal(pos.0, pos.1, w, h)
        } else {
            0
        };

        // Border takes 2 additional rows and columns.
        let nrows = rows + 2;
        let ncolumns = columns + 2;

        let mut ry = pos.1;
        for i in 0..nrows {
            let mut x = pos.0;
            let m = if i == 0 {
                0
            } else if i == nrows - 1 {
                2
            } else {
                1
            };
            for j in 0..ncolumns {
                let n = if j == 0 {
                    0
                } else if j == ncolumns - 1 {
                    2
                } else {
                    1
                };
                let fnum = frame(m, n);
                self.blit_ui_frame_shadow(fnum, x + shadow_offset, ry + shadow_offset);
                self.blit_ui_frame(fnum, x, ry);
                x += self.ui_frame_size(fnum).0;
            }
            ry += self.ui_frame_size(frame(m, 0)).1;
        }

        handle
    }

    /// PAL_CreateSingleLineBox.
    pub fn create_single_line_box(
        &mut self,
        pos: (i32, i32),
        len: i32,
        save_screen: bool,
    ) -> BoxHandle {
        self.create_single_line_box_with_shadow(pos, len, save_screen, 6)
    }

    /// PAL_CreateSingleLineBoxWithShadow.
    pub fn create_single_line_box_with_shadow(
        &mut self,
        pos: (i32, i32),
        len: i32,
        save_screen: bool,
        shadow_offset: i32,
    ) -> BoxHandle {
        const LEFT: usize = 44;
        const MID: usize = 45;
        const RIGHT: usize = 46;

        let (lw, lh) = self.ui_frame_size(LEFT);
        let (mw, _) = self.ui_frame_size(MID);
        let (rw, _) = self.ui_frame_size(RIGHT);

        let w = lw + rw + mw * len + shadow_offset;
        let h = lh + shadow_offset;

        let handle = if save_screen {
            self.create_box_internal(pos.0, pos.1, w, h)
        } else {
            0
        };

        // Draw the shadow row.
        let mut x = pos.0;
        self.blit_ui_frame_shadow(LEFT, x + shadow_offset, pos.1 + shadow_offset);
        x += lw;
        for _ in 0..len {
            self.blit_ui_frame_shadow(MID, x + shadow_offset, pos.1 + shadow_offset);
            x += mw;
        }
        self.blit_ui_frame_shadow(RIGHT, x + shadow_offset, pos.1 + shadow_offset);

        // Draw the box itself.
        let mut x = pos.0;
        self.blit_ui_frame(LEFT, x, pos.1);
        x += lw;
        for _ in 0..len {
            self.blit_ui_frame(MID, x, pos.1);
            x += mw;
        }
        self.blit_ui_frame(RIGHT, x, pos.1);

        handle
    }

    /// PAL_DeleteBox: restore the saved background under the box.
    pub fn delete_box(&mut self, handle: BoxHandle) {
        if handle == 0 || handle > self.ui.boxes.len() {
            return;
        }
        if let Some(rec) = self.ui.boxes[handle - 1].take() {
            for row in 0..rec.h {
                for col in 0..rec.w {
                    let c = rec.saved[(row * rec.w + col) as usize];
                    self.screen.put_pixel(rec.pos.0 + col, rec.pos.1 + row, c);
                }
            }
        }
    }

    // =======================================================================
    // Numbers (ui.c PAL_DrawNumber).
    // =======================================================================

    /// PAL_DrawNumber.
    pub fn draw_number(
        &mut self,
        num: u32,
        len: usize,
        pos: (i32, i32),
        color: NumColor,
        align: NumAlign,
    ) {
        // Blue starts from 29, Cyan from 56, Yellow from 19.
        let base = match color {
            NumColor::Blue => 29usize,
            NumColor::Cyan => 56,
            NumColor::Yellow => 19,
        };

        // Actual number of digits.
        let mut i = num;
        let mut actual_len = 0usize;
        while i > 0 {
            i /= 10;
            actual_len += 1;
        }
        if actual_len > len {
            actual_len = len;
        } else if actual_len == 0 {
            actual_len = 1;
        }

        let mut x = pos.0 - 6;
        let y = pos.1;
        match align {
            NumAlign::Left => x += 6 * actual_len as i32,
            NumAlign::Mid => x += 3 * (len + actual_len) as i32,
            NumAlign::Right => x += 6 * len as i32,
        }

        let mut num = num;
        let mut remaining = actual_len;
        while remaining > 0 {
            self.blit_ui_frame(base + (num % 10) as usize, x, y);
            x -= 6;
            num /= 10;
            remaining -= 1;
        }
    }

    // =======================================================================
    // Menu (ui.c PAL_ReadMenu).
    // =======================================================================

    /// Current shimmering "selected" color (ui.h MENUITEM_COLOR_SELECTED).
    pub fn menuitem_color_selected(&self) -> u8 {
        MENUITEM_COLOR_SELECTED_FIRST.wrapping_add(
            ((self.ticks() / (600 / MENUITEM_COLOR_SELECTED_TOTALNUM as u64))
                % MENUITEM_COLOR_SELECTED_TOTALNUM as u64) as u8,
        )
    }

    /// PAL_ReadMenu, keeping the stub's parameter list; default item is 0.
    pub fn read_menu(
        &mut self,
        items: &[MenuItem],
        label_color: u8,
        on_change: Option<MenuItemChanged<'_>>,
    ) -> Option<u16> {
        self.read_menu_default(items, 0, label_color, on_change)
    }

    /// PAL_ReadMenu with the full C signature (adds `default_item`). Returns
    /// the chosen value, or `None` on cancel.
    pub fn read_menu_default(
        &mut self,
        items: &[MenuItem],
        default_item: u16,
        label_color: u8,
        mut on_change: Option<MenuItemChanged<'_>>,
    ) -> Option<u16> {
        if items.is_empty() {
            return None;
        }
        let n = items.len();
        let mut current = if (default_item as usize) < n {
            default_item as usize
        } else {
            0
        };

        // Draw all menu texts.
        for (i, it) in items.iter().enumerate() {
            let mut color = label_color;
            if !it.enabled {
                color = if i == current {
                    MENUITEM_COLOR_SELECTED_INACTIVE
                } else {
                    MENUITEM_COLOR_INACTIVE
                };
            }
            let word = self.texts.word(it.num_word as usize);
            self.draw_text(&word, it.pos, color, true, false);
        }
        self.video_update();

        if let Some(cb) = on_change.as_mut() {
            cb(self, items[current].value);
        }

        // Headless escape hatch: confirm the default item immediately.
        if self.ui.auto_confirm {
            if items[current].enabled {
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(
                    &word,
                    items[current].pos,
                    MENUITEM_COLOR_CONFIRMED,
                    false,
                    true,
                );
                return Some(items[current].value);
            }
            return None;
        }

        // PAL_ReadMenu: the wait sits BETWEEN clear_key_state and the key
        // checks so presses arriving while we sleep survive to the checks.
        let mut deadline = self.ticks();
        loop {
            self.input.clear_key_state();

            // Redraw the selected item shimmering.
            if items[current].enabled {
                let color = self.menuitem_color_selected();
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, true);
            }

            self.process_event();
            self.delay_until(deadline);
            deadline = self.ticks() + 50;
            if self.quit_requested {
                return None;
            }

            use crate::input::{KEY_DOWN, KEY_LEFT, KEY_MENU, KEY_RIGHT, KEY_SEARCH, KEY_UP};

            if self.input.pressed(KEY_DOWN | KEY_RIGHT) {
                // Dehighlight current.
                let color = if items[current].enabled {
                    label_color
                } else {
                    MENUITEM_COLOR_INACTIVE
                };
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, false);

                current = (current + 1) % n;

                let color = if items[current].enabled {
                    self.menuitem_color_selected()
                } else {
                    MENUITEM_COLOR_SELECTED_INACTIVE
                };
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, false);
                self.video_update();

                if let Some(cb) = on_change.as_mut() {
                    cb(self, items[current].value);
                }
            } else if self.input.pressed(KEY_UP | KEY_LEFT) {
                let color = if items[current].enabled {
                    label_color
                } else {
                    MENUITEM_COLOR_INACTIVE
                };
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, false);

                current = if current > 0 { current - 1 } else { n - 1 };

                let color = if items[current].enabled {
                    self.menuitem_color_selected()
                } else {
                    MENUITEM_COLOR_SELECTED_INACTIVE
                };
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, false);
                self.video_update();

                if let Some(cb) = on_change.as_mut() {
                    cb(self, items[current].value);
                }
            } else if self.input.pressed(KEY_MENU) {
                let color = if items[current].enabled {
                    label_color
                } else {
                    MENUITEM_COLOR_INACTIVE
                };
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(&word, items[current].pos, color, false, false);
                break;
            } else if self.input.pressed(KEY_SEARCH) && items[current].enabled {
                let word = self.texts.word(items[current].num_word as usize);
                self.draw_text(
                    &word,
                    items[current].pos,
                    MENUITEM_COLOR_CONFIRMED,
                    false,
                    false,
                );
                return Some(items[current].value);
            }
        }

        None
    }

    // =======================================================================
    // Dialog (text.c).
    // =======================================================================

    /// PAL_DialogSetDelayTime.
    pub fn dialog_set_delay_time(&mut self, delay: i32) {
        self.ui.delay_time = delay;
    }

    /// PAL_IsInDialog.
    pub fn is_in_dialog(&self) -> bool {
        self.ui.current_dialog_line != 0
    }

    /// PAL_DialogIsPlayingRNG.
    pub fn dialog_is_playing_rng(&self) -> bool {
        self.ui.playing_rng
    }

    /// PAL_StartDialog.
    pub fn start_dialog(
        &mut self,
        dialog_location: u8,
        font_color: u8,
        num_char_face: i32,
        playing_rng: bool,
    ) {
        self.start_dialog_with_offset(
            dialog_location,
            font_color,
            num_char_face,
            playing_rng,
            0,
            0,
        );
    }

    /// PAL_StartDialogWithOffset.
    pub fn start_dialog_with_offset(
        &mut self,
        dialog_location: u8,
        font_color: u8,
        num_char_face: i32,
        playing_rng: bool,
        x_off: i32,
        y_off: i32,
    ) {
        if self.globals.in_battle && !self.ui.updated_in_battle {
            self.video_update();
            self.ui.updated_in_battle = true;
        }

        self.ui.icon = 0;
        self.ui.pos_icon = (0, 0);
        self.ui.current_dialog_line = 0;
        self.ui.pos_dialog_title = (12, 8);
        self.ui.user_skip = false;
        self.ui.dialog_face = if dialog_location == DIALOG_UPPER || dialog_location == DIALOG_LOWER
        {
            num_char_face
        } else {
            0
        };

        if font_color != 0 {
            self.ui.current_font_color = font_color;
        }

        if playing_rng && num_char_face != 0 {
            self.backup_screen();
            self.ui.playing_rng = true;
        }

        match dialog_location {
            DIALOG_UPPER => {
                if num_char_face > 0 {
                    if let Ok(buf) = self
                        .globals
                        .files
                        .rgm
                        .chunk_decompressed(num_char_face as usize)
                    {
                        let fw = surface::rle_width(&buf) as i32;
                        let fh = surface::rle_height(&buf) as i32;
                        let rx = (48 - fw / 2 + x_off).max(0);
                        let ry = (55 - fh / 2 + y_off).max(0);
                        self.screen.blit_rle(&buf, rx, ry);
                        self.video_update();
                    }
                }
                self.ui.pos_dialog_title = (if num_char_face > 0 { 80 } else { 12 }, 8);
                self.ui.pos_dialog_text = (if num_char_face > 0 { 96 } else { 44 }, 26);
            }
            DIALOG_CENTER => {
                self.ui.pos_dialog_text = (80, 40);
            }
            DIALOG_LOWER => {
                if num_char_face > 0 {
                    if let Ok(buf) = self
                        .globals
                        .files
                        .rgm
                        .chunk_decompressed(num_char_face as usize)
                    {
                        let fw = surface::rle_width(&buf) as i32;
                        let fh = surface::rle_height(&buf) as i32;
                        let rx = 270 - fw / 2 + x_off;
                        let ry = 144 - fh / 2 + y_off;
                        self.screen.blit_rle(&buf, rx, ry);
                        self.video_update();
                    }
                }
                self.ui.pos_dialog_title = (if num_char_face > 0 { 4 } else { 12 }, 108);
                self.ui.pos_dialog_text = (if num_char_face > 0 { 20 } else { 44 }, 126);
            }
            DIALOG_CENTER_WINDOW => {
                self.ui.pos_dialog_text = (160, 40);
            }
            _ => {}
        }

        self.ui.pos_dialog_title.0 += x_off;
        self.ui.pos_dialog_title.1 += y_off;
        self.ui.pos_dialog_text.0 += x_off;
        self.ui.pos_dialog_text.1 += y_off;
        self.ui.dialog_position = dialog_location;
    }

    /// Delay wrapper that honours the headless escape hatch.
    fn ui_delay(&mut self, ms: u64) {
        if self.ui.auto_confirm {
            self.process_event();
            return;
        }
        self.delay(ms);
    }

    /// TEXT_DisplayText: draw one interpreted line of text, honouring the
    /// byte-level dialog control codes. Returns the x coordinate just past
    /// the drawn text.
    ///
    /// Control bytes (interpreted only outside a Big5 double-byte char):
    /// `-` toggle cyan, `'` toggle red, `@` toggle alt-red, `"` toggle yellow
    /// (non-dialog only), `$NN` set per-char delay, `~NN` delay-and-end line,
    /// `)`/`(` set waiting icon 1/2, `\` escape next byte.
    fn display_text(&mut self, text: &[u8], mut x: i32, y: i32, is_dialog: bool) -> i32 {
        let mut i = 0;
        while i < text.len() {
            let b = text[i];

            // Big5 double-byte character: consume atomically, draw it.
            if byte_is_lead(b) && i + 1 < text.len() {
                let ch = [b, text[i + 1]];
                i += 2;
                self.draw_dialog_char(&ch, &mut x, y, is_dialog, false);
                if !is_dialog && !self.ui.user_skip {
                    self.per_char_delay();
                }
                continue;
            }

            match b {
                b'-' => {
                    self.ui.current_font_color = if self.ui.current_font_color == FONT_COLOR_CYAN {
                        FONT_COLOR_DEFAULT
                    } else {
                        FONT_COLOR_CYAN
                    };
                    i += 1;
                }
                b'\'' => {
                    self.ui.current_font_color = if self.ui.current_font_color == FONT_COLOR_RED {
                        FONT_COLOR_DEFAULT
                    } else {
                        FONT_COLOR_RED
                    };
                    i += 1;
                }
                b'@' => {
                    self.ui.current_font_color = if self.ui.current_font_color == FONT_COLOR_RED_ALT
                    {
                        FONT_COLOR_DEFAULT
                    } else {
                        FONT_COLOR_RED_ALT
                    };
                    i += 1;
                }
                b'"' => {
                    if !is_dialog {
                        self.ui.current_font_color =
                            if self.ui.current_font_color == FONT_COLOR_YELLOW {
                                FONT_COLOR_DEFAULT
                            } else {
                                FONT_COLOR_YELLOW
                            };
                    }
                    i += 1;
                }
                b'$' => {
                    let v = parse_leading_int(&text[i + 1..]);
                    self.ui.delay_time = (v * 10 / 7) as i32;
                    i += 3;
                }
                b'~' => {
                    if self.ui.user_skip {
                        self.video_update();
                    }
                    if !is_dialog {
                        let v = parse_leading_int(&text[i + 1..]);
                        self.ui_delay((v * 80 / 7).max(0) as u64);
                    }
                    self.ui.current_dialog_line = -1;
                    self.ui.user_skip = false;
                    return x;
                }
                b')' => {
                    self.ui.icon = 1;
                    i += 1;
                }
                b'(' => {
                    self.ui.icon = 2;
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    if i < text.len() {
                        let b2 = text[i];
                        if byte_is_lead(b2) && i + 1 < text.len() {
                            let ch = [b2, text[i + 1]];
                            i += 2;
                            self.draw_dialog_char(&ch, &mut x, y, is_dialog, false);
                        } else {
                            let ch = [b2];
                            i += 1;
                            self.draw_dialog_char(&ch, &mut x, y, is_dialog, false);
                        }
                        if !is_dialog && !self.ui.user_skip {
                            self.per_char_delay();
                        }
                    }
                }
                _ => {
                    let ch = [b];
                    i += 1;
                    self.draw_dialog_char(&ch, &mut x, y, is_dialog, true);
                    if !is_dialog && !self.ui.user_skip {
                        self.per_char_delay();
                    }
                }
            }
        }
        x
    }

    /// Draw one character (glyph bytes) of dialog/interpreted text, advancing
    /// `x`. `maybe_number` marks a single ASCII byte that could be a digit.
    fn draw_dialog_char(
        &mut self,
        ch: &[u8],
        x: &mut i32,
        y: i32,
        is_dialog: bool,
        maybe_number: bool,
    ) {
        let mut color = self.ui.current_font_color;
        let mut is_number = false;
        if is_dialog {
            if self.ui.current_font_color == FONT_COLOR_DEFAULT {
                color = 0;
            }
            if maybe_number && ch.len() == 1 && ch[0].is_ascii_digit() {
                is_number = true;
            }
        }

        if is_number {
            self.draw_number(
                (ch[0] - b'0') as u32,
                1,
                (*x, y + 4),
                NumColor::Yellow,
                NumAlign::Left,
            );
        } else {
            let shadow = !is_dialog;
            self.font
                .draw_text(&mut self.screen, ch, *x, y, color, shadow);
            if !is_dialog && !self.ui.user_skip {
                self.video_update();
            }
        }

        *x += if ch.len() == 2 { 16 } else { 8 };
    }

    /// The per-character reveal delay and skip check for non-dialog text.
    fn per_char_delay(&mut self) {
        self.input.clear_key_state();
        self.ui_delay((self.ui.delay_time * 8).max(0) as u64);
        use crate::input::{KEY_MENU, KEY_SEARCH};
        if self.input.pressed(KEY_SEARCH | KEY_MENU) {
            self.ui.user_skip = true;
        }
    }

    /// PAL_ShowDialogText.
    pub fn show_dialog_text(&mut self, text: &[u8]) {
        self.input.clear_key_state();
        self.ui.icon = 0;

        if self.globals.in_battle && !self.ui.updated_in_battle {
            self.video_update();
            self.ui.updated_in_battle = true;
        }

        if self.ui.current_dialog_line > 3 {
            // Rest of the dialog goes on the next page.
            self.dialog_wait_for_key();
            self.ui.current_dialog_line = 0;
            self.restore_screen();
            self.video_update();
        }

        let x = self.ui.pos_dialog_text.0;
        let y = self.ui.pos_dialog_text.1 + self.ui.current_dialog_line * 18;

        if self.ui.dialog_position == DIALOG_CENTER_WINDOW {
            // Small window at the center of the screen.
            let len: i32 = self.font.text_width(text) >> 3;
            let pos = (
                self.ui.pos_dialog_text.0 - len * 4,
                self.ui.pos_dialog_text.1,
            );
            let shadow = self.ui.dialog_shadow;
            let lp_box = self.create_single_line_box_with_shadow(pos, (len + 1) / 2, false, shadow);
            self.video_update();

            self.display_text(text, pos.0 + 8 + ((len & 1) << 2), pos.1 + 10, true);
            self.video_update();

            self.dialog_wait_for_key_with_max(1.4);

            self.delete_box(lp_box);
            self.video_update();

            self.end_dialog();
        } else {
            // Detect a "name of character" line (ends with a full-width or
            // half-width colon): full-width colon is Big5 0xA1 0x47.
            let ends_with_colon = {
                let l = text.len();
                (l >= 2 && text[l - 2] == 0xA1 && text[l - 1] == 0x47)
                    || (l >= 1 && text[l - 1] == b':')
            };

            if self.ui.current_dialog_line == 0
                && self.ui.dialog_position != DIALOG_CENTER
                && ends_with_colon
            {
                let pos = self.ui.pos_dialog_title;
                self.draw_text(text, pos, FONT_COLOR_CYAN_ALT, true, true);
            } else {
                if !self.ui.playing_rng && self.ui.current_dialog_line == 0 {
                    self.backup_screen();
                }

                let nx = self.display_text(text, x, y, false);

                if self.ui.user_skip {
                    self.video_update();
                }

                self.ui.pos_icon = (nx, y);
                self.ui.current_dialog_line += 1;
            }
        }
    }

    /// PAL_DialogWaitForKey.
    fn dialog_wait_for_key(&mut self) {
        self.dialog_wait_for_key_with_max(0.0);
    }

    /// PAL_DialogWaitForKeyWithMaximumSeconds.
    fn dialog_wait_for_key_with_max(&mut self, max_seconds: f32) {
        let animated = self.ui.dialog_position != DIALOG_CENTER_WINDOW
            && self.ui.dialog_position != DIALOG_CENTER;

        // Show the waiting icon.
        if animated {
            if let Some(f) = surface::sprite_frame(&self.ui.dialog_icons, self.ui.icon as usize) {
                let (ix, iy) = self.ui.pos_icon;
                self.screen.blit_rle(f, ix, iy);
                self.video_update();
            }
        }

        self.input.clear_key_state();

        // Headless: don't block.
        if self.ui.auto_confirm {
            self.input.clear_key_state();
            self.ui.user_skip = false;
            return;
        }

        let mut palette = self
            .get_palette(
                self.globals.num_palette as usize,
                self.globals.night_palette,
            )
            .unwrap_or([[0; 3]; 256]);
        let begin = self.ticks();

        loop {
            self.ui_delay(100);

            if animated {
                // Palette shift on entries 0xF9..0xFE.
                let t = palette[0xF9];
                for i in 0xF9..0xFE {
                    palette[i] = palette[i + 1];
                }
                palette[0xFE] = t;
                self.set_raw_palette(palette);
            }

            if max_seconds.abs() > f32::EPSILON
                && (self.ticks() - begin) as f32 > 1000.0 * max_seconds
            {
                break;
            }
            if self.input.key_press != 0 {
                break;
            }
            if self.quit_requested {
                break;
            }
        }

        if animated {
            self.set_palette(
                self.globals.num_palette as usize,
                self.globals.night_palette,
            );
        }

        // The dialog advanced (key press or timeout): cut any voice-over
        // still speaking the lines the player just moved past.
        if let Some(a) = self.audio.as_ref() {
            a.stop_voice();
        }

        self.input.clear_key_state();
        self.ui.user_skip = false;
    }

    /// PAL_ClearDialog.
    pub fn clear_dialog(&mut self, wait_for_key: bool) {
        if self.ui.current_dialog_line > 0 && wait_for_key {
            self.dialog_wait_for_key();
        }

        self.ui.current_dialog_line = 0;

        if self.ui.dialog_position == DIALOG_CENTER {
            self.ui.pos_dialog_title = (12, 8);
            self.ui.pos_dialog_text = (44, 26);
            self.ui.current_font_color = FONT_COLOR_DEFAULT;
            self.ui.dialog_position = DIALOG_UPPER;
        }
    }

    /// PAL_EndDialog.
    pub fn end_dialog(&mut self) {
        self.clear_dialog(true);
        self.ui.pos_dialog_title = (12, 8);
        self.ui.pos_dialog_text = (44, 26);
        self.ui.current_font_color = FONT_COLOR_DEFAULT;
        self.ui.dialog_position = DIALOG_UPPER;
        self.ui.user_skip = false;
        self.ui.playing_rng = false;
        self.ui.dialog_face = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_loop::Engine;

    fn engine() -> Engine {
        std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
        let mut e = Engine::new(true).expect("headless engine");
        e.ui.auto_confirm = true;
        e.init_ui().expect("init ui");
        e
    }

    fn nonzero_pixels(e: &Engine) -> usize {
        e.screen.pixels.iter().filter(|&&p| p != 0).count()
    }

    #[test]
    fn ui_sprite_and_icons_load() {
        let e = engine();
        assert!(surface::sprite_frame_count(&e.ui.sprite_ui) > 70);
        assert!(surface::sprite_frame_count(&e.ui.dialog_icons) > 0);
    }

    #[test]
    fn box_draw_and_delete_restores_pixels() {
        let mut e = engine();
        e.screen.clear(5);
        let before = e.screen.pixels.clone();
        let h = e.create_box((40, 60), 3, 6, 0, true);
        assert_ne!(h, 0);
        assert_ne!(e.screen.pixels, before, "box did not draw");
        e.delete_box(h);
        assert_eq!(e.screen.pixels, before, "delete_box did not restore");
    }

    #[test]
    fn single_line_box_and_showcash() {
        let mut e = engine();
        e.screen.clear(0);
        let h = e.create_single_line_box((0, 0), 5, true);
        assert_ne!(h, 0);
        assert!(nonzero_pixels(&e) > 0);
        e.delete_box(h);
        assert_eq!(nonzero_pixels(&e), 0);
        // Cash box draws text + number.
        e.screen.clear(0);
        let cash = e.show_cash(12345);
        assert_ne!(cash, 0);
        assert!(nonzero_pixels(&e) > 0);
    }

    #[test]
    fn draw_number_renders_pixels() {
        let mut e = engine();
        e.screen.clear(0);
        e.draw_number(1234, 6, (60, 20), NumColor::Yellow, NumAlign::Right);
        assert!(nonzero_pixels(&e) > 0);
        e.screen.clear(0);
        e.draw_number(0, 2, (60, 20), NumColor::Cyan, NumAlign::Right);
        assert!(nonzero_pixels(&e) > 0);
    }

    #[test]
    fn draw_text_renders_and_unescapes() {
        let mut e = engine();
        e.screen.clear(0);
        let w = e.texts.word(3); // 状态 "status"
        assert!(!w.is_empty());
        e.draw_text(&w, (20, 20), MENUITEM_COLOR, true, false);
        assert!(nonzero_pixels(&e) > 0);

        let escaped = b"-\xaa\xac\xba\x41".to_vec(); // '-' + 状态
        let out = Engine::unescape_text(&escaped);
        assert_eq!(out, b"\xaa\xac\xba\x41");
    }

    #[test]
    fn read_menu_auto_confirms_default() {
        let mut e = engine();
        e.screen.clear(0);
        let items = [
            MenuItem {
                value: 10,
                num_word: 7,
                enabled: true,
                pos: (30, 40),
            },
            MenuItem {
                value: 20,
                num_word: 8,
                enabled: true,
                pos: (30, 58),
            },
        ];
        let got = e.read_menu_default(&items, 1, MENUITEM_COLOR, None);
        assert_eq!(got, Some(20));
        assert!(nonzero_pixels(&e) > 0);
    }

    #[test]
    fn dialog_text_draws_and_terminates() {
        let mut e = engine();
        e.globals.load_default_game().unwrap();
        e.screen.clear(0);
        e.start_dialog(DIALOG_LOWER, 0, 0, false);
        e.ui.user_skip = true; // force instant reveal
        let msg = e.texts.msg(0); // 此門已上鎖
        assert!(!msg.is_empty());
        e.show_dialog_text(&msg);
        assert_eq!(e.ui.current_dialog_line, 1);
        assert!(nonzero_pixels(&e) > 0);

        e.start_dialog(DIALOG_LOWER, 0, 0, false);
        e.ui.user_skip = true;
        e.show_dialog_text(b"hello~60");
        e.end_dialog();
        assert!(!e.is_in_dialog());
    }

    #[test]
    fn word_widths() {
        let e = engine();
        assert!(e.word_width(7) >= 1);
        assert!(e.word_max_width(7, 2) >= 1);
    }
}
