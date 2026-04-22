use std::fs;
use std::io::{self, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

pub(super) type SharedFile = Arc<Mutex<fs::File>>;

#[derive(Clone)]
pub(super) struct LogWriter {
    tx: mpsc::Sender<String>,
    file: Option<SharedFile>,
}

impl LogWriter {
    pub(super) fn new(tx: mpsc::Sender<String>, file: Option<SharedFile>) -> Self {
        Self { tx, file }
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            buf: Vec::new(),
            file: self.file.clone(),
        }
    }
}

pub(super) struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buf: Vec<u8>,
    file: Option<SharedFile>,
}

impl ChannelWriter {
    fn drain_lines(&mut self) {
        while let Some(newline_pos) = self.buf.iter().position(|&b| b == b'\n') {
            let trimmed_line = String::from_utf8_lossy(&self.buf[..=newline_pos])
                .trim()
                .to_string();
            self.buf.drain(..=newline_pos);
            if trimmed_line.is_empty() {
                continue;
            }
            self.emit(&trimmed_line);
        }
    }

    fn emit(&self, log_line: &str) {
        if let Some(mut handle) = self.file.as_ref().and_then(|f| f.lock().ok()) {
            let _ = writeln!(handle, "{log_line}");
        }
        let _ = self.tx.send(log_line.to_string());
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.drain_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_lines();
        let trailing = String::from_utf8_lossy(&self.buf).trim().to_string();
        if !trailing.is_empty() {
            self.emit(&trailing);
        }
        self.buf.clear();
        Ok(())
    }
}
