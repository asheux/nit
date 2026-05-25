//! Buffer-side JumpList ring exercises.
//!
//! These tests duplicate-by-design with the inline buffer-tests block at
//! the bottom of `tests/buffer.rs` only on the surface the state-side
//! integrator depends on: `push` truncation semantics and `jump_back` /
//! `jump_forward` walking. Keeping a focused file separate from the
//! kitchen-sink buffer test file makes the cross-buffer push contract
//! easier to spot when it breaks.

use crate::buffer::{JumpEntry, JumpList, JUMPLIST_CAPACITY};

#[test]
fn push_then_back_returns_pushed_entry() {
    let mut list = JumpList::new();
    list.push(JumpEntry::new(0, 12, 5));
    assert_eq!(list.jump_back(), Some(JumpEntry::new(0, 12, 5)));
}

#[test]
fn back_when_empty_returns_none() {
    let mut list = JumpList::new();
    assert!(list.jump_back().is_none());
}

#[test]
fn entries_cap_at_jumplist_capacity() {
    let mut list = JumpList::new();
    for line in 0..(JUMPLIST_CAPACITY + 7) {
        list.push(JumpEntry::new(0, line, 0));
    }
    assert_eq!(list.len(), JUMPLIST_CAPACITY);
}

#[test]
fn cross_buffer_entries_preserve_buffer_id() {
    let mut list = JumpList::new();
    list.push(JumpEntry::new(0, 1, 0));
    list.push(JumpEntry::new(2, 5, 3));
    let entry = list.jump_back().expect("entry");
    assert_eq!(entry.buffer_id, 2);
    assert_eq!(entry.line, 5);
    assert_eq!(entry.col, 3);
}

#[test]
fn jumplist_is_cleared_to_empty() {
    let mut list = JumpList::new();
    list.push(JumpEntry::new(0, 1, 0));
    list.push(JumpEntry::new(0, 2, 0));
    list.clear();
    assert!(list.is_empty());
    assert!(list.jump_back().is_none());
}
