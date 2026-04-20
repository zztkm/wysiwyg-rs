//! Built-in editing commands.
//!
//! A **command** is a function that takes an `EditorState` and optionally
//! produces a `Transaction`.  When the command is not applicable (e.g., the
//! selection is empty), it returns `None`.  The caller decides whether to
//! apply the returned transaction to produce a new state.

pub mod blocks;
pub mod input;
pub mod marks;

pub use blocks::{set_block_type, toggle_heading};
pub use input::{backspace, delete_selection, insert_text, split_block};
pub use marks::{toggle_bold, toggle_code, toggle_italic, toggle_mark};
