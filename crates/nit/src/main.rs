#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use clap::{Parser, Subcommand};
use nit_core::{io as core_io, AppKind, Buffer, Mode, SelectedRule};
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Parser, Debug)]
#[command(
    name = "nit",
    version,
    about = "Neural Interface Terminal",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    /// File or directory to open (GoL mode)
    path: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Explicit GoL mode (current behavior)
    Gol {
        /// File or directory to open
        path: Option<PathBuf>,
    },
    /// Games mode (games between programs)
    Games {
        /// File or directory to open
        path: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (app_kind, target) = match cli.command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path }) => (AppKind::Games, path),
        None => (AppKind::Gol, cli.path),
    };
    let (workspace_root, editor) = match app_kind {
        AppKind::Gol => open_target_gol(target.as_deref())?,
        AppKind::Games => open_target_games(target.as_deref())?,
    };
    let notes = load_notes(&workspace_root);

    let theme_path = find_theme();
    let theme = Theme::load(theme_path.as_deref());

    let (log_tx, log_rx) = mpsc::channel::<String>();
    let log_path = log_path_for_workspace(&workspace_root);
    init_tracing(log_tx, log_path)?;
    install_panic_hook();

    let mut state = nit_core::AppState::new(workspace_root, editor, notes);
    state.app_kind = app_kind;
    let seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.visualizer.seed = seed;
    state.mode = Mode::Normal;

    if app_kind == AppKind::Gol {
        let rule_config = nit_core::load_rule_config(&state.workspace_root);
        let (catalog, mut rule_warnings) = nit_core::load_rule_catalog(&rule_config.rules.user);
        rule_warnings.extend(rule_config.warnings.into_iter());
        for warning in rule_warnings {
            tracing::warn!("{warning}");
        }
        let selected_key = if rule_config.rule.workspace_override {
            rule_config
                .workspace_rule
                .clone()
                .unwrap_or_else(|| rule_config.rule.default.clone())
        } else {
            rule_config.rule.default.clone()
        };
        let selected = match catalog.select(&selected_key) {
            Ok(selected) => selected,
            Err(err) => {
                tracing::warn!("Invalid configured GoL rule '{selected_key}': {err}");
                SelectedRule::default()
            }
        };
        state.settings.gol.rule = rule_config.rule.clone();
        state.settings.gol.rules = rule_config.rules.clone();
        state.init_rules(
            catalog,
            selected,
            nit_core::RulePersistence {
                global_path: rule_config.global_path,
                workspace_path: rule_config.workspace_path,
                workspace_override: rule_config.rule.workspace_override,
            },
        );
    }

    run(state, theme, log_rx)?;
    Ok(())
}

fn open_target_gol(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let content = core_io::load_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            let buffer = Buffer::from_str(name, &content, Some(p.to_path_buf()));
            let root = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(std::env::current_dir()?);
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => {
            let root = p.to_path_buf();
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        None => {
            let root = std::env::current_dir()?;
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn open_target_games(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let content = core_io::load_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "games.toml".into());
            let buffer = Buffer::from_str(name, &content, Some(p.to_path_buf()));
            let root = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(std::env::current_dir()?);
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => open_games_workspace(p),
        None => {
            let root = std::env::current_dir()?;
            open_games_workspace(&root)
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn open_games_workspace(root: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    let root = root.to_path_buf();
    let config_path = root.join("games.toml");
    if config_path.exists() {
        let content = core_io::load_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let buffer = Buffer::from_str("games.toml", &content, Some(config_path));
        return Ok((root, buffer));
    }
    let buffer = Buffer::from_str("games.toml", games_template(), Some(config_path));
    Ok((root, buffer))
}

fn games_template() -> &'static str {
    r#"schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = false
seed = 12345
noise = 0.0

[payoff]
R = 3
S = 0
T = 5
P = 1

[history]
enabled = true

[[strategy]]
id = "allc"
type = "builtin"
name = "Always Cooperate"

[[strategy]]
id = "alld"
type = "builtin"
name = "Always Defect"

[[strategy]]
id = "tft"
type = "builtin"
name = "Tit For Tat"

[[strategy]]
id = "grim"
type = "builtin"
name = "Grim Trigger"

[[strategy]]
id = "pavlov"
type = "builtin"
name = "Win Stay Lose Shift"

[[strategy]]
id = "rand50"
type = "random"
p_cooperate = 0.5

[[strategy]]
id = "fsm1"
type = "fsm"
start_state = 0
output = ["C", "D"]
transitions = [
  [0, 1, 0, 1],
  [1, 1, 0, 0],
]

[[strategy]]
id = "mem1"
type = "memory"
n = 1
initial = "C"
table = ["C", "D", "D", "C"]
"#
}

fn find_theme() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let local = cwd.join("assets/themes/devs.toml");
    if local.exists() {
        return Some(local);
    }
    None
}

fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(path) = notes_path_for_workspace(workspace_root) else {
        return Buffer::empty("notes", None);
    };
    if path.exists() {
        if let Ok(content) = core_io::load_to_string(&path) {
            return Buffer::from_str("notes", &content, Some(path));
        }
    }
    Buffer::empty("notes", Some(path))
}

fn notes_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let notes_dir = base.join("notes");
    let _ = fs::create_dir_all(&notes_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.md", hash);
    Some(notes_dir.join(filename))
}

fn init_tracing(tx: mpsc::Sender<String>, log_path: Option<PathBuf>) -> anyhow::Result<()> {
    let file = log_path
        .as_ref()
        .and_then(|path| open_log_file(path).ok())
        .map(|file| Arc::new(Mutex::new(file)));
    let writer = LogWriter { tx, file };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter("info")
        .try_init()
        .ok();
    if let Some(path) = log_path {
        tracing::info!("Log file: {}", path.display());
    }
    Ok(())
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("PANIC: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("BACKTRACE: {bt:?}");
    }));
}

#[derive(Clone)]
struct LogWriter {
    tx: mpsc::Sender<String>,
    file: Option<Arc<Mutex<std::fs::File>>>,
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

struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buf: Vec<u8>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.drain_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_lines();
        if !self.buf.is_empty() {
            let msg = String::from_utf8_lossy(&self.buf).trim().to_string();
            if !msg.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", msg);
                    }
                }
                let _ = self.tx.send(msg);
            }
            self.buf.clear();
        }
        Ok(())
    }
}

impl ChannelWriter {
    fn drain_lines(&mut self) {
        loop {
            let Some(pos) = self.buf.iter().position(|b| *b == b'\n') else {
                break;
            };
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).trim().to_string();
            if !line.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", line);
                    }
                }
                let _ = self.tx.send(line);
            }
        }
    }
}

fn log_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NIT_LOG_PATH") {
        return Some(PathBuf::from(path));
    }
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let logs_dir = base.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.log", hash);
    Some(logs_dir.join(filename))
}

fn open_log_file(path: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}
