//! Polls a PTY shell's current working directory via sysinfo so the
//! terminal popup title can follow `cd`. Throttled to one refresh per
//! `REFRESH_INTERVAL` so the cost stays well below a render frame.
//!
//! macOS reads cwd via `proc_pidinfo`; Linux reads `/proc/<pid>/cwd`;
//! both are exposed uniformly by `sysinfo::ProcessExt::cwd`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sysinfo::{Pid, PidExt, ProcessExt, ProcessRefreshKind, System, SystemExt};

const REFRESH_INTERVAL: Duration = Duration::from_millis(400);

/// Caches a `sysinfo::System` and only refreshes the watched pid at
/// most once per [`REFRESH_INTERVAL`]. The cache key is the pid, so a
/// new shell session (e.g. operator ran `exit`, popup respawned) is
/// detected when the pid changes.
pub struct ShellCwdProbe {
    system: System,
    last_refresh: Instant,
    last_pid: Option<u32>,
    last_cwd: Option<PathBuf>,
}

impl ShellCwdProbe {
    pub fn new() -> Self {
        Self {
            system: System::new(),
            // Force the first call to refresh by dating the cache back.
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            last_pid: None,
            last_cwd: None,
        }
    }

    /// Returns the live cwd of the shell with `pid`, or `None` if the
    /// platform backend can't read it (sandboxed env, dead pid, etc.).
    /// Cached for `REFRESH_INTERVAL`; the cache invalidates on pid
    /// change so respawned shells don't keep the old cwd.
    pub fn cwd(&mut self, pid: u32) -> Option<PathBuf> {
        let pid_changed = self.last_pid != Some(pid);
        let stale = pid_changed || self.last_refresh.elapsed() >= REFRESH_INTERVAL;
        if !stale {
            return self.last_cwd.clone();
        }
        self.system
            .refresh_process_specifics(Pid::from_u32(pid), ProcessRefreshKind::new());
        let cwd = self
            .system
            .process(Pid::from_u32(pid))
            .map(|process| process.cwd().to_path_buf())
            .filter(|path| !path.as_os_str().is_empty());
        self.last_refresh = Instant::now();
        self.last_pid = Some(pid);
        self.last_cwd = cwd.clone();
        cwd
    }
}

impl Default for ShellCwdProbe {
    fn default() -> Self {
        Self::new()
    }
}
