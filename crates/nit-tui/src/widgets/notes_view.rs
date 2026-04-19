use nit_core::{Mode, PaneId};
use nit_syntax::HighlightSnapshot;
use ratatui::Frame;

use crate::{
    theme::Theme,
    widgets::editor_view::{render_buffer, CursorPlacement},
};

const NOTES_TITLE: &str = "NOTES  [ SCRATCH ]";
const SHOW_CURSOR: bool = true;

/// Render the Notes/Scratch pane by delegating to `editor_view::render_buffer`
/// with the fixed Notes title and `PaneId::Notes` focus token. Returns cursor
/// placement when the Notes pane owns focus so the caller can position the
/// terminal cursor.
#[allow(clippy::too_many_arguments)]
pub fn render_notes(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    buffer: &nit_core::Buffer,
    snapshot: Option<&HighlightSnapshot>,
    line_map: Option<&[Option<usize>]>,
    focus: PaneId,
    mode: Mode,
    theme: &Theme,
    tab_width: usize,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        snapshot,
        line_map,
        PaneId::Notes,
        focus,
        NOTES_TITLE,
        theme,
        tab_width,
        SHOW_CURSOR,
        mode,
        None,
    )
}
