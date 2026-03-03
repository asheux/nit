use nit_core::{Mode, PaneId};
use nit_syntax::HighlightSnapshot;
use ratatui::Frame;

use crate::{
    theme::Theme,
    widgets::editor_view::{render_buffer, CursorPlacement},
};

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
        "NOTES  [ SCRATCH ]",
        theme,
        tab_width,
        true,
        mode,
    )
}
