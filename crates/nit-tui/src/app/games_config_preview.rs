use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use nit_core::{AppKind, AppState};
use nit_games::config::GamesConfig;

pub(super) enum GamesConfigPreviewCommand {
    Parse {
        version: u64,
        config_text: String,
        workspace_root: PathBuf,
    },
    Shutdown,
}

pub(super) struct GamesConfigPreviewEvent {
    version: u64,
    result: Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
}

pub(super) struct GamesConfigPreviewRuntime {
    cmd_tx: Sender<GamesConfigPreviewCommand>,
    events: Receiver<GamesConfigPreviewEvent>,
    handle: Option<JoinHandle<()>>,
    pending_version: Option<u64>,
}

impl GamesConfigPreviewRuntime {
    pub(super) fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<GamesConfigPreviewCommand>();
        let (event_tx, event_rx) = mpsc::channel::<GamesConfigPreviewEvent>();
        let handle = thread::Builder::new()
            .name("nit-games-config-preview".into())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        GamesConfigPreviewCommand::Parse {
                            version,
                            config_text,
                            workspace_root,
                        } => {
                            let result = GamesConfig::from_toml_with_root(
                                &config_text,
                                Some(&workspace_root),
                            );
                            let _ = event_tx.send(GamesConfigPreviewEvent { version, result });
                        }
                        GamesConfigPreviewCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawn games config preview");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            pending_version: None,
        }
    }

    pub(super) fn request_for_editor(&mut self, state: &mut AppState) {
        if state.app_kind != AppKind::Games {
            return;
        }
        let version = state.editor_buffer().version();
        if state
            .games
            .config_preview
            .as_ref()
            .is_some_and(|preview| preview.version == version)
            || self.pending_version == Some(version)
        {
            return;
        }
        let cmd = GamesConfigPreviewCommand::Parse {
            version,
            config_text: state.editor_buffer().content_as_string(),
            workspace_root: state.workspace_root.clone(),
        };
        if self.cmd_tx.send(cmd).is_ok() {
            self.pending_version = Some(version);
            state.games.config_preview_pending = true;
        } else {
            state.games.config_preview_pending = false;
        }
    }

    pub(super) fn poll(&mut self, state: &mut AppState) {
        while let Ok(event) = self.events.try_recv() {
            state.games.config_preview = Some(nit_core::GamesConfigPreview {
                version: event.version,
                result: event.result,
            });
            if self.pending_version == Some(event.version) {
                self.pending_version = None;
            }
            if state.editor_buffer().version() == event.version {
                state.games.config_preview_pending = false;
            }
        }
    }

    pub(super) fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(GamesConfigPreviewCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
