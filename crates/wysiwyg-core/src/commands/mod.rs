//! Built-in editing commands.
//!
//! A **command** is a function that takes an `EditorState` and optionally
//! produces a `Transaction`.  When the command is not applicable (e.g., the
//! selection is empty), it returns `None`.  The caller decides whether to
//! apply the returned transaction to produce a new state.

pub mod marks;
pub mod blocks;

pub use marks::{toggle_bold, toggle_italic, toggle_code, toggle_mark};
pub use blocks::{set_block_type, toggle_heading};
