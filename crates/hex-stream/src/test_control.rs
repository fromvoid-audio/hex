//! Test-only control of one exact source page on its real worker thread.
//! The control never runs on the callback, and drop or worker Stop releases it.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Gate {
    path: PathBuf,
    page: u64,
    entered: AtomicBool,
    released: AtomicBool,
}
fn slot() -> &'static Mutex<Option<Arc<Gate>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<Gate>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}
/// Owns a single test gate. Only the matching path and page are held.
pub struct HeldPage(Arc<Gate>);
impl HeldPage {
    pub fn install(path: PathBuf, page: u64) -> Result<Self, &'static str> {
        let mut slot = slot().lock().map_err(|_| "page gate poisoned")?;
        if slot.is_some() {
            return Err("page gate already installed");
        }
        let gate = Arc::new(Gate {
            path,
            page,
            entered: AtomicBool::new(false),
            released: AtomicBool::new(false),
        });
        *slot = Some(Arc::clone(&gate));
        Ok(Self(gate))
    }
    pub fn entered(&self) -> bool {
        self.0.entered.load(Ordering::Acquire)
    }
    pub fn release(&self) {
        self.0.released.store(true, Ordering::Release);
    }
}
impl Drop for HeldPage {
    fn drop(&mut self) {
        self.release();
        if let Ok(mut slot) = slot().lock() {
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.0))
            {
                *slot = None;
            }
        }
    }
}
pub(crate) fn hold_if_selected(path: &Path, page: u64, stop: &AtomicBool) {
    let gate = slot().lock().ok().and_then(|slot| {
        slot.as_ref()
            .filter(|g| g.path == path && g.page == page)
            .cloned()
    });
    if let Some(gate) = gate {
        gate.entered.store(true, Ordering::Release);
        while !gate.released.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
            thread::park_timeout(Duration::from_millis(2));
        }
    }
}
