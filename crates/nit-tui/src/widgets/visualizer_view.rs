use nit_core::{AppState, PaneId, SeedPreviewMode};
use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders},
    Frame,
};

use crate::{
    gol_render::{GolPalette, GolRenderConfig, GolWidget},
    seed_runtime::SeedRuntime,
    theme::Theme,
};

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    seed_runtime: &SeedRuntime,
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
            "VISUALIZER  [ APPLY ] [ SEED ] [ SNAP ] [ SEARCH ]",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let header = build_seed_hud(state, seed_runtime);
    match state.visualizer.seed_preview {
        SeedPreviewMode::BitGrid => {
            if let Some(seed) = seed_runtime.encoded() {
                let cfg = GolRenderConfig {
                    mode: state.visualizer.render_mode,
                    age_shading: false,
                    trails: false,
                    overlay_bbox: false,
                    overlay_heat: false,
                    scanlines: false,
                    braille_enabled: state.settings.gol.braille_enabled,
                };
                let widget = GolWidget {
                    grid: &seed.grid,
                    state: seed_runtime.render_state(),
                    cfg,
                    palette,
                    hud: crate::gol_render::GolHudState {
                        rule: "",
                        generation: 0,
                        alive: 0,
                        period: None,
                        mode: state.visualizer.mode,
                        paused: true,
                        delta: 0,
                        history: seed_runtime.render_state().hud_metrics().history(),
                    },
                };
                frame.render_widget(widget, inner);
                draw_header(frame.buffer_mut(), inner, &header, palette.hud_text, palette.bg);
            } else {
                draw_header(frame.buffer_mut(), inner, &header, palette.hud_text, palette.bg);
            }
        }
        SeedPreviewMode::Matrix => {
            render_matrix(frame.buffer_mut(), inner, seed_runtime, palette, &header);
        }
        SeedPreviewMode::Motif => {
            render_motif(frame.buffer_mut(), inner, seed_runtime, palette, &header);
        }
    }
}

fn build_seed_hud(state: &AppState, seed_runtime: &SeedRuntime) -> String {
    let seed_hash = if state.visualizer.seed_hash == 0 {
        "--".to_string()
    } else {
        format!("{:08x}", state.visualizer.seed_hash as u32)
    };
    let density = state.visualizer.seed_stats.density;
    let components = state.visualizer.seed_stats.components;
    let enc = state.visualizer.seed_encoder.as_str();
    let view = state.visualizer.seed_preview.label();
    if seed_runtime.encoded().is_some() {
        format!(
            "ENC:{enc} | SeedHash:{seed_hash} | Density:{density:.2} | Comp:{components} | View:{view}"
        )
    } else {
        format!("ENC:{enc} | SeedHash:{seed_hash} | View:{view}")
    }
}

fn draw_header(buf: &mut Buffer, area: ratatui::layout::Rect, text: &str, fg: ratatui::style::Color, bg: ratatui::style::Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_x = area.x.saturating_add(area.width);
    let mut x = area.x;
    let style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::DIM);
    for ch in text.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, area.y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    while x < max_x {
        let cell = buf.get_mut(x, area.y);
        cell.set_char(' ');
        cell.set_style(style);
        x = x.saturating_add(1);
    }
}

fn render_matrix(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    seed_runtime: &SeedRuntime,
    palette: GolPalette,
    header: &str,
) {
    fill_bg(buf, area, palette.bg);
    draw_header(buf, area, header, palette.hud_text, palette.bg);
    let Some(seed) = seed_runtime.encoded() else {
        return;
    };
    let content = ratatui::layout::Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let w = seed.base_bits.width();
    let h = seed.base_bits.height();
    let max_cols = content.width as usize;
    let max_rows = content.height as usize;
    let x0 = content.x + content.width.saturating_sub(w as u16).min(content.width) / 2;
    let y0 = content.y + content.height.saturating_sub(h as u16).min(content.height) / 2;
    let rows = h.min(max_rows);
    let cols = w.min(max_cols);
    for y in 0..rows {
        for x in 0..cols {
            let ch = if seed.base_bits.get(x, y) { '1' } else { '0' };
            let cell = buf.get_mut(x0 + x as u16, y0 + y as u16);
            cell.set_char(ch);
            cell.set_fg(palette.hud_text);
            cell.set_bg(palette.bg);
        }
    }
}

fn render_motif(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    seed_runtime: &SeedRuntime,
    palette: GolPalette,
    header: &str,
) {
    fill_bg(buf, area, palette.bg);
    draw_header(buf, area, header, palette.hud_text, palette.bg);
    let Some(seed) = seed_runtime.encoded() else {
        return;
    };
    let content = ratatui::layout::Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let motifs = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    let w = seed.base_values.width();
    let h = seed.base_values.height();
    let max_cols = content.width as usize;
    let max_rows = content.height as usize;
    let x0 = content.x + content.width.saturating_sub(w as u16).min(content.width) / 2;
    let y0 = content.y + content.height.saturating_sub(h as u16).min(content.height) / 2;
    let rows = h.min(max_rows);
    let cols = w.min(max_cols);
    for y in 0..rows {
        for x in 0..cols {
            let value = seed.base_values.get(x, y) as usize;
            let idx = value.saturating_mul(motifs.len() - 1) / 255;
            let ch = motifs[idx];
            let cell = buf.get_mut(x0 + x as u16, y0 + y as u16);
            cell.set_char(ch);
            cell.set_fg(palette.hud_text);
            cell.set_bg(palette.bg);
        }
    }
}

fn fill_bg(buf: &mut Buffer, area: ratatui::layout::Rect, bg: ratatui::style::Color) {
    let max_x = area.x.saturating_add(area.width);
    let max_y = area.y.saturating_add(area.height);
    let mut y = area.y;
    while y < max_y {
        let mut x = area.x;
        while x < max_x {
            let cell = buf.get_mut(x, y);
            cell.set_char(' ');
            cell.set_bg(bg);
            x = x.saturating_add(1);
        }
        y = y.saturating_add(1);
    }
}
