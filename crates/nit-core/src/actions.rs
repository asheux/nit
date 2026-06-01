use crate::{mode::Mode, pane::PaneId, search::SearchMode};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Quit,
    Save,
    SaveAndNormal,
    ConfirmQuitYes,
    ConfirmQuitNo,
    /// Discard dirty changes and close the active editor buffer
    /// (switching to the last remaining buffer or opening NITTree).
    /// Driven by `Prompt::ConfirmCloseBuffer`, which fires when `:q` is
    /// run on a dirty buffer in directory-launch mode.
    ConfirmCloseBufferYes,
    /// Dismiss the `ConfirmCloseBuffer` prompt without closing.
    ConfirmCloseBufferNo,
    FocusNextPane,
    FocusPrevPane,
    FocusPane(PaneId),
    SwitchMode(Mode),
    ToggleMode,
    InsertChar(char),
    InsertNewline,
    InsertTab,
    Append,
    Backspace,
    Delete,
    DeleteLine,
    YankLine,
    EnterVisual,
    ExitVisual,
    YankSelection,
    DeleteSelection,
    Paste,
    PasteLineAbove,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    PageUp,
    PageDown,
    Home,
    End,
    MoveWordEnd,
    MoveWordBack,
    GoToTop,
    GoToBottom,
    OpenLineBelow,
    OpenLineAbove,
    Undo,
    Redo,
    ScrollUp,
    ScrollDown,
    ClearLogs,
    ToggleJobPause,
    CommandPromptOpen,
    CommandPromptCancel,
    CommandPromptBackspace,
    CommandPromptMoveLeft,
    CommandPromptMoveRight,
    CommandPromptExecute,
    CommandPromptInput(char),
    VisualizerReseed,
    VisualizerApply,
    VisualizerToggleSearch,
    VisualizerToggleWrap,
    VisualizerToggleSeedSource,
    VisualizerSnapshot,
    VisualizerPause,
    VisualizerCycleAutoStop,
    VisualizerSpeedUp,
    VisualizerSpeedDown,
    VisualizerRun,
    VisualizerStop,
    GamesRun,
    GamesStop,
    GamesHide,
    GamesShow,
    GamesHistoryOpen,
    VisualizerCycleRenderMode,
    VisualizerToggleAgeShading,
    VisualizerToggleTrails,
    VisualizerToggleBBox,
    VisualizerToggleHeat,
    VisualizerToggleScanlines,
    VisualizerCycleSeedView,
    VisualizerCycleEncoder,
    VisualizerCycleSeedOverlays,
    VisualizerCycleSymmetry,
    VisualizerToggleSeedView,
    VisualizerInspectLeft,
    VisualizerInspectRight,
    VisualizerInspectUp,
    VisualizerInspectDown,
    VisualizerInspectHome,
    VisualizerInspectEnd,
    VisualizerInspectCenter,
    VisualizerInspectToggle,
    VisualizerInspectJump(u64),
    GateMonitorToggleSubView,
    GateMonitorSetSubView(crate::state::GateMonitorSubView),
    /// Operator clicked the EVALUATE button — kick off a full workspace
    /// genome scan. The runner picks up `agents.workspace_scan_requested`
    /// on the next tick and calls `WorkspaceScanRuntime::rescan`.
    WorkspaceScanStart,
    ShowSubstrate,
    HideSubstrate,
    SubstrateOverlayToggleTab,
    SetGolRuleById(String),
    SetGolRuleByString(String),
    OpenRulePicker,
    OpenProtocolPicker,
    CloseModal,
    ApplySelectedRuleFromPicker,
    ApplySelectedProtocolFromPicker,
    PetriShow,
    ShowHelp,
    HideHelp,
    ToggleSyntax,
    ToggleDebug,
    ToggleFileTree,
    OpenFile(PathBuf),
    OpenSearchPopup(SearchMode),
    CloseSearchPopup,
    // --- Vim-style editor motions ---
    MoveWordForward,
    MoveBigWordForward,
    MoveBigWordBack,
    MoveBigWordEnd,
    MoveFirstNonBlank,
    MoveLastNonBlank,
    MoveParagraphUp,
    MoveParagraphDown,
    MoveViewportTop,
    MoveViewportMiddle,
    MoveViewportBottom,
    // --- Vim-style editor operators (no explicit motion pairing) ---
    DeleteToEnd,
    ChangeToEnd,
    /// `cw` / `ce` — change to end of word, then enter Insert mode.
    /// Both `cw` and `ce` route here per vim's `cw == ce` quirk.
    ChangeWordEnd,
    /// `cW` / `cE` — same as `ChangeWordEnd` but using WORD boundaries.
    ChangeBigWordEnd,
    /// `cb` — change backward to start of word, then enter Insert mode.
    ChangeWordBack,
    /// `cB` — change backward to start of WORD, then enter Insert mode.
    ChangeBigWordBack,
    /// `cc` — change entire line (preserve indent), then enter Insert mode.
    ChangeLine,
    SubstituteChar,
    SubstituteLine,
    JoinLines,
    ToggleCaseChar,
    ReplaceChar(char),
    /// Find a character on the current line.
    /// Fields: (ch, forward, till)
    ///   forward=true  → f / t
    ///   forward=false → F / T
    ///   till=true     → stop one character before the target (t / T)
    FindChar(char, bool, bool),
    // --- Vim-style scroll / viewport ---
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    CenterViewportOnCursor,
    ViewportTopOnCursor,
    ViewportBottomOnCursor,
    // --- Vim-style in-editor word search: * / # / n / N ---
    /// `*`: set search term to word under cursor and jump to next match.
    SearchWordForward,
    /// `#`: set search term to word under cursor and jump to previous match.
    SearchWordBack,
    /// `n`: jump to next occurrence of the active search term.
    SearchNext,
    /// `N`: jump to previous occurrence of the active search term.
    SearchPrev,
    /// Clear the highlighted search term (vim's `:nohlsearch`).
    SearchClear,
    // --- `/` search prompt ---
    SearchPromptOpen,
    SearchPromptCancel,
    SearchPromptExecute,
    SearchPromptInput(char),
    SearchPromptBackspace,
    // --- Vim numeric prefix ---
    /// Append a digit (0-9) to the count prefix buffered in
    /// `state.pending_count`. `5` then `6` then `j` runs MoveDown 56 times;
    /// any non-digit non-motion action clears the count.
    AppendCountDigit(u8),
    // --- Vim word-delete operators (T2) ---
    /// `dw`: delete from cursor to start of next word/class run.
    DeleteWordForward,
    /// `de`: delete to end of current/next word (inclusive).
    DeleteWordEnd,
    /// `db`: delete back to the start of the previous word/class run.
    DeleteWordBack,
    /// `dW`: WORD-aware forward delete (whitespace-separated runs).
    DeleteBigWordForward,
    /// `dE`: WORD-aware delete to end of run.
    DeleteBigWordEnd,
    /// `dB`: WORD-aware backward delete.
    DeleteBigWordBack,
    // --- Jumplist navigation (T5) ---
    /// `Ctrl-O`: pop the previous jumplist position.
    JumpBack,
    /// `Ctrl-I`: advance to the next jumplist position.
    JumpForward,
    /// `%`: jump to the bracket matching the one under the cursor. Covers
    /// `()`, `[]`, `{}`, and `<>`. Treated as a jump so `Ctrl-O` walks back.
    MatchBracket,
    /// Visual-mode `>` / normal-mode `>>`: prepend one indent unit to
    /// every line touched by the active selection (or the cursor's line
    /// when nothing is selected). One undo step for the whole block.
    IndentSelection,
    /// Visual-mode `<` / normal-mode `<<`: strip up to one indent unit of
    /// leading whitespace from each line in the block. One undo step.
    DedentSelection,
    /// Visual-mode `U`: uppercase the selection with Unicode-correct folding, one undo group.
    UppercaseSelection,
    /// Visual-mode `u`: lowercase the selection with Unicode-correct folding, one undo group.
    LowercaseSelection,
    /// `gd`: open the same-file goto-definition popup for the identifier under
    /// the cursor. v1 resolves heuristically (no project index) — see
    /// `state::find_definition_line`.
    GotoDefinition,
    /// `Ctrl+\`: swap the chat pane for an OS-shell terminal (per-pane in
    /// multipane). Flips a render flag; the TUI event loop spawns/kills the
    /// PTY by reconciling it, since nit-core owns no subprocess.
    ToggleTerminalPane,
    /// `Ctrl+Shift+T`: toggle the modal terminal popup. Records the intent;
    /// the TUI event loop pins the cwd and reconciles the persistent PTY
    /// (close hides, quit kills) since nit-core owns no subprocess.
    ToggleTerminalPopup,
}
