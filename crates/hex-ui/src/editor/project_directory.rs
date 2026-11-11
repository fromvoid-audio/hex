//! One last-successful project folder, with control-side atomic preference I/O.
use super::EditorView;
use gpui::Context;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

type PreferenceResult<T> = Result<T, hex_preferences::PreferencesError>;

#[cfg(test)]
std::thread_local! {
    static INITIAL_LOAD_GATE: std::cell::RefCell<Option<Arc<TestGate>>> = const { std::cell::RefCell::new(None) };
}

#[derive(Default)]
struct Writes {
    pending: Option<PathBuf>,
    running: bool,
}

pub(super) struct State {
    path: PathBuf,
    directory: Option<PathBuf>,
    loading: bool,
    writable: bool,
    changed_while_loading: bool,
    writes: Arc<Mutex<Writes>>,
    #[cfg(test)]
    save_gate: Option<Arc<TestGate>>,
    #[cfg(test)]
    directory_gate: Option<Arc<TestGate>>,
}
impl State {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_path(&self) -> &std::path::Path {
        &self.path
    }

    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            directory: None,
            loading: true,
            writable: true,
            changed_while_loading: false,
            writes: Arc::default(),
            #[cfg(test)]
            save_gate: None,
            #[cfg(test)]
            directory_gate: None,
        }
    }
}

pub(super) fn default_preferences_path() -> PathBuf {
    #[cfg(test)]
    {
        PathBuf::new()
    } // Resolved to the retained #414 environment before startup.
    #[cfg(not(test))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        hex_preferences::project_directory_preferences_path(&home)
    }
}

fn usable_directory(directory: Option<PathBuf>) -> Option<PathBuf> {
    directory.filter(|path| path.is_absolute() && std::fs::read_dir(path).is_ok())
}

impl EditorView {
    pub(super) fn start_project_directory_load(&mut self, cx: &mut Context<Self>) {
        let path = self.project_directory.path.clone();
        #[cfg(test)]
        let environment = Arc::clone(&self.test_environment);
        #[cfg(test)]
        let load_gate = INITIAL_LOAD_GATE.with(|slot| slot.borrow_mut().take());
        #[cfg(test)]
        let completion = load_gate.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    #[cfg(test)]
                    if let Some(gate) = load_gate {
                        gate.wait().await;
                    }
                    hex_preferences::load_project_directory(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.project_directory.loading = false;
                match result {
                    Ok(directory) => {
                        if !view.project_directory.changed_while_loading {
                            view.project_directory.directory = directory;
                        }
                        view.start_project_directory_save(cx);
                    }
                    Err(error) => {
                        view.project_directory.writable = false;
                        view.project_directory.directory = None;
                        if let Ok(mut writes) = view.project_directory.writes.lock() {
                            writes.pending = None;
                        }
                        view.project_directory_error("load", error);
                    }
                }
                cx.notify();
            });
            #[cfg(test)]
            if let Some(gate) = completion {
                gate.finished
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        })
        .detach();
    }

    fn project_directory_error(
        &mut self,
        operation: &str,
        error: hex_preferences::PreferencesError,
    ) {
        self.error = Some(
            format!(
                "could not {operation} project folder · {}",
                hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
            )
            .into(),
        );
    }

    /// Called ONLY after canonical successful Open/Save completion, never picker acceptance.
    pub(super) fn record_project_directory(&mut self, document: PathBuf, cx: &mut Context<Self>) {
        if !self.project_directory.writable {
            self.error = Some(
                "could not remember project folder · original invalid preferences preserved".into(),
            );
            return;
        }
        let Some(parent) = document.parent().filter(|path| path.is_absolute()) else {
            return;
        };
        let directory = parent.to_path_buf();
        self.project_directory.directory = Some(directory.clone());
        self.project_directory.changed_while_loading = true;
        match self.project_directory.writes.lock() {
            Ok(mut writes) => writes.pending = Some(directory),
            Err(_) => {
                self.error = Some("could not queue project folder preference".into());
                return;
            }
        }
        self.start_project_directory_save(cx);
    }

    fn start_project_directory_save(&mut self, cx: &mut Context<Self>) {
        if !self.project_directory.writable {
            return;
        }
        let writes = Arc::clone(&self.project_directory.writes);
        let start = match writes.lock() {
            Ok(mut writes) if !writes.running && writes.pending.is_some() => {
                writes.running = true;
                true
            }
            _ => false,
        };
        if !start {
            return;
        }
        let path = self.project_directory.path.clone();
        #[cfg(test)]
        let gate = self.project_directory.save_gate.take();
        #[cfg(test)]
        let environment = Arc::clone(&self.test_environment);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.wait().await;
                    }
                    // This owned worker drains the one latest pending value even if
                    // its view is dropped. No filesystem I/O occurs under the mutex.
                    let mut result: PreferenceResult<()> = Ok(());
                    loop {
                        let directory = match writes.lock() {
                            Ok(mut writes) => match writes.pending.take() {
                                Some(directory) => directory,
                                None => {
                                    writes.running = false;
                                    break;
                                }
                            },
                            Err(_) => {
                                result = Err(hex_preferences::PreferencesError::InvalidDocument(
                                    "project folder queue unavailable".into(),
                                ));
                                break;
                            }
                        };
                        result = hex_preferences::save_project_directory(&path, &directory);
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.project_directory_error("save", error);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Wait for the initial bounded load, then validate access off the UI thread.
    /// Missing/unavailable directories produce the existing picker default.
    pub(super) fn with_project_picker_directory(
        &mut self,
        cx: &mut Context<Self>,
        ready: impl FnOnce(&mut Self, Option<PathBuf>, &mut Context<Self>) + 'static,
    ) {
        if !self.project_directory.loading && self.project_directory.directory.is_none() {
            ready(self, None, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let directory = loop {
                match this.update(cx, |view, _| {
                    (!view.project_directory.loading)
                        .then(|| view.project_directory.directory.clone())
                }) {
                    Ok(Some(directory)) => break directory,
                    Err(_) => return,
                    _ => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(5))
                            .await
                    }
                }
            };
            #[cfg(test)]
            let environment = match this.update(cx, |view, _| Arc::clone(&view.test_environment)) {
                Ok(environment) => environment,
                Err(_) => return,
            };
            #[cfg(test)]
            let gate = match this.update(cx, |view, _| view.project_directory.directory_gate.take())
            {
                Ok(gate) => gate,
                Err(_) => return,
            };
            let directory = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.wait().await;
                    }
                    usable_directory(directory)
                })
                .await;
            let _ = this.update(cx, |view, cx| ready(view, directory, cx));
        })
        .detach();
    }
}

#[cfg(test)]
#[derive(Default)]
struct TestGate {
    finished: std::sync::atomic::AtomicBool,
    reached: std::sync::atomic::AtomicBool,
    released: std::sync::atomic::AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}
#[cfg(test)]
impl TestGate {
    async fn wait(&self) {
        use std::{sync::atomic::Ordering, task::Poll};
        self.reached.store(true, Ordering::Release);
        std::future::poll_fn(|cx| {
            let mut waker = self.waker.lock().expect("test gate");
            if self.released.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                *waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }
    #[cfg(feature = "ui-test-support")]
    fn release(&self) {
        self.released
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("test gate").take() {
            waker.wake();
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
