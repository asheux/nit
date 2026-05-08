//! Quit / dirty-buffer interactions and `OpenFile` buffer-reuse semantics.

use super::*;

#[test]
fn command_q_quits_when_clean_and_prompts_when_dirty() {
    let (_root, mut state) = empty_state("cmd-q");
    assert!(!state.editor_buffer().is_dirty());
    assert!(handle_command_line(&mut state, "q"));

    // Mark dirty: :q must request confirmation instead of exiting.
    state.editor_buffer_mut().insert_char('x');
    assert!(state.editor_buffer().is_dirty());
    assert!(!handle_command_line(&mut state, "q"));
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}

#[test]
fn open_file_creates_new_editor_buffer_when_current_buffer_is_dirty() {
    let root = temp_dir("open-file-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');

    let outcome = apply_action(&mut state, Action::OpenFile(file_b.clone()));

    assert!(!outcome.should_exit);
    assert_eq!(state.buffers.len(), 3);
    assert_eq!(state.active_editor_buffer_id, 2);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.editor_buffer().content_as_string(), "beta");

    let original = state.buffer(0).expect("original editor buffer");
    assert_eq!(original.path(), Some(&file_a));
    assert!(original.is_dirty());
    assert_eq!(original.content_as_string(), "!alpha");
}

#[test]
fn open_file_switches_to_existing_dirty_buffer_instead_of_reloading() {
    let root = temp_dir("open-file-existing");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b.clone()));

    fs::write(&file_a, "disk copy changed").unwrap();
    let outcome = apply_action(&mut state, Action::OpenFile(file_a.clone()));

    assert!(!outcome.should_exit);
    assert_eq!(state.buffers.len(), 3);
    assert_eq!(state.active_editor_buffer_id, 0);
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
    assert!(state.editor_buffer().is_dirty());
    assert_eq!(state.editor_buffer().content_as_string(), "!alpha");
}

#[test]
fn quit_prompts_when_hidden_editor_buffer_is_dirty() {
    let root = temp_dir("quit-hidden-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b));

    let outcome = apply_action(&mut state, Action::Quit);

    assert!(!outcome.should_exit);
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}

#[test]
fn command_q_prompts_when_hidden_editor_buffer_is_dirty() {
    let root = temp_dir("cmd-q-hidden-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b));

    assert!(!handle_command_line(&mut state, "q"));
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}
