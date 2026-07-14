//! Font Awesome glyphs from the bundled Symbols Nerd Font (private-use area).
//! Codepoints are the classic FA4 assignments Nerd Fonts preserve. Render
//! these through [`crate::theme::mono`] or any other monospace `FontId` —
//! [`crate::theme::apply`] installs the Nerd Font/Symbols2 fallback fonts
//! that back them.

pub const PLAY: &str = "\u{f04b}";
pub const PAUSE: &str = "\u{f04c}";
pub const STEP_BACK: &str = "\u{f053}"; // chevron-left
pub const STEP_FWD: &str = "\u{f054}"; // chevron-right
pub const ZOOM_OUT: &str = "\u{f010}"; // search-minus
pub const ZOOM_IN: &str = "\u{f00e}"; // search-plus
pub const FIT: &str = "\u{f0b2}"; // arrows-alt (expand to full)
pub const TO_MARKS: &str = "\u{f05b}"; // crosshairs
pub const JUMP_IN: &str = "\u{f048}"; // step-backward (|◀ to boundary)
pub const JUMP_OUT: &str = "\u{f051}"; // step-forward (▶| to boundary)
pub const MOVE_UP: &str = "\u{f077}"; // chevron-up
pub const MOVE_DOWN: &str = "\u{f078}"; // chevron-down
pub const DELETE: &str = "\u{f00d}"; // times
pub const ADD: &str = "\u{f067}"; // plus
pub const EDIT: &str = "\u{f040}"; // pencil
pub const SAVE: &str = "\u{f0c7}"; // floppy
pub const FOLDER: &str = "\u{f07c}"; // folder-open
pub const REFRESH: &str = "\u{f021}"; // refresh
pub const PIN: &str = "\u{f08d}"; // thumb-tack
