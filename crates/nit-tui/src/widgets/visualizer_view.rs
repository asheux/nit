use nit_core::seed::SeedViewMode;
use nit_core::{Action, AppState, PaneId, SeedEncoderId, VisualizerSubView};
use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders},
    Frame,
};

use crate::{
    seed_render::{ascii_layout, render_genome, render_seed, SeedPalette, SeedRenderConfig},
    seed_runtime::SeedRuntime,
    theme::Theme,
};

// Tab-bar-in-title labels (rendered with a leading space separator, same as
// gate_monitor_view.rs).
const TITLE_PREFIX: &str = "VISUALIZER ";
const BTN_SIGNALS_LABEL: &str = " SUBSTRATE SIGNALS ";
const BTN_VIZ_LABEL: &str = " VISUALIZER ";
// APPLY/SEED/SNAP/SEARCH only render on the Visualizer sub-view.
const BTN_APPLY_LABEL: &str = " APPLY ";
const BTN_SEED_LABEL: &str = " SEED ";
const BTN_SNAP_LABEL: &str = " SNAP ";
const BTN_SEARCH_LABEL: &str = " SEARCH ";

/// Returns an action if the click column (relative to the visualizer rect)
/// hits a title button. `sub_view` gates the inner buttons (APPLY/SEED/…) so
/// they are unresponsive on the Signals tab where they aren't rendered.
pub fn title_button_hit(col_in_rect: u16, sub_view: VisualizerSubView) -> Option<Action> {
    // Title text starts 1 cell in from the left border.
    let col = col_in_rect.saturating_sub(1);
    let prefix_len = TITLE_PREFIX.len() as u16;

    // Tab buttons: [" SUBSTRATE SIGNALS "] [space] [" VISUALIZER "]
    let sig_start = prefix_len + 1; // single-space separator after prefix
    let sig_end = sig_start + BTN_SIGNALS_LABEL.len() as u16;
    let viz_start = sig_end + 1;
    let viz_end = viz_start + BTN_VIZ_LABEL.len() as u16;
    if (sig_start..sig_end).contains(&col) || (viz_start..viz_end).contains(&col) {
        return Some(Action::VisualizerToggleSubView);
    }

    // Inner action buttons only exist on the Visualizer tab.
    if sub_view != VisualizerSubView::Visualizer {
        return None;
    }
    let apply_start = viz_end + 1;
    let apply_end = apply_start + BTN_APPLY_LABEL.len() as u16;
    let seed_start = apply_end + 1;
    let seed_end = seed_start + BTN_SEED_LABEL.len() as u16;
    let snap_start = seed_end + 1;
    let snap_end = snap_start + BTN_SNAP_LABEL.len() as u16;
    let search_start = snap_end + 1;
    let search_end = search_start + BTN_SEARCH_LABEL.len() as u16;
    if (apply_start..apply_end).contains(&col) {
        Some(Action::VisualizerApply)
    } else if (seed_start..seed_end).contains(&col) {
        Some(Action::VisualizerCycleSymmetry)
    } else if (snap_start..snap_end).contains(&col) {
        Some(Action::VisualizerSnapshot)
    } else if (search_start..search_end).contains(&col) {
        Some(Action::VisualizerToggleSearch)
    } else {
        None
    }
}

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
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

    let title_style = Style::default()
        .fg(title_color)
        .add_modifier(Modifier::BOLD);
    let btn_active = Style::default()
        .fg(theme.background)
        .bg(title_color)
        .add_modifier(Modifier::BOLD);
    let btn_inactive = Style::default().fg(title_color).add_modifier(Modifier::DIM);
    let sep_style = Style::default().fg(title_color);

    let sub_view = state.visualizer_sub_view;
    let is_signals = sub_view == VisualizerSubView::SubstrateSignals;
    let signals_style = if is_signals { btn_active } else { btn_inactive };
    let viz_style = if is_signals { btn_inactive } else { btn_active };

    let mut title_spans: Vec<Span<'static>> = vec![
        Span::styled(TITLE_PREFIX, title_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_SIGNALS_LABEL, signals_style),
        Span::styled(" ", sep_style),
        Span::styled(BTN_VIZ_LABEL, viz_style),
    ];
    // APPLY/SEED/SNAP/SEARCH only appear on the Visualizer tab.
    if !is_signals {
        title_spans.extend([
            Span::styled(" ", sep_style),
            Span::styled(BTN_APPLY_LABEL, btn_active),
            Span::styled(" ", sep_style),
            Span::styled(BTN_SEED_LABEL, btn_active),
            Span::styled(" ", sep_style),
            Span::styled(BTN_SNAP_LABEL, btn_active),
            Span::styled(" ", sep_style),
            Span::styled(BTN_SEARCH_LABEL, btn_active),
        ]);
    }
    let title = Line::from(title_spans);

    // Signals tab uses the ordinary pane background; visualizer tab uses the
    // seed palette background for the plate/genome canvas.
    let body_bg = if is_signals { theme.background } else { palette.bg };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(body_bg))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if is_signals {
        crate::widgets::signals_view::render_body(frame, inner, state, theme);
        return;
    }

    let hud_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    draw_hud_line(frame.buffer_mut(), hud_area, state, seed_runtime, &palette);

    let mut top = inner.y.saturating_add(1);
    let legend_area = if inner.height >= 4 && state.visualizer.seed_view == SeedViewMode::Genome {
        let area = ratatui::layout::Rect {
            x: inner.x,
            y: top,
            width: inner.width,
            height: 1,
        };
        top = top.saturating_add(1);
        Some(area)
    } else {
        None
    };

    let show_inspector = state.visualizer.seed_view == SeedViewMode::Genome
        && seed_runtime.encoded().is_some()
        && inner.height >= 3;
    let bottom_reserved = if show_inspector { 1 } else { 0 };
    let used_top = top.saturating_sub(inner.y);
    let render_height = inner
        .height
        .saturating_sub(used_top)
        .saturating_sub(bottom_reserved);
    let render_area = ratatui::layout::Rect {
        x: inner.x,
        y: top,
        width: inner.width,
        height: render_height,
    };

    if let Some(legend_area) = legend_area {
        draw_legend_line(
            frame.buffer_mut(),
            legend_area,
            seed_runtime,
            render_area,
            &palette,
        );
    }

    fill_bg(frame.buffer_mut(), render_area, palette.bg);

    if state.editor_buffer().path().is_none() {
        draw_genome_placeholder(frame.buffer_mut(), render_area, &palette);
        return;
    }

    let Some(seed) = seed_runtime.encoded() else {
        draw_loading_bar(frame, render_area, &palette);
        return;
    };
    match state.visualizer.seed_view {
        SeedViewMode::Genome => {
            render_genome(
                render_area,
                frame.buffer_mut(),
                seed,
                seed_runtime.render_cache(),
                &palette,
            );
            if focused && state.visualizer.inspector_enabled {
                draw_genome_crosshair(
                    frame.buffer_mut(),
                    render_area,
                    state,
                    seed,
                    seed_runtime,
                    &palette,
                );
            }
        }
        SeedViewMode::Plate => {
            let cfg = SeedRenderConfig {
                mode: state.visualizer.seed_plate_mode,
                show_grid: false,
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
            render_stats(
                frame.buffer_mut(),
                render_area,
                state,
                seed_runtime,
                &palette,
            );
        }
    }

    if show_inspector {
        let inspector_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        draw_inspector_line(
            frame.buffer_mut(),
            inspector_area,
            state,
            seed_runtime,
            &palette,
        );
    }
}

fn draw_hud_line(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    state: &AppState,
    seed_runtime: &SeedRuntime,
    palette: &SeedPalette,
) {
    let style = Style::default()
        .fg(palette.hud_text)
        .bg(palette.bg)
        .add_modifier(Modifier::DIM);
    let mut writer = LineWriter::new(buf, area, style);
    writer.write_str("SeedHash:");
    if state.visualizer.seed_hash == 0 {
        writer.write_str("--");
    } else {
        writer.write_hex_u32(state.visualizer.seed_hash as u32);
    }
    if seed_runtime.encoded().is_some() {
        let cache = seed_runtime.render_cache();
        writer.write_sep();
        writer.write_str("DenG:");
        writer.write_f32_2(cache.genome_density);
        writer.write_sep();
        writer.write_str("DenP:");
        writer.write_f32_2(state.visualizer.seed_stats.density);
        writer.write_sep();
        writer.write_str("CompP:");
        writer.write_u32(state.visualizer.seed_stats.components as u32);
    }
    writer.write_sep();
    writer.write_str("VIEW:");
    write_view_label(&mut writer, state);
    writer.finish();
}

fn draw_loading_bar(frame: &mut Frame, area: ratatui::layout::Rect, palette: &SeedPalette) {
    if area.width < 6 || area.height < 2 {
        return;
    }
    let bar_w = area.width / 3;
    let x = area.x + (area.width.saturating_sub(bar_w)) / 2;
    let y = area.y.saturating_add(area.height / 2);

    let label = "Genome loading";
    let label_y = y.saturating_sub(1);
    if label_y >= area.y {
        let label_x = x + bar_w.saturating_sub(label.len() as u16) / 2;
        let label_style = Style::default()
            .fg(palette.hud_text)
            .add_modifier(Modifier::DIM);
        let buf = frame.buffer_mut();
        for (i, ch) in label.chars().enumerate() {
            let cx = label_x + i as u16;
            if cx < x + bar_w {
                buf.get_mut(cx, label_y).set_char(ch).set_style(label_style);
            }
        }
    }

    let ratio = loading_ratio();
    let seg_w = (bar_w / 5).max(2);
    let travel = bar_w.saturating_sub(seg_w);
    let seg_x = x + (travel as f64 * ratio) as u16;

    let track_style = Style::default()
        .fg(palette.hud_text)
        .add_modifier(Modifier::DIM);
    let seg_style = Style::default()
        .fg(palette.hud_text)
        .add_modifier(Modifier::BOLD);
    let buf = frame.buffer_mut();
    for dx in 0..bar_w {
        let cx = x + dx;
        if cx >= seg_x && cx < seg_x + seg_w {
            buf.get_mut(cx, y).set_char('━').set_style(seg_style);
        } else {
            buf.get_mut(cx, y).set_char('─').set_style(track_style);
        }
    }
}

fn draw_genome_placeholder(buf: &mut Buffer, area: ratatui::layout::Rect, palette: &SeedPalette) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let msg = " Open file in editor to view code genome ";
    let msg_len = msg.len() as u16;
    let bar_w = msg_len;
    let bar_h = 3u16;
    let bar_x = area.x + area.width.saturating_sub(bar_w) / 2;
    let bar_y = area.y + area.height.saturating_sub(bar_h) / 2;

    let cyan = ratatui::style::Color::Rgb(0, 215, 215);
    let bar_style = Style::default().bg(cyan);
    let text_style = Style::default()
        .fg(palette.bg)
        .bg(cyan)
        .add_modifier(Modifier::BOLD);

    // Fill the 3-row bar with background color.
    for row in 0..bar_h {
        for dx in 0..bar_w {
            let cell = buf.get_mut(bar_x + dx, bar_y + row);
            cell.set_char(' ');
            cell.set_style(bar_style);
        }
    }
    // Draw centered text on the middle row.
    let text_y = bar_y + 1;
    for (i, ch) in msg.chars().enumerate() {
        let cx = bar_x + i as u16;
        if cx < bar_x + bar_w {
            let cell = buf.get_mut(cx, text_y);
            cell.set_char(ch);
            cell.set_style(text_style);
        }
    }
}

fn loading_ratio() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis() as f64;
    let period = 1600.0;
    let phase = (millis % period) / period;
    let tri = if phase <= 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    tri.clamp(0.0, 1.0)
}

fn draw_legend_line(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    seed_runtime: &SeedRuntime,
    render_area: ratatui::layout::Rect,
    palette: &SeedPalette,
) {
    let style = Style::default()
        .fg(palette.hud_dim)
        .bg(palette.bg)
        .add_modifier(Modifier::DIM);
    let mut writer = LineWriter::new(buf, area, style);
    let Some(seed) = seed_runtime.encoded() else {
        writer.finish();
        return;
    };
    match seed.encoder_id {
        SeedEncoderId::AsciiBytes => {
            writer.write_str("BYTE: dec 0-255 | sep=8 bytes | printable marked");
        }
        SeedEncoderId::Lifehash16 => {
            writer.write_str("16×16 base genome | SYM axis dotted | bit=1 violet");
        }
        SeedEncoderId::HilbertBits => {
            writer.write_str("bitstream tape | sep=64 bits | PATH inset shows traversal");
            if let Some(stream) = seed_runtime.render_cache().hilbert_stream.as_ref() {
                let cols = render_area.width as usize;
                if cols > 0 {
                    let total = stream.len().max(1);
                    let stride = total.div_ceil(cols);
                    if stride > 1 {
                        writer.write_sep();
                        writer.write_str("stride=");
                        writer.write_u32(stride as u32);
                    }
                }
            }
        }
        SeedEncoderId::Structural => {
            writer.write_str("structural 32×32 | Hilbert-curve layout | block boundaries");
            if let Some(stream) = seed_runtime.render_cache().hilbert_stream.as_ref() {
                let cols = render_area.width as usize;
                if cols > 0 {
                    let total = stream.len().max(1);
                    let stride = total.div_ceil(cols);
                    if stride > 1 {
                        writer.write_sep();
                        writer.write_str("stride=");
                        writer.write_u32(stride as u32);
                    }
                }
            }
        }
        SeedEncoderId::TokenSpectrum => {
            writer.write_str("token_spectrum 32×32 | AST token categories | Hilbert layout");
        }
        SeedEncoderId::AstStructure => {
            writer.write_str("ast_structure 32×32 | AST node properties | Hilbert layout");
        }
        SeedEncoderId::ComplexityField => {
            writer.write_str("complexity_field 32×32 | per-line metrics heatmap");
        }
    }
    writer.finish();
}

fn draw_inspector_line(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    state: &AppState,
    seed_runtime: &SeedRuntime,
    palette: &SeedPalette,
) {
    let style = Style::default()
        .fg(palette.hud_dim)
        .bg(palette.bg)
        .add_modifier(Modifier::DIM);
    let mut writer = LineWriter::new(buf, area, style);
    let Some(seed) = seed_runtime.encoded() else {
        writer.finish();
        return;
    };
    if !state.visualizer.inspector_enabled {
        writer.write_str("INSPECTOR OFF");
        writer.finish();
        return;
    }
    let (x, y) = clamp_inspector_pos(state, seed);
    let w = seed.base_bits.width().max(1);
    let idx = y.saturating_mul(w).saturating_add(x);
    match seed.encoder_id {
        SeedEncoderId::AsciiBytes => {
            let value = seed.base_values.get(x, y);
            let printable = (0x20..=0x7e).contains(&value);
            writer.write_str("IDX:");
            writer.write_u32(idx as u32);
            writer.write_sep();
            writer.write_str("XY:");
            writer.write_u32(x as u32);
            writer.write_char(',');
            writer.write_u32(y as u32);
            writer.write_sep();
            writer.write_str("HEX:");
            writer.write_hex_u8(value);
            writer.write_sep();
            writer.write_str("ASCII:");
            if printable {
                writer.write_char(value as char);
            } else {
                writer.write_char('.');
            }
            writer.write_sep();
            writer.write_str("CLASS:");
            if matches!(value, b' ' | b'\t' | b'\n' | b'\r') {
                writer.write_str("WS");
            } else if printable {
                writer.write_str("PRINT");
            } else {
                writer.write_str("CTRL");
            }
        }
        SeedEncoderId::Lifehash16 => {
            let bit = seed.base_bits.get(x, y);
            writer.write_str("IDX:");
            writer.write_u32(idx as u32);
            writer.write_sep();
            writer.write_str("XY:");
            writer.write_u32(x as u32);
            writer.write_char(',');
            writer.write_u32(y as u32);
            writer.write_sep();
            writer.write_str("BIT:");
            writer.write_char(if bit { '1' } else { '0' });
        }
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            let bit = seed.base_bits.get(x, y);
            writer.write_str("IDX:");
            if let Some(map) = seed_runtime.render_cache().hilbert_index_by_xy.as_ref() {
                let idx_map = map[y.saturating_mul(w) + x];
                writer.write_u32(idx_map);
            } else {
                writer.write_u32(idx as u32);
            }
            writer.write_sep();
            writer.write_str("XY:");
            writer.write_u32(x as u32);
            writer.write_char(',');
            writer.write_u32(y as u32);
            writer.write_sep();
            writer.write_str("BIT:");
            writer.write_char(if bit { '1' } else { '0' });
        }
        SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => {
            let value = seed.base_values.get(x, y);
            writer.write_str("IDX:");
            writer.write_u32(idx as u32);
            writer.write_sep();
            writer.write_str("XY:");
            writer.write_u32(x as u32);
            writer.write_char(',');
            writer.write_u32(y as u32);
            writer.write_sep();
            writer.write_str("VAL:");
            writer.write_u32(value as u32);
        }
    }
    writer.finish();
}

fn draw_genome_crosshair(
    buf: &mut Buffer,
    area: ratatui::layout::Rect,
    state: &AppState,
    seed: &nit_core::EncodedSeed,
    seed_runtime: &SeedRuntime,
    palette: &SeedPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (x, y) = clamp_inspector_pos(state, seed);
    match seed.encoder_id {
        SeedEncoderId::AsciiBytes => {
            let w = seed.base_values.width().max(1);
            let h = seed.base_values.height().max(1);
            let (digits, gap, cols) = ascii_layout(area.width as usize, w);
            if cols == 0 || digits == 0 {
                return;
            }
            let rows = area.height as usize;
            let cx = x.saturating_mul(cols) / w;
            let cy = y.saturating_mul(rows.max(1)) / h;
            let stride = digits + gap;
            let start_x = area.x + (cx * stride) as u16;
            let row_y = area.y + cy as u16;
            for dx in 0..digits {
                let cell = buf.get_mut(start_x + dx as u16, row_y);
                cell.set_bg(palette.accent);
            }
        }
        SeedEncoderId::Lifehash16 => {
            if let Some((sx, sy)) =
                map_to_screen(area, x, y, seed.base_bits.width(), seed.base_bits.height())
            {
                let cell = buf.get_mut(sx, sy);
                cell.set_char('+');
                cell.set_fg(palette.accent);
            }
        }
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            let w = seed.base_bits.width().max(1);
            let idx = if let Some(map) = seed_runtime.render_cache().hilbert_index_by_xy.as_ref() {
                map[y.saturating_mul(w) + x] as usize
            } else {
                y.saturating_mul(w).saturating_add(x)
            };
            let cols = area.width as usize;
            if let Some(stream) = seed_runtime.render_cache().hilbert_stream.as_ref() {
                let total = stream.len().max(1);
                if cols > 0 {
                    let stride = total.div_ceil(cols);
                    if stride > 0 {
                        let col = idx / stride;
                        if col < cols {
                            let sx = area.x + col as u16;
                            for yy in 0..area.height {
                                let cell = buf.get_mut(sx, area.y + yy);
                                cell.set_char('│');
                                cell.set_fg(palette.accent);
                            }
                            draw_hilbert_inset_highlight(area, buf, seed, palette, x, y);
                        }
                    }
                }
            }
        }
        SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => {
            // Same crosshair as AsciiBytes — highlight the cell.
            let w = seed.base_values.width().max(1);
            let h = seed.base_values.height().max(1);
            let (digits, gap, cols) = ascii_layout(area.width as usize, w);
            if cols == 0 || digits == 0 {
                return;
            }
            let rows = area.height as usize;
            let cx = x.saturating_mul(cols) / w;
            let cy = y.saturating_mul(rows.max(1)) / h;
            let stride = digits + gap;
            let start_x = area.x + (cx * stride) as u16;
            let row_y = area.y + cy as u16;
            for dx in 0..digits {
                let cell = buf.get_mut(start_x + dx as u16, row_y);
                cell.set_bg(palette.accent);
            }
        }
    }
}

fn draw_hilbert_inset_highlight(
    area: ratatui::layout::Rect,
    buf: &mut Buffer,
    seed: &nit_core::EncodedSeed,
    palette: &SeedPalette,
    x: usize,
    y: usize,
) {
    let inset_w = 16u16;
    let inset_h = 16u16;
    if area.width < inset_w || area.height < inset_h {
        return;
    }
    let inset_x = area.x + area.width - inset_w;
    let inset_y = area.y;
    let w = seed.base_bits.width().max(1);
    let h = seed.base_bits.height().max(1);
    let ix = x.saturating_mul(16) / w;
    let iy = y.saturating_mul(16) / h;
    if ix >= 16 || iy >= 16 {
        return;
    }
    let cell = buf.get_mut(inset_x + ix as u16, inset_y + iy as u16);
    cell.set_char('o');
    cell.set_fg(palette.accent);
}

fn inspector_pos(state: &AppState) -> (usize, usize) {
    match state.visualizer.seed_encoder {
        SeedEncoderId::AsciiBytes
        | SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => (
            state.visualizer.inspect_ascii_x,
            state.visualizer.inspect_ascii_y,
        ),
        SeedEncoderId::Lifehash16 => (
            state.visualizer.inspect_lifehash_x,
            state.visualizer.inspect_lifehash_y,
        ),
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => (
            state.visualizer.inspect_hilbert_x,
            state.visualizer.inspect_hilbert_y,
        ),
    }
}

fn clamp_inspector_pos(state: &AppState, seed: &nit_core::EncodedSeed) -> (usize, usize) {
    let (mut x, mut y) = inspector_pos(state);
    let w = seed.base_bits.width().max(1);
    let h = seed.base_bits.height().max(1);
    if x >= w {
        x = w - 1;
    }
    if y >= h {
        y = h - 1;
    }
    (x, y)
}

fn map_to_screen(
    area: ratatui::layout::Rect,
    x: usize,
    y: usize,
    grid_w: usize,
    grid_h: usize,
) -> Option<(u16, u16)> {
    if grid_w == 0 || grid_h == 0 || area.width == 0 || area.height == 0 {
        return None;
    }
    let left = x.saturating_mul(area.width as usize) / grid_w;
    let right = (x + 1).saturating_mul(area.width as usize) / grid_w;
    let top = y.saturating_mul(area.height as usize) / grid_h;
    let bottom = (y + 1).saturating_mul(area.height as usize) / grid_h;
    let sx = area.x + ((left + right) / 2) as u16;
    let sy = area.y + ((top + bottom) / 2) as u16;
    Some((sx, sy))
}

fn write_view_label(writer: &mut LineWriter<'_>, state: &AppState) {
    match state.visualizer.seed_view {
        SeedViewMode::Genome => writer.write_str("GENOME"),
        SeedViewMode::Plate => {
            writer.write_str("PLATE/");
            writer.write_str(state.visualizer.seed_plate_mode.label());
        }
        SeedViewMode::Map => writer.write_str("MAP"),
        SeedViewMode::Stats => writer.write_str("STATS"),
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
    let label_style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text);
    let heading_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    write_line(buf, area, y, "GENOME PROTOCOL", heading_style);
    y = y.saturating_add(1);
    let params = &seed.params;
    write_kv(
        buf,
        area,
        &mut y,
        "Encoder",
        seed.encoder_id.as_str(),
        label_style,
        value_style,
        max_y,
    );
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
    let label_style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text);
    let heading_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
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
        "Plate dens",
        &format!("{:.2}", state.visualizer.seed_stats.density),
        label_style,
        value_style,
        max_y,
    );
    write_kv(
        buf,
        area,
        &mut y,
        "Plate comp",
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

fn write_line(buf: &mut Buffer, area: ratatui::layout::Rect, y: u16, text: &str, style: Style) {
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

#[allow(clippy::too_many_arguments)]
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

struct LineWriter<'a> {
    buf: &'a mut Buffer,
    y: u16,
    x: u16,
    max_x: u16,
    style: Style,
    truncated: bool,
}

impl<'a> LineWriter<'a> {
    fn new(buf: &'a mut Buffer, area: ratatui::layout::Rect, style: Style) -> Self {
        let max_x = area.x.saturating_add(area.width);
        Self {
            buf,
            y: area.y,
            x: area.x,
            max_x,
            style,
            truncated: false,
        }
    }

    fn write_char(&mut self, ch: char) {
        if self.x >= self.max_x {
            self.truncated = true;
            return;
        }
        let cell = self.buf.get_mut(self.x, self.y);
        cell.set_char(ch);
        cell.set_style(self.style);
        self.x = self.x.saturating_add(1);
    }

    fn write_str(&mut self, text: &str) {
        for ch in text.chars() {
            if self.x >= self.max_x {
                self.truncated = true;
                return;
            }
            let cell = self.buf.get_mut(self.x, self.y);
            cell.set_char(ch);
            cell.set_style(self.style);
            self.x = self.x.saturating_add(1);
        }
    }

    fn write_sep(&mut self) {
        self.write_str(" | ");
    }

    fn write_u32(&mut self, mut value: u32) {
        let mut buf = [0u8; 10];
        let mut i = 0usize;
        if value == 0 {
            self.write_char('0');
            return;
        }
        while value > 0 && i < buf.len() {
            buf[i] = (value % 10) as u8;
            value /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            self.write_char((b'0' + buf[i]) as char);
        }
    }

    fn write_hex_u32(&mut self, value: u32) {
        for shift in (0..8).rev() {
            let nibble = ((value >> (shift * 4)) & 0xF) as u8;
            self.write_char(hex_digit(nibble));
        }
    }

    fn write_hex_u8(&mut self, value: u8) {
        self.write_char(hex_digit(value >> 4));
        self.write_char(hex_digit(value & 0xF));
    }

    fn write_f32_2(&mut self, value: f32) {
        let scaled = (value * 100.0).round().clamp(0.0, 9999.0) as u32;
        let int_part = scaled / 100;
        let frac = scaled % 100;
        self.write_u32(int_part);
        self.write_char('.');
        self.write_char((b'0' + (frac / 10) as u8) as char);
        self.write_char((b'0' + (frac % 10) as u8) as char);
    }

    fn finish(mut self) {
        if self.truncated && self.max_x > 0 {
            let ellip_x = self.max_x - 1;
            let cell = self.buf.get_mut(ellip_x, self.y);
            cell.set_char('…');
            cell.set_style(self.style);
        }
        while self.x < self.max_x {
            let cell = self.buf.get_mut(self.x, self.y);
            cell.set_char(' ');
            cell.set_style(self.style);
            self.x = self.x.saturating_add(1);
        }
    }
}

fn hex_digit(value: u8) -> char {
    match value & 0xF {
        0..=9 => (b'0' + (value & 0xF)) as char,
        v => (b'a' + (v - 10)) as char,
    }
}
