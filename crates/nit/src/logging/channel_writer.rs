use std::fs;
use std::io::{self, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

pub(super) type SharedFile = Arc<Mutex<fs::File>>;

#[derive(Clone)]
pub(super) struct LogWriter {
    pub(super) tx: mpsc::Sender<String>,
    pub(super) file: Option<SharedFile>,
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
            let line = String::from_utf8_lossy(&self.buf[..=newline_pos])
                .trim()
                .to_string();
            self.buf.drain(..=newline_pos);
            if line.is_empty() {
                continue;
            }
            if let Some(mut handle) = self.file.as_ref().and_then(|f| f.lock().ok()) {
                let _ = writeln!(handle, "{line}");
            }
            let _ = self.tx.send(line);
        }
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
        self.buf.clear();
        if !trailing.is_empty() {
            if let Some(mut handle) = self.file.as_ref().and_then(|f| f.lock().ok()) {
                let _ = writeln!(handle, "{trailing}");
            }
            let _ = self.tx.send(trailing);
        }
        Ok(())
    }
}
