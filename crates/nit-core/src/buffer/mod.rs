mod brackets;
mod cursor_motion;
mod diff;
mod edit;
mod edit_tracking;
mod indent;
mod jumplist;
mod repr;
mod scroll;
mod search;
mod selection;
mod types;
mod undo;
mod undo_log;

pub use brackets::{find_matching_bracket, BracketMatch};
pub use jumplist::{JumpEntry, JumpList, JUMPLIST_CAPACITY};
pub use repr::Buffer;
pub use types::{BufferEdit, BufferPoint, LineDiffStatus};

#[cfg(test)]
#[path = "../tests/buffer.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/vim_semantics.rs"]
mod vim_semantics;
