//! Keyboard input state, mapped from winit key events.
#![allow(dead_code)]

/// Game-relevant keys (mirrors SDLPAL's PAL_INPUT_STATE essentials).
#[derive(Default, Clone, Copy)]
pub struct InputState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// Confirm / search (Enter, Space).
    pub confirm: bool,
    /// Cancel / menu (Escape).
    pub cancel: bool,
}

impl InputState {
    pub fn any_dir(&self) -> bool {
        self.up || self.down || self.left || self.right
    }
}
