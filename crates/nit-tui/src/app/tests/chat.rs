//! Chat input smoke tests separate from `agent_chat`. Verifies the
//! `state.agents.chat_input` buffer initialises empty and accepts standard
//! text edits without exercising the full event loop.

use super::*;

#[test]
fn chat_input_starts_empty_in_fresh_state() {
    let state = state_for_test();
    assert!(state.agents.chat_input.is_empty());
}

#[test]
fn chat_input_accepts_plain_text() {
    let mut state = state_for_test();
    state.agents.chat_input.push_str("hello world");
    assert_eq!(state.agents.chat_input, "hello world");
}

#[test]
fn chat_input_can_be_cleared() {
    let mut state = state_for_test();
    state.agents.chat_input.push('x');
    state.agents.chat_input.clear();
    assert!(state.agents.chat_input.is_empty());
}

#[test]
fn agents_messages_starts_empty() {
    let state = state_for_test();
    assert_eq!(state.agents.messages.len(), 0);
}
