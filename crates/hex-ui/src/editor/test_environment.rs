//! Test-only startup paths and filesystem-job custody. Never built into the app.
use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Poll, Waker},
};

/// One test-only constructor input boundary; explicit fixture paths are caller-owned.
#[derive(Default)]
pub(super) struct Inputs {
    pub(super) existing_environment: Option<Arc<Environment>>,
    pub(super) directory: Option<PathBuf>,
    pub(super) home: Option<PathBuf>,
    pub(super) defaults: [bool; 4],
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct OwnedRoot {
    path: PathBuf,
    retain_for_evidence: AtomicBool,
}
impl OwnedRoot {
    fn create() -> io::Result<Self> {
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "hex-editor-test-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        retain_for_evidence: AtomicBool::new(false),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate a private editor test root",
        ))
    }
}
impl Drop for OwnedRoot {
    fn drop(&mut self) {
        if self.retain_for_evidence.load(Ordering::Acquire) {
            // Failure evidence outlives uncertain Service completion. Release
            // memory normally; do not leak an Arc or panic during unwinding.
            let _ = writeln!(
                io::stderr().lock(),
                "retained editor test root after unacknowledged setup: {:?}",
                self.path
            );
            return;
        }
        let path = self.path.clone();
        let report_path = path.clone();
        // Teardown must not block the UI. This independent cleanup owns only our
        // exclusively-created root; it is not a Session or application worker join.
        if let Err(error) = std::thread::Builder::new()
            .name("hex-test-cleanup".into())
            .spawn(move || {
                if let Err(error) = fs::remove_dir_all(&path)
                    && error.kind() != io::ErrorKind::NotFound
                {
                    eprintln!("could not remove editor test root {path:?}: {error}");
                }
            })
        {
            eprintln!("could not schedule editor test root cleanup {report_path:?}: {error}");
        }
    }
}

pub(super) struct Environment {
    root: OwnedRoot,
    pub(super) home: PathBuf,
    catalog_save_gate: Mutex<Option<Arc<CatalogSaveGate>>>,
    folder_validation_gate: Mutex<Option<Arc<FolderValidationGate>>>,
}
impl Environment {
    pub(super) fn new() -> io::Result<Arc<Self>> {
        let root = OwnedRoot::create()?;
        let home = root.path.join("home");
        fs::create_dir_all(home.join("Music"))?;
        fs::create_dir(root.path.join("config"))?;
        Ok(Arc::new(Self {
            root,
            home,
            catalog_save_gate: Mutex::new(None),
            folder_validation_gate: Mutex::new(None),
        }))
    }
    #[cfg(feature = "ui-test-support")]
    pub(super) fn retain_for_unacknowledged_setup_failure(&self) {
        self.root.retain_for_evidence.store(true, Ordering::Release);
    }
    pub(super) fn root(&self) -> &Path {
        &self.root.path
    }
    pub(super) fn resolve_paths(
        &self,
        paths: (PathBuf, PathBuf, PathBuf, PathBuf),
        defaults: [bool; 4],
    ) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let (browser, recent, theme, project) = paths;
        let config = self.root.path.join("config");
        // Flags describe constructor intent; explicit paths are never recognized
        // by string comparison or derived from an arbitrary preferences parent.
        (
            if defaults[0] {
                config.join("browser.json")
            } else {
                browser
            },
            if defaults[1] {
                config.join("recent-projects.json")
            } else {
                recent
            },
            if defaults[2] {
                config.join("theme.json")
            } else {
                theme
            },
            if defaults[3] {
                config.join("project-directory.json")
            } else {
                project
            },
        )
    }
    #[cfg(feature = "ui-test-support")]
    pub(super) fn gate_next_folder_validation(&self) -> Arc<FolderValidationGate> {
        let gate = Arc::new(FolderValidationGate::default());
        let mut slot = self
            .folder_validation_gate
            .lock()
            .expect("folder test gate poisoned");
        assert!(
            slot.is_none(),
            "a folder validation gate is already installed"
        );
        *slot = Some(Arc::clone(&gate));
        gate
    }
    pub(super) async fn before_folder_validation(&self) -> Option<Arc<FolderValidationGate>> {
        let gate = self
            .folder_validation_gate
            .lock()
            .expect("folder test gate poisoned")
            .take();
        if let Some(gate) = &gate {
            gate.wait().await;
        }
        gate
    }
    #[cfg(feature = "ui-test-support")]
    pub(super) fn gate_next_catalog_save(&self) -> Arc<CatalogSaveGate> {
        let gate = Arc::new(CatalogSaveGate::default());
        let mut slot = self
            .catalog_save_gate
            .lock()
            .expect("catalog test gate poisoned");
        assert!(
            slot.is_none(),
            "a catalog save test gate is already installed"
        );
        *slot = Some(Arc::clone(&gate));
        gate
    }
    pub(super) async fn before_catalog_save(&self) -> Option<Arc<CatalogSaveGate>> {
        let gate = self
            .catalog_save_gate
            .lock()
            .expect("catalog test gate poisoned")
            .take();
        if let Some(gate) = &gate {
            gate.wait().await;
        }
        gate
    }
}

/// A deterministic test checkpoint inside the real catalog scan/save job. It
/// yields the test executor rather than blocking its thread or replacing I/O.
#[derive(Default)]
pub(super) struct CatalogSaveGate {
    pub(super) reached: AtomicBool,
    released: AtomicBool,
    waker: Mutex<Option<Waker>>,
    pub(super) finished: AtomicBool,
    pub(super) save_succeeded: AtomicBool,
    pub(super) root_present_during_save: AtomicBool,
}
impl CatalogSaveGate {
    pub(super) async fn wait(&self) {
        std::future::poll_fn(|cx| {
            self.reached.store(true, Ordering::Release);
            if self.released.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            *self.waker.lock().expect("catalog test gate poisoned") = Some(cx.waker().clone());
            if self.released.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
    #[cfg(feature = "ui-test-support")]
    pub(super) fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Some(waker) = self
            .waker
            .lock()
            .expect("catalog test gate poisoned")
            .take()
        {
            waker.wake();
        }
    }
    pub(super) fn did_save(&self, succeeded: bool, root_present: bool) {
        self.save_succeeded.store(succeeded, Ordering::Relaxed);
        self.root_present_during_save
            .store(root_present, Ordering::Relaxed);
        self.finished.store(true, Ordering::Release);
    }
}

/// Checkpoint inside the actual folder metadata job, separate from catalog saves.
#[derive(Default)]
pub(super) struct FolderValidationGate {
    pub(super) reached: AtomicBool,
    released: AtomicBool,
    waker: Mutex<Option<Waker>>,
    pub(super) finished: AtomicBool,
    pub(super) validation_succeeded: AtomicBool,
    pub(super) root_present_during_validation: AtomicBool,
}
impl FolderValidationGate {
    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            self.reached.store(true, Ordering::Release);
            *self.waker.lock().expect("folder test gate poisoned") = Some(cx.waker().clone());
            if self.released.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
    #[cfg(feature = "ui-test-support")]
    pub(super) fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("folder test gate poisoned").take() {
            waker.wake();
        }
    }
    pub(super) fn did_validate(&self, succeeded: bool, root_present: bool) {
        self.validation_succeeded
            .store(succeeded, Ordering::Relaxed);
        self.root_present_during_validation
            .store(root_present, Ordering::Relaxed);
        self.finished.store(true, Ordering::Release);
    }
}
