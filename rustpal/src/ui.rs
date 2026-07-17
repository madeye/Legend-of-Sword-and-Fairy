//! UI framework and dialog system (port of SDLPAL ui.c and the dialog part
//! of text.c). BRING-UP STUB — the real port replaces this file; the
//! signatures below are the stable contract other modules compile against.
#![allow(dead_code)] // used incrementally as engine bring-up proceeds

use crate::game_loop::Engine;

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
pub type BoxHandle = usize;

/// Callback invoked when the highlighted menu item changes
/// (lpfnMenuItemChanged).
pub type MenuItemChanged<'a> = &'a mut dyn FnMut(&mut Engine, u16);

/// Module-private UI/dialog state (text.c g_TextLib and ui.c statics).
#[derive(Default)]
pub struct UiState {
    /// Whether a dialog is currently on screen.
    pub in_dialog: bool,
    /// Dialog is showing while an RNG cutscene plays.
    pub playing_rng: bool,
}

impl Engine {
    // ==== dialog (text.c) ====

    /// PAL_DialogSetDelayTime.
    pub fn dialog_set_delay_time(&mut self, _delay: i32) {
        // STUB
    }

    /// PAL_StartDialog.
    pub fn start_dialog(
        &mut self,
        _dialog_location: u8,
        _font_color: u8,
        _num_char_face: i32,
        _playing_rng: bool,
    ) {
        // STUB
    }

    /// PAL_ShowDialogText.
    pub fn show_dialog_text(&mut self, _text: &[u8]) {
        // STUB
    }

    /// PAL_ClearDialog.
    pub fn clear_dialog(&mut self, _wait_for_key: bool) {
        // STUB
    }

    /// PAL_EndDialog.
    pub fn end_dialog(&mut self) {
        // STUB
    }

    // ==== boxes / menus / numbers (ui.c) ====

    /// PAL_CreateBox.
    pub fn create_box(
        &mut self,
        _pos: (i32, i32),
        _rows: i32,
        _columns: i32,
        _style: i32,
        _save_screen: bool,
    ) -> BoxHandle {
        // STUB
        0
    }

    /// PAL_CreateSingleLineBox.
    pub fn create_single_line_box(
        &mut self,
        _pos: (i32, i32),
        _len: i32,
        _save_screen: bool,
    ) -> BoxHandle {
        // STUB
        0
    }

    /// PAL_DeleteBox: restore the saved background under the box.
    pub fn delete_box(&mut self, _handle: BoxHandle) {
        // STUB
    }

    /// PAL_DrawNumber.
    pub fn draw_number(
        &mut self,
        _num: u32,
        _len: usize,
        _pos: (i32, i32),
        _color: NumColor,
        _align: NumAlign,
    ) {
        // STUB
    }

    /// PAL_ReadMenu: run a menu loop; returns the chosen item value or None
    /// on cancel (MENUITEM_VALUE_CANCELLED). `on_change` is invoked whenever
    /// the highlighted item changes (like lpfnMenuItemChanged).
    pub fn read_menu(
        &mut self,
        _items: &[MenuItem],
        _label_color: u8,
        _on_change: Option<MenuItemChanged<'_>>,
    ) -> Option<u16> {
        // STUB
        None
    }

    /// PAL_WordWidth (in units of 16-pixel columns).
    pub fn word_width(&self, _word_num: u16) -> i32 {
        // STUB
        1
    }
}
