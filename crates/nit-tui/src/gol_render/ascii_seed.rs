use ratatui::{
    buffer::Buffer as FrameBuffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Widget,
};

use nit_core::Buffer as TextBuffer;

use super::hud::write_str;
use super::palette::GolPalette;

pub struct AsciiSeedWidget<'a> {
    pub buffer: &'a TextBuffer,
    pub palette: GolPalette,
    pub header: &'a str,
}

impl Widget for AsciiSeedWidget<'_> {
    fn render(self, area: Rect, buf: &mut FrameBuffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let max_x = area.x.saturating_add(area.width);
        let max_y = area.y.saturating_add(area.height);

        self.clear_area(buf, area, max_x, max_y);
        self.write_header(buf, area, max_x);

        let content_height = area.height.saturating_sub(1) as usize;
        if content_height == 0 {
            return;
        }
        self.write_content(buf, area, max_x, content_height);
    }
}

impl AsciiSeedWidget<'_> {
    fn clear_area(&self, buf: &mut FrameBuffer, area: Rect, max_x: u16, max_y: u16) {
        let bg = self.palette.bg;
        let fg = self.palette.hud_dim;
        for y in area.y..max_y {
            for x in area.x..max_x {
                let cell = buf.get_mut(x, y);
                cell.set_char(' ');
                cell.set_bg(bg);
                cell.set_fg(fg);
            }
        }
    }

    fn write_header(&self, buf: &mut FrameBuffer, area: Rect, max_x: u16) {
        let style = Style::default()
            .fg(self.palette.hud_text)
            .bg(self.palette.bg)
            .add_modifier(Modifier::DIM);
        let _ = write_str(buf, area.x, area.y, max_x, style, self.header);
    }

    fn write_content(&self, buf: &mut FrameBuffer, area: Rect, max_x: u16, content_height: usize) {
        let bg = self.palette.bg;
        let styles = DigitStyles {
            text: Style::default().fg(self.palette.hud_text).bg(bg),
            zero: Style::default()
                .fg(self.palette.hud_dim)
                .bg(bg)
                .add_modifier(Modifier::DIM),
            space: Style::default().fg(self.palette.hud_dim).bg(bg),
        };

        let start_line = self.buffer.viewport.offset_line;
        let start_col = self.buffer.viewport.offset_col;

        for row in 0..content_height {
            let line = self.line_suffix(start_line + row, start_col);
            let y = area.y + 1 + row as u16;
            render_digit_line(buf, area.x, y, max_x, &line, &styles);
        }
    }

    fn line_suffix(&self, line_idx: usize, start_col: usize) -> String {
        let mut line = if line_idx < self.buffer.lines_len() {
            self.buffer.line_as_string(line_idx)
        } else {
            String::new()
        };
        if line.ends_with('\n') {
            line.pop();
        }
        line.chars().skip(start_col).collect()
    }
}

struct DigitStyles {
    text: Style,
    zero: Style,
    space: Style,
}

fn render_digit_line(
    buf: &mut FrameBuffer,
    start_x: u16,
    y: u16,
    max_x: u16,
    line: &str,
    styles: &DigitStyles,
) {
    let mut x = start_x;
    for ch in line.chars() {
        if x >= max_x {
            break;
        }
        x = write_digit_triplet(buf, x, y, max_x, ch, styles);
        if x < max_x {
            let cell = buf.get_mut(x, y);
            cell.set_char(' ');
            cell.set_style(styles.space);
            x = x.saturating_add(1);
        }
    }
}

fn write_digit_triplet(
    buf: &mut FrameBuffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    ch: char,
    styles: &DigitStyles,
) -> u16 {
    let code = if ch.is_ascii() { ch as u32 } else { 127 };
    let digits = [
        ((code / 100) % 10) as u8,
        ((code / 10) % 10) as u8,
        (code % 10) as u8,
    ];
    for digit in digits {
        if x >= max_x {
            break;
        }
        let style = if digit == 0 { styles.zero } else { styles.text };
        let cell = buf.get_mut(x, y);
        cell.set_char((b'0' + digit) as char);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}
