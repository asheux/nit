use nit_core::{AppState, PaneId, SeedPreviewMode};
use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders},
    Frame,
};

use crate::{
    seed_render::{render_seed, SeedPalette, SeedRenderConfig},
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

    let palette = SeedPalette::from_theme(theme);

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
    let render_area = if inner.height > 1 {
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height - 1,
        }
    } else {
        inner
    };
    fill_bg(frame.buffer_mut(), render_area, palette.bg);
    match state.visualizer.seed_preview {
        SeedPreviewMode::Matrix => {
            render_matrix(frame.buffer_mut(), render_area, seed_runtime, &palette);
        }
        SeedPreviewMode::Motif => {
            render_motif(frame.buffer_mut(), render_area, seed_runtime, &palette);
        }
        _ => {
            if let Some(seed) = seed_runtime.encoded() {
                let cfg = SeedRenderConfig {
                    mode: state.visualizer.seed_preview,
                    show_grid: state.visualizer.seed_show_grid,
                    show_bbox: state.visualizer.seed_show_bbox,
                    show_halo: state.visualizer.seed_show_halo,
                    show_components: state.visualizer.seed_show_components
                        || state.visualizer.seed_preview == SeedPreviewMode::Tissue,
                    show_inset_genome: state.visualizer.seed_show_inset,
                    scanline: state.visualizer.seed_scanline,
                    zoom: state.visualizer.seed_zoom,
                };
                render_seed(
                    render_area,
                    frame.buffer_mut(),
                    seed,
                    &cfg,
                    seed_runtime.render_cache(),
                    &palette,
                );
            }
        }
    }
    draw_header(
        frame.buffer_mut(),
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
        &header,
        palette.hud_text,
        palette.bg,
    );
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
    let overlays = overlay_label(state);
    if seed_runtime.encoded().is_some() {
        format!(
            "ENC:{enc} | SeedHash:{seed_hash} | Density:{density:.2} | Comp:{components} | View:{view} | OVR:{overlays}"
        )
    } else {
        format!("ENC:{enc} | SeedHash:{seed_hash} | View:{view} | OVR:{overlays}")
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
    palette: &SeedPalette,
) {
    fill_bg(buf, area, palette.bg);
    let Some(seed) = seed_runtime.encoded() else {
        return;
    };
    let w = seed.base_bits.width();
    let h = seed.base_bits.height();
    let max_cols = area.width as usize;
    let max_rows = area.height as usize;
    let x0 = area.x + area.width.saturating_sub(w as u16).min(area.width) / 2;
    let y0 = area.y + area.height.saturating_sub(h as u16).min(area.height) / 2;
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
    palette: &SeedPalette,
) {
    fill_bg(buf, area, palette.bg);
    let Some(seed) = seed_runtime.encoded() else {
        return;
    };
    let motifs = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    let w = seed.base_values.width();
    let h = seed.base_values.height();
    let max_cols = area.width as usize;
    let max_rows = area.height as usize;
    let x0 = area.x + area.width.saturating_sub(w as u16).min(area.width) / 2;
    let y0 = area.y + area.height.saturating_sub(h as u16).min(area.height) / 2;
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

fn overlay_label(state: &AppState) -> String {
    let mut parts = Vec::new();
    if state.visualizer.seed_show_grid {
        parts.push("GRID");
    }
    if state.visualizer.seed_show_halo {
        parts.push("HALO");
    }
    if state.visualizer.seed_show_components {
        parts.push("COMP");
    }
    if state.visualizer.seed_show_bbox {
        parts.push("BBOX");
    }
    if state.visualizer.seed_show_inset {
        parts.push("INSET");
    }
    if parts.is_empty() {
        "OFF".into()
    } else {
        parts.join("+")
    }
}
