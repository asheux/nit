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

#[test]
fn command_w_writes_active_buffer_to_disk() {
    let root = temp_dir("cmd-w-write");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    assert!(state.editor_buffer().is_dirty());

    // `:w` writes, marks clean, surfaces "Saved" in status. Does NOT exit.
    assert!(!handle_command_line(&mut state, "w"));
    assert!(!state.editor_buffer().is_dirty());
    assert_eq!(state.status.as_deref(), Some("Saved"));
    assert_eq!(fs::read_to_string(&file_a).unwrap(), "!alpha");
}

#[test]
fn command_w_reports_no_path_for_untitled_buffer() {
    let (_root, mut state) = empty_state("cmd-w-untitled");
    // Initial editor buffer in `empty_state` has no path — `:w` must not
    // silently no-op; it must surface the error so the user knows.
    assert!(state.editor_buffer().path().is_none());
    assert!(!handle_command_line(&mut state, "w"));
    assert_eq!(state.status.as_deref(), Some("No path to save"));
}

#[test]
fn command_wq_quits_when_launched_with_file_path() {
    // `nit foo.rs` launch — `:wq` saves, then quits the editor. Matches
    // vim's single-file ergonomics.
    let root = temp_dir("cmd-wq-file-launch");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.launched_with_file_path = true;
    state.editor_buffer_mut().insert_char('!');

    assert!(handle_command_line(&mut state, "wq"));
    assert!(!state.editor_buffer().is_dirty());
    assert_eq!(fs::read_to_string(&file_a).unwrap(), "!alpha");
}

#[test]
fn command_wq_in_directory_mode_opens_nittree_when_only_one_buffer() {
    // `nit src/` launch with only one editor buffer open: `:wq` saves,
    // closes the buffer (replacing with untitled to keep the
    // active-id invariant), and opens NITTree so the user has somewhere
    // to land. No previous-buffer hop available.
    let root = temp_dir("cmd-wq-dir-launch-single");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.launched_with_file_path = false;
    state.file_tree.open = false;
    state.focus = PaneId::Editor;
    state.editor_buffer_mut().insert_char('!');

    assert!(!handle_command_line(&mut state, "wq"));
    // File is written.
    assert_eq!(fs::read_to_string(&file_a).unwrap(), "!alpha");
    // Active buffer was replaced with an untitled blank (closed in place).
    assert!(state.editor_buffer().path().is_none());
    assert_eq!(state.editor_buffer().content_as_string(), "");
    // NITTree was opened so the user has a landing place.
    assert!(state.file_tree.open);
    assert_eq!(state.focus, PaneId::Editor);
}

#[test]
fn command_wq_in_directory_mode_switches_to_last_buffer_when_multiple_open() {
    // `nit src/` launch with TWO editor buffers open: `:wq` saves the
    // active one, removes it from the buffer list, and switches to the
    // remaining file buffer. NITTree is NOT triggered when another
    // buffer is still around.
    let root = temp_dir("cmd-wq-dir-launch-multi");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.launched_with_file_path = false;
    state.file_tree.open = false;
    // Open b.txt while a.txt is dirty → pushed as a new buffer; b becomes
    // active. Buffer layout: [a (dirty), notes, b (active)].
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b.clone()));
    assert_eq!(state.buffers.len(), 3);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));

    // Save + close b.txt — should switch back to a.txt, NOT open NITTree.
    state.editor_buffer_mut().insert_char('?');
    assert!(!handle_command_line(&mut state, "wq"));
    assert_eq!(fs::read_to_string(&file_b).unwrap(), "?beta");
    // Active buffer is now a.txt (the remaining file buffer).
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
    // NITTree was NOT opened; we had a buffer to switch to.
    assert!(!state.file_tree.open);
    assert_eq!(state.focus, PaneId::Editor);
    // Buffer was removed, not just hidden.
    assert_eq!(state.buffers.len(), 2);
}

#[test]
fn command_wq_does_not_quit_when_save_fails_on_untitled() {
    // `:wq` on a path-less buffer: save fails, status reports the
    // problem, and we DO NOT quit (because the file wasn't actually
    // saved). Important guard against data loss.
    let (_root, mut state) = empty_state("cmd-wq-untitled-no-quit");
    state.launched_with_file_path = true;
    state.editor_buffer_mut().insert_char('x');

    assert!(!handle_command_line(&mut state, "wq"));
    assert_eq!(state.status.as_deref(), Some("No path to save"));
    // Buffer still dirty — the failed save didn't lie about success.
    assert!(state.editor_buffer().is_dirty());
}

#[test]
fn command_e_opens_file_at_workspace_relative_path() {
    let root = temp_dir("cmd-e-relative");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    // Editor starts untitled (clean) → `:e a.txt` swaps in place.
    let (_root_dummy, mut state) = empty_state("cmd-e-dummy");
    state.workspace_root = root.clone();

    assert!(!handle_command_line(&mut state, "e a.txt"));
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
    assert_eq!(state.editor_buffer().content_as_string(), "alpha");
    assert!(state
        .status
        .as_deref()
        .is_some_and(|s| s.starts_with("Opened")));
}

#[test]
fn command_e_with_no_arg_shows_usage() {
    let (_root, mut state) = empty_state("cmd-e-noarg");
    assert!(!handle_command_line(&mut state, "e"));
    assert_eq!(state.status.as_deref(), Some("Usage: :e <path>"));
}

#[test]
fn command_e_with_missing_file_reports_error() {
    let (_root, mut state) = empty_state("cmd-e-missing");
    assert!(!handle_command_line(&mut state, "e does-not-exist.txt"));
    assert!(state
        .status
        .as_deref()
        .is_some_and(|s| s.starts_with("Open failed")));
    // Active buffer was NOT replaced — the load failed before any state
    // mutation could happen.
    assert!(state.editor_buffer().path().is_none());
}

#[test]
fn command_edit_is_alias_for_e() {
    let root = temp_dir("cmd-edit-alias");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    let (_root_dummy, mut state) = empty_state("cmd-edit-dummy");
    state.workspace_root = root.clone();

    assert!(!handle_command_line(&mut state, "edit a.txt"));
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
}

#[test]
fn command_e_preserves_filename_case() {
    // Filesystems may be case-sensitive. Tokens are lowercased for
    // dispatch, but the path arg must be extracted from the case-
    // preserving raw input so `:e Foo.RS` opens `Foo.RS`, not `foo.rs`.
    let root = temp_dir("cmd-e-case");
    let file = root.join("MixedCase.TXT");
    fs::write(&file, "mixed").unwrap();

    let (_root_dummy, mut state) = empty_state("cmd-e-case-dummy");
    state.workspace_root = root.clone();

    assert!(!handle_command_line(&mut state, "e MixedCase.TXT"));
    assert_eq!(state.editor_buffer().path(), Some(&file));
}

#[test]
fn command_x_is_alias_for_wq() {
    // vim parity: `:x` and `:wq` are the same command.
    let root = temp_dir("cmd-x-alias");
    let file_a = root.join("a.txt");
    fs::write(&file_a, "alpha").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.launched_with_file_path = true;
    state.editor_buffer_mut().insert_char('!');

    assert!(handle_command_line(&mut state, "x"));
    assert_eq!(fs::read_to_string(&file_a).unwrap(), "!alpha");
}
