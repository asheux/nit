use nit_core::{AppState, PaneId};
use ratatui::{
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders},
    Frame,
};

use crate::{
    gol_render::{AsciiSeedWidget, GolHudState, GolPalette, GolRenderConfig, GolWidget},
    theme::Theme,
    visualizer::VisualizerRuntime,
};

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    visualizer: &VisualizerRuntime,
) {
    let focused = state.focus == PaneId::Visualizer;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };

    let palette = GolPalette::from_theme(theme);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(palette.bg))
        .title(Span::styled(
            visualizer.title_text(),
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if !state.visualizer.running {
        let buffer = match state.visualizer.seed_source {
            nit_core::GolSeedSource::Editor => state.editor_buffer(),
            nit_core::GolSeedSource::Notes => state.notes_buffer(),
        };
        let header = match state.visualizer.seed_source {
            nit_core::GolSeedSource::Editor => {
                "ASCII SEED | Ctrl+Enter RUN | Ctrl+E ASCII | Source: EDITOR"
            }
            nit_core::GolSeedSource::Notes => {
                "ASCII SEED | Ctrl+Enter RUN | Ctrl+E ASCII | Source: NOTES"
            }
        };
        let widget = AsciiSeedWidget {
            buffer,
            palette,
            header,
        };
        frame.render_widget(widget, inner);
        return;
    }

    let Some(grid) = visualizer.grid() else {
        return;
    };

    let hud_metrics = visualizer.render_state().hud_metrics();
    let hud = GolHudState {
        rule: &state.visualizer.rule,
        generation: state.visualizer.generation,
        alive: state.visualizer.alive,
        period: state.visualizer.period,
        mode: state.visualizer.mode,
        paused: state.visualizer.paused,
        delta: hud_metrics.delta(),
        history: hud_metrics.history(),
    };

    let cfg = GolRenderConfig {
        mode: state.visualizer.render_mode,
        age_shading: state.visualizer.age_shading,
        trails: state.visualizer.trails,
        overlay_bbox: state.visualizer.overlay_bbox,
        overlay_heat: state.visualizer.overlay_heat,
        scanlines: state.visualizer.scanlines,
        braille_enabled: state.settings.gol.braille_enabled,
    };

    let widget = GolWidget {
        grid,
        state: visualizer.render_state(),
        cfg,
        palette,
        hud,
    };
    frame.render_widget(widget, inner);
}
