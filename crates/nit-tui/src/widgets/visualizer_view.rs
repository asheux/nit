use nit_core::{AppState, PaneId, SeedEncoderId};
use nit_core::seed::SeedViewMode;
use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders},
    Frame,
};

use crate::{
    seed_render::{render_genome, render_seed, SeedPalette, SeedRenderConfig},
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
    if let Some(seed) = seed_runtime.encoded() {
        match state.visualizer.seed_view {
            SeedViewMode::Genome => {
                render_genome(
                    render_area,
                    frame.buffer_mut(),
                    seed,
                    seed_runtime.render_cache(),
                    &palette,
                );
            }
            SeedViewMode::Plate => {
                let cfg = SeedRenderConfig {
                    mode: state.visualizer.seed_plate_mode,
                    show_grid: state.visualizer.seed_show_grid,
                    show_bbox: state.visualizer.seed_show_bbox,
                    show_halo: state.visualizer.seed_show_halo,
                    show_components: state.visualizer.seed_show_components
                        || state.visualizer.seed_plate_mode == nit_core::SeedPreviewMode::Tissue,
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
            SeedViewMode::Map => {
                render_map(frame.buffer_mut(), render_area, seed, &palette);
            }
            SeedViewMode::Stats => {
                render_stats(frame.buffer_mut(), render_area, state, seed_runtime, &palette);
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
    let view = view_label(state);
    let overlays = overlay_label(state, seed_runtime);
    let mut out = String::with_capacity(128);
    push_seg(&mut out, &format!("ENC:{enc}"));
    push_seg(&mut out, &format!("SeedHash:{seed_hash}"));
    if seed_runtime.encoded().is_some() {
        push_seg(&mut out, &format!("Density:{density:.2}"));
        push_seg(&mut out, &format!("Comp:{components}"));
    }
    push_seg(&mut out, &format!("VIEW:{view}"));
    push_seg(&mut out, &format!("OVR:{overlays}"));
    out
}

fn push_seg(buf: &mut String, segment: &str) {
    if !buf.is_empty() {
        buf.push_str(" | ");
    }
    buf.push_str(segment);
}

fn view_label(state: &AppState) -> String {
    match state.visualizer.seed_view {
        SeedViewMode::Genome => "GENOME".to_string(),
        SeedViewMode::Plate => {
            format!("PLATE/{}", state.visualizer.seed_plate_mode.label())
        }
        SeedViewMode::Map => "MAP".to_string(),
        SeedViewMode::Stats => "STATS".to_string(),
    }
}

fn overlay_label(state: &AppState, seed_runtime: &SeedRuntime) -> String {
    match state.visualizer.seed_view {
        SeedViewMode::Genome => {
            if seed_runtime.encoded().is_none() {
                return "OFF".into();
            }
            match state.visualizer.seed_encoder {
                SeedEncoderId::Lifehash16 => {
                    if state.visualizer.seed_params.symmetry != nit_core::SeedSymmetry::None {
                        "SYM".into()
                    } else {
                        "OFF".into()
                    }
                }
                SeedEncoderId::HilbertBits => "PATH".into(),
                SeedEncoderId::AsciiBytes => "BYTE".into(),
            }
        }
        SeedViewMode::Plate => {
            let mut parts = Vec::new();
            if state.visualizer.seed_show_grid {
                parts.push("GRID");
            }
            if state.visualizer.seed_show_halo {
                parts.push("HALO");
            }
            if state.visualizer.seed_show_components
                || state.visualizer.seed_plate_mode == nit_core::SeedPreviewMode::Tissue
            {
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
        SeedViewMode::Map | SeedViewMode::Stats => "OFF".into(),
    }
}

fn draw_header(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_x = area.x.saturating_add(area.width);
    let mut x = area.x;
    let style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::DIM);
    let text_len = text.chars().count();
    let width = area.width as usize;
    let use_ellipsis = text_len > width && width > 0;
    let max_chars = if use_ellipsis && width > 0 {
        width.saturating_sub(1)
    } else {
        width
    };
    for ch in text.chars().take(max_chars) {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, area.y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    if use_ellipsis && x < max_x {
        let cell = buf.get_mut(x, area.y);
        cell.set_char('…');
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

fn render_map(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    seed: &nit_core::EncodedSeed,
    palette: &SeedPalette,
) {
    fill_bg(buf, area, palette.bg);
    let mut y = area.y;
    let max_y = area.y.saturating_add(area.height);
    let label_style = Style::default().fg(palette.hud_dim).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text);
    let heading_style = Style::default().fg(palette.accent).add_modifier(Modifier::BOLD);
    write_line(buf, area, y, "GENOME PROTOCOL", heading_style);
    y = y.saturating_add(1);
    let params = &seed.params;
    let encoder = seed.encoder_id.as_str();
    write_kv(buf, area, &mut y, "Encoder", encoder, label_style, value_style, max_y);
    write_kv(
        buf,
        area,
        &mut y,
        "Placement",
        params.placement.label(),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Padding",
        &format!("{}", params.padding),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Symmetry",
        params.symmetry.label(),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Target dens",
        &format!("{:.2}", params.target_density),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Jitter",
        &format!("{:.2}", params.jitter),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Genome",
        &format!("{}x{}", seed.base_bits.width(), seed.base_bits.height()),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Plate",
        &format!("{}x{}", seed.grid.width(), seed.grid.height()),
        label_style,
        value_style,
        max_y,
    );
}

fn render_stats(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    state: &AppState,
    seed_runtime: &SeedRuntime,
    palette: &SeedPalette,
) {
    fill_bg(buf, area, palette.bg);
    let mut y = area.y;
    let max_y = area.y.saturating_add(area.height);
    let label_style = Style::default().fg(palette.hud_dim).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text);
    let heading_style = Style::default().fg(palette.accent).add_modifier(Modifier::BOLD);
    write_line(buf, area, y, "GENOME STATS", heading_style);
    y = y.saturating_add(1);
    let seed_hash = if state.visualizer.seed_hash == 0 {
        "--".to_string()
    } else {
        format!("{:08x}", state.visualizer.seed_hash as u32)
    };
    let input_hash = if state.visualizer.input_hash == 0 {
        "--".to_string()
    } else {
        format!("{:08x}", state.visualizer.input_hash as u32)
    };
    write_kv(
        buf,
        area,
        &mut y,
        "Seed hash",
        &seed_hash,
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Input hash",
        &input_hash,
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Density",
        &format!("{:.2}", state.visualizer.seed_stats.density),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Components",
        &format!("{}", state.visualizer.seed_stats.components),
        label_style,
        value_style,
        max_y,
    );
    let cache = seed_runtime.render_cache();
    if cache.genome_total > 0 {
        write_kv(
            buf,
            area,
            &mut y,
            "Genome dens",
            &format!("{:.2}", cache.genome_density),
            label_style,
            value_style,
            max_y,
        );
        write_kv(
            buf,
            area,
            &mut y,
            "Genome live",
            &format!("{}/{}", cache.genome_live, cache.genome_total),
            label_style,
            value_style,
            max_y,
        );
    }
    write_kv(
        buf,
        area,
        &mut y,
        "ASCII ok",
        &format!("{}", cache.ascii_printable),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "ASCII ctrl",
        &format!("{}", cache.ascii_nonprintable),
        label_style,
        value_style,
        max_y,
    );
}

fn write_line(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    y: u16,
    text: &str,
    style: Style,
) {
    if y >= area.y.saturating_add(area.height) {
        return;
    }
    let mut x = area.x;
    let max_x = area.x.saturating_add(area.width);
    for ch in text.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
}

fn write_kv(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    y: &mut u16,
    label: &str,
    value: &str,
    label_style: Style,
    value_style: Style,
    max_y: u16,
) {
    if *y >= max_y {
        return;
    }
    let mut x = area.x;
    let max_x = area.x.saturating_add(area.width);
    for ch in label.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, *y);
        cell.set_char(ch);
        cell.set_style(label_style);
        x = x.saturating_add(1);
    }
    if x < max_x {
        let cell = buf.get_mut(x, *y);
        cell.set_char(':');
        cell.set_style(label_style);
        x = x.saturating_add(1);
    }
    if x < max_x {
        let cell = buf.get_mut(x, *y);
        cell.set_char(' ');
        cell.set_style(label_style);
        x = x.saturating_add(1);
    }
    for ch in value.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, *y);
        cell.set_char(ch);
        cell.set_style(value_style);
        x = x.saturating_add(1);
    }
    *y = y.saturating_add(1);
}
