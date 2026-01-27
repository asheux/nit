use ratatui::{
    buffer::Buffer as FrameBuffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Widget,
};

use nit_core::Buffer as TextBuffer;

use super::palette::GolPalette;

pub struct AsciiSeedWidget<'a> {
    pub buffer: &'a TextBuffer,
    pub palette: GolPalette,
    pub header: &'a str,
}

impl<'a> Widget for AsciiSeedWidget<'a> {
    fn render(self, area: Rect, buf: &mut FrameBuffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let bg = self.palette.bg;
        let mut y = area.y;
        let max_x = area.x.saturating_add(area.width);
        let max_y = area.y.saturating_add(area.height);

        while y < max_y {
            let mut x = area.x;
            while x < max_x {
                let cell = buf.get_mut(x, y);
                cell.set_char(' ');
                cell.set_bg(bg);
                cell.set_fg(self.palette.hud_dim);
                x = x.saturating_add(1);
            }
            y = y.saturating_add(1);
        }

        let header_style = Style::default()
            .fg(self.palette.hud_text)
            .bg(bg)
            .add_modifier(Modifier::DIM);
        write_str(buf, area.x, area.y, max_x, header_style, self.header);

        let content_height = area.height.saturating_sub(1) as usize;
        if content_height == 0 {
            return;
        }

        let text_style = Style::default().fg(self.palette.hud_text).bg(bg);
        let zero_style = Style::default()
            .fg(self.palette.hud_dim)
            .bg(bg)
            .add_modifier(Modifier::DIM);
        let space_style = Style::default().fg(self.palette.hud_dim).bg(bg);

        let start_line = self.buffer.viewport.offset_line;
        let start_col = self.buffer.viewport.offset_col;

        for row in 0..content_height {
            let line_idx = start_line + row;
            let mut line = if line_idx < self.buffer.lines_len() {
                self.buffer.line_as_string(line_idx)
            } else {
                String::new()
            };
            if line.ends_with('\n') {
                line.pop();
            }
            let mut chars = line.chars();
            for _ in 0..start_col {
                if chars.next().is_none() {
                    break;
                }
            }

            let y = area.y + 1 + row as u16;
            let mut x = area.x;
            while x < max_x {
                let Some(ch) = chars.next() else {
                    break;
                };
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
                    let style = if digit == 0 { zero_style } else { text_style };
                    let cell = buf.get_mut(x, y);
                    cell.set_char((b'0' + digit) as char);
                    cell.set_style(style);
                    x = x.saturating_add(1);
                }
                if x < max_x {
                    let cell = buf.get_mut(x, y);
                    cell.set_char(' ');
                    cell.set_style(space_style);
                    x = x.saturating_add(1);
                }
            }
        }
    }
}

fn write_str(buf: &mut FrameBuffer, mut x: u16, y: u16, max_x: u16, style: Style, text: &str) {
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
