//! Process-wide signal registration guards for standalone scanner executables.
//! Never invoke from library or host code.
use std::sync::{Arc, atomic::AtomicBool};

pub struct SignalGuard(Vec<signal_hook::SigId>);

impl SignalGuard {
    /// Register a safe `SIGCHLD` flag handler so waitable children remain
    /// observable even if a launcher inherited `SIG_IGN` or `SA_NOCLDWAIT`.
    pub fn register_sigchld() -> Result<Self, std::io::Error> {
        let notice = Arc::new(AtomicBool::new(false));
        let id = signal_hook::flag::register(signal_hook::consts::SIGCHLD, notice)?;
        Ok(Self(vec![id]))
    }

    /// Register `SIGCHLD` alongside shared `SIGINT`/`SIGTERM` cancellation flags.
    pub fn register_sigchld_and_cancel() -> Result<(Self, Arc<AtomicBool>), String> {
        let mut guard = Self(Vec::with_capacity(3));
        guard.0.push(
            signal_hook::flag::register(
                signal_hook::consts::SIGCHLD,
                Arc::new(AtomicBool::new(false)),
            )
            .map_err(|e| format!("waitable child disposition unavailable: {e}"))?,
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            guard.0.push(
                signal_hook::flag::register(signal, Arc::clone(&cancelled))
                    .map_err(|e| format!("scoped cancellation signal unavailable: {e}"))?,
            );
        }
        Ok((guard, cancelled))
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_guard_unregisters_sigchld_on_drop() {
        let guard = SignalGuard::register_sigchld().unwrap();
        let id = guard.0[0];
        drop(guard);
        assert!(!signal_hook::low_level::unregister(id));
    }
}
