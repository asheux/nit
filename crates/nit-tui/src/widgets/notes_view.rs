use nit_core::{Mode, PaneId};
use ratatui::Frame;

use crate::{
    theme::Theme,
    widgets::editor_view::{render_buffer, CursorPlacement},
};

pub fn render_notes(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    buffer: &nit_core::Buffer,
    focus: PaneId,
    _mode: Mode,
    theme: &Theme,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        focus,
        "NOTES  [ SCRATCH ]",
        theme,
        true,
    )
}
