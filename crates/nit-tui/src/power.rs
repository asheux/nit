//! Idle-sleep guard for long-running agent turns.
//!
//! macOS hibernates after a few minutes of input inactivity by default. The
//! Claude / Codex subprocesses get SIGSTOP'd, their TCP connections to the
//! provider die, and on wake they exit non-zero — taking the swarm with
//! them. The guard holds a `caffeinate -i` while at least one agent has an
//! in-flight turn, blocking idle sleep without blocking display sleep
//! (battery cost is minimal: the screen still dims/sleeps).
//!
//! Scope:
//! - Only blocks **idle** sleep. Lid-close on macOS forces sleep regardless
//!   of any power assertion (unless on AC + external display in clamshell).
//! - macOS-only. On other platforms the guard is a no-op so callers can
//!   keep the same wiring.
//! - `caffeinate -i -w <nit_pid>` is the implementation: caffeinate
//!   auto-exits when the watched pid dies, so even a hard-kill of nit
//!   leaves no stale caffeinate behind.

use std::process::{Child, Command, Stdio};

#[derive(Default)]
pub struct IdleSleepGuard {
    child: Option<Child>,
}

impl IdleSleepGuard {
    /// Spawn `caffeinate -i -w <pid>` if not already held. Idempotent — a
    /// second `acquire` while held is a no-op. On non-macOS targets the
    /// guard remains released; callers don't need to gate calls themselves.
    pub fn acquire(&mut self) {
        if self.child.is_some() {
            return;
        }
        if !cfg!(target_os = "macos") {
            return;
        }
        let pid = std::process::id().to_string();
        // `-i` = no idle sleep (display can still sleep).
        // `-w <pid>` = caffeinate exits when the watched pid does, so a
        // panic / hard-kill in nit can never strand a caffeinate process.
        match Command::new("caffeinate")
            .args(["-i", "-w", pid.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => self.child = Some(child),
            Err(_) => {
                // Caffeinate missing (unusual on macOS) → degrade silently.
                // Releasing the guard repeatedly hits this same path and
                // also no-ops, so no extra state needed.
            }
        }
    }

    /// Kill the held child, if any. Safe to call repeatedly.
    pub fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn is_held(&self) -> bool {
        self.child.is_some()
    }

    /// Reconcile the guard against the current in-flight count and setting.
    /// Centralising this lets the event loop call `sync` after each drain
    /// without juggling acquire/release directly.
    pub fn sync(&mut self, enabled: bool, in_flight: usize) {
        if enabled && in_flight > 0 {
            self.acquire();
        } else {
            self.release();
        }
    }
}

impl Drop for IdleSleepGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_on_unheld_guard_is_a_noop() {
        let mut guard = IdleSleepGuard::default();
        assert!(!guard.is_held());
        guard.release();
        assert!(!guard.is_held());
    }

    #[test]
    fn sync_disabled_releases_even_when_in_flight() {
        let mut guard = IdleSleepGuard::default();
        guard.sync(false, 5);
        assert!(!guard.is_held());
    }

    #[test]
    fn sync_enabled_with_zero_in_flight_releases() {
        let mut guard = IdleSleepGuard::default();
        guard.sync(true, 0);
        assert!(!guard.is_held());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sync_enabled_with_in_flight_acquires_then_releases_on_idle() {
        let mut guard = IdleSleepGuard::default();
        guard.sync(true, 3);
        assert!(
            guard.is_held(),
            "guard should hold caffeinate while turns active"
        );
        guard.sync(true, 0);
        assert!(
            !guard.is_held(),
            "guard should release when in-flight returns to 0"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn drop_releases_held_guard() {
        let mut guard = IdleSleepGuard::default();
        guard.sync(true, 1);
        assert!(guard.is_held());
        drop(guard);
        // No panic / leftover process — drop is the only signal we can
        // assert on without spelunking process tables. Coverage is in
        // `release()` being called from Drop.
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn double_acquire_is_idempotent() {
        let mut guard = IdleSleepGuard::default();
        guard.acquire();
        let first_held = guard.is_held();
        guard.acquire();
        assert_eq!(first_held, guard.is_held());
        guard.release();
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_acquire_is_noop() {
        let mut guard = IdleSleepGuard::default();
        guard.sync(true, 5);
        assert!(!guard.is_held());
    }
}
