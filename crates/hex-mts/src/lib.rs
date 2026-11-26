//! # Purpose
//! Own explicit desktop-controlled MTS-ESP tuning publication outside audio callbacks.
//!
//! # Dependencies
//! Uses hex-tuning and the existing native mts-esp-master-adapter boundary.
//!
//! # Does not own
//! Does not own project tuning history, instrument DSP or a second tuning model. Process-global
//! native ownership and publisher shutdown remain explicit lifecycle operations.
use hex_tuning::ProjectTuning;
use mts_esp_master_adapter::{Error as NativeError, ScaleMetadata, TuningPublication};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
mod transport;
use transport::Transport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub client: ClientId,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Revision {
    pub owner: Owner,
    pub serial: u64,
    pub project_revision: u64,
}

/// Default scale name published when no authored preset/scale name is supplied.
pub const DEFAULT_SCALE_NAME: &str = "hex tuning";
/// Maximum UTF-8 byte length accepted by MTS-ESP's 17-byte NUL-terminated scale name buffer.
pub const MAX_SCALE_NAME_BYTES: usize = mts_esp_master_adapter::MAX_SCALE_NAME_BYTES;

/// Sanitize and bound an authored project/preset scale name for MTS-ESP publication.
/// Strips control/NUL characters, collapses/trims outer whitespace, truncates cleanly
/// on a UTF-8 character boundary to at most [`MAX_SCALE_NAME_BYTES`] (16) bytes, and
/// falls back to [`DEFAULT_SCALE_NAME`] if the result is empty.
pub fn sanitize_scale_name(raw: &str) -> String {
    let mut cleaned = String::with_capacity(MAX_SCALE_NAME_BYTES);
    let mut pending_space = false;
    for ch in raw.trim().chars() {
        if ch.is_control() {
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !cleaned.is_empty();
            continue;
        }
        let extra = usize::from(pending_space) + ch.len_utf8();
        if cleaned.len() + extra > MAX_SCALE_NAME_BYTES {
            break;
        }
        if pending_space {
            cleaned.push(' ');
            pending_space = false;
        }
        cleaned.push(ch);
    }
    if cleaned.is_empty() {
        DEFAULT_SCALE_NAME.to_owned()
    } else {
        cleaned
    }
}

#[derive(Clone, Copy)]
pub struct Publication {
    revision: Revision,
    payload: TuningPublication,
    name: [u8; MAX_SCALE_NAME_BYTES],
    name_len: u8,
}
impl Publication {
    pub fn new(revision: Revision, tuning: ProjectTuning) -> Result<Self, NativeError> {
        Self::new_named(revision, tuning, None)
    }

    /// Build one validated MTS-ESP publication with an optional authored scale/preset
    /// name. Names are sanitized and bounded to [`MAX_SCALE_NAME_BYTES`] via
    /// [`sanitize_scale_name`], falling back to [`DEFAULT_SCALE_NAME`] when absent/empty.
    pub fn new_named(
        revision: Revision,
        tuning: ProjectTuning,
        scale_name: Option<&str>,
    ) -> Result<Self, NativeError> {
        let scale = if tuning.enabled() {
            tuning.scale()
        } else {
            ProjectTuning::default().scale()
        };
        let metadata = ScaleMetadata::from_root(
            2.0_f64.powf(f64::from(scale.period_cents()) / 1200.0),
            scale.count() as u8,
            scale.root_key(),
            scale.root_degree() as u8,
        )?;
        let sanitized = scale_name
            .map(sanitize_scale_name)
            .unwrap_or_else(|| DEFAULT_SCALE_NAME.to_owned());
        let mut name = [0_u8; MAX_SCALE_NAME_BYTES];
        let bytes = sanitized.as_bytes();
        name[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            revision,
            payload: TuningPublication::new(tuning.frequencies(), &sanitized, metadata)?,
            name,
            name_len: bytes.len() as u8,
        })
    }

    /// Sanitized scale name encoded inside this publication's MTS-ESP payload.
    pub fn scale_name(&self) -> &str {
        std::str::from_utf8(&self.name[..usize::from(self.name_len)]).unwrap_or(DEFAULT_SCALE_NAME)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Pending,
    Disabling,
    Active,
    Blocked { recovery_available: bool },
    Unavailable(NativeError),
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub owner: Option<Owner>,
    pub desired: Option<Revision>,
    pub published: Option<Revision>,
    pub clients: u32,
    pub state: State,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            owner: None,
            desired: None,
            published: None,
            clients: 0,
            state: State::Disabled,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlError {
    Unavailable,
    Busy,
    Stale,
    Exhausted,
}
impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "mts publisher unavailable",
            Self::Busy => "another hex project owns mts",
            Self::Stale => "mts project publication is stale",
            Self::Exhausted => "mts publication identity exhausted",
        })
    }
}
impl std::error::Error for ControlError {}

#[derive(Default)]
struct Mailbox {
    snapshot: Snapshot,
    latest: Option<Publication>,
    epoch: u64,
    attempt: u64,
    recover: bool,
    next_client: u64,
    shutdown: bool,
    #[cfg(test)]
    last_idle_latest: Option<Revision>,
    #[cfg(test)]
    idle_passes: u64,
    #[cfg(test)]
    last_wait_unbounded: bool,
}
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    ready: Condvar,
    #[cfg(test)]
    idle: Condvar,
}
#[derive(Clone)]
pub struct Handle(Arc<Shared>);
impl Handle {
    pub fn new_client(&self) -> Result<ClientId, ControlError> {
        let mut state = self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?;
        state.next_client = state
            .next_client
            .checked_add(1)
            .ok_or(ControlError::Exhausted)?;
        Ok(ClientId(state.next_client))
    }
    pub fn snapshot(&self) -> Result<Snapshot, ControlError> {
        Ok(self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?
            .snapshot)
    }
    /// Explicit ownership acquisition. Focus and ordinary publication cannot claim it.
    pub fn enable(&self, publication: Publication) -> Result<(), ControlError> {
        let mut state = self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?;
        if state.shutdown {
            return Err(ControlError::Unavailable);
        }
        if state
            .snapshot
            .owner
            .is_some_and(|owner| owner != publication.revision.owner)
        {
            return Err(ControlError::Busy);
        }
        if publication.revision.serial == 0 {
            return Err(ControlError::Stale);
        }
        if state.snapshot.owner == Some(publication.revision.owner) {
            if state.snapshot.desired.is_some_and(|old| {
                publication.revision.serial <= old.serial
                    || publication.revision.project_revision < old.project_revision
            }) {
                return Err(ControlError::Stale);
            }
            state.latest = Some(publication);
            state.snapshot.desired = Some(publication.revision);
            match state.snapshot.state {
                State::Active | State::Pending => {
                    if state.snapshot.published != Some(publication.revision) {
                        state.snapshot.state = State::Pending;
                    }
                }
                State::Blocked { .. } | State::Unavailable(_) => {
                    state.attempt = state
                        .attempt
                        .checked_add(1)
                        .ok_or(ControlError::Exhausted)?;
                    state.recover = false;
                    state.snapshot.state = State::Pending;
                }
                State::Disabled | State::Disabling | State::Stopped => {
                    state.epoch = state.epoch.checked_add(1).ok_or(ControlError::Exhausted)?;
                    state.attempt = state
                        .attempt
                        .checked_add(1)
                        .ok_or(ControlError::Exhausted)?;
                    state.recover = false;
                    state.snapshot = Snapshot {
                        owner: Some(publication.revision.owner),
                        desired: Some(publication.revision),
                        state: State::Pending,
                        ..Snapshot::default()
                    };
                }
            }
            self.0.ready.notify_one();
            return Ok(());
        }
        let epoch = state.epoch.checked_add(1).ok_or(ControlError::Exhausted)?;
        let attempt = state
            .attempt
            .checked_add(1)
            .ok_or(ControlError::Exhausted)?;
        state.epoch = epoch;
        state.attempt = attempt;
        state.recover = false;
        state.snapshot = Snapshot {
            owner: Some(publication.revision.owner),
            desired: Some(publication.revision),
            state: State::Pending,
            ..Snapshot::default()
        };
        state.latest = Some(publication);
        self.0.ready.notify_one();
        Ok(())
    }
    /// One latest immutable snapshot; updates never replace reserved lifecycle state.
    pub fn publish(&self, publication: Publication) -> Result<(), ControlError> {
        let mut state = self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?;
        if state.shutdown
            || state.snapshot.owner != Some(publication.revision.owner)
            || state.snapshot.desired.is_some_and(|old| {
                publication.revision.serial <= old.serial
                    || publication.revision.project_revision < old.project_revision
            })
        {
            return Err(ControlError::Stale);
        }
        state.latest = Some(publication);
        state.snapshot.desired = Some(publication.revision);
        if matches!(state.snapshot.state, State::Active | State::Pending) {
            state.snapshot.state = State::Pending;
        }
        self.0.ready.notify_one();
        Ok(())
    }
    pub fn disable(&self, owner: Owner) -> Result<(), ControlError> {
        let mut state = self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?;
        if state.snapshot.owner != Some(owner) {
            return Err(ControlError::Stale);
        }
        state.epoch = state.epoch.checked_add(1).ok_or(ControlError::Exhausted)?;
        state.snapshot.owner = None;
        state.snapshot.desired = None;
        state.snapshot.state = State::Disabling;
        state.latest = None;
        self.0.ready.notify_one();
        Ok(())
    }
    pub fn retry(&self, owner: Owner) -> Result<(), ControlError> {
        self.request_attempt(owner, false)
    }
    /// Request a single shared-state reset after an explicit user action.
    /// Only the current owner of an IPC-blocked registration can request it.
    pub fn recover(&self, owner: Owner) -> Result<(), ControlError> {
        self.request_attempt(owner, true)
    }
    fn request_attempt(&self, owner: Owner, recover: bool) -> Result<(), ControlError> {
        let mut state = self
            .0
            .mailbox
            .lock()
            .map_err(|_| ControlError::Unavailable)?;
        if state.shutdown
            || state.snapshot.owner != Some(owner)
            || recover
                && state.snapshot.state
                    != (State::Blocked {
                        recovery_available: true,
                    })
        {
            return Err(ControlError::Stale);
        }
        state.attempt = state
            .attempt
            .checked_add(1)
            .ok_or(ControlError::Exhausted)?;
        state.recover = recover;
        state.snapshot.state = State::Pending;
        self.0.ready.notify_one();
        Ok(())
    }
    #[cfg(test)]
    fn wait_for_idle_revision(&self, revision: Revision) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut state = self.0.mailbox.lock().unwrap();
        while state.last_idle_latest != Some(revision) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for worker idle revision {revision:?}; observed {:?}",
                state.last_idle_latest
            );
            let (next, _) = self.0.idle.wait_timeout(state, remaining).unwrap();
            state = next;
        }
    }
    #[cfg(test)]
    fn wait_for_idle_pass(&self, min_passes: u64) -> (u64, bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut state = self.0.mailbox.lock().unwrap();
        while state.idle_passes < min_passes {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for idle pass >= {min_passes}; observed {}",
                state.idle_passes
            );
            let (next, _) = self.0.idle.wait_timeout(state, remaining).unwrap();
            state = next;
        }
        (state.idle_passes, state.last_wait_unbounded)
    }
}

pub struct Worker {
    handle: Handle,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start() -> std::io::Result<Self> {
        Self::start_with(transport::Native::default)
    }
    /// In-memory publisher for headless UI tests; never loads the system MTS library.
    #[cfg(feature = "test-support")]
    pub fn start_test_mock() -> std::io::Result<Self> {
        Self::start_with(transport::Memory::default)
    }
    fn start_with<T: Transport + 'static>(
        factory: impl FnOnce() -> T + Send + 'static,
    ) -> std::io::Result<Self> {
        let handle = Handle(Arc::default());
        let shared = Arc::clone(&handle.0);
        let thread = thread::Builder::new()
            .name("hex mts publisher".into())
            .spawn(move || run(&shared, factory()))?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
    pub fn shutdown(&mut self) {
        if let Ok(mut state) = self.handle.0.mailbox.lock() {
            state.shutdown = true;
            state.latest = None;
            self.handle.0.ready.notify_one();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(shared: &Shared, mut transport: impl Transport) {
    let mut active_epoch = None;
    let mut attempted = None;
    let mut published = None;
    loop {
        let (epoch, attempt, recover, owner, latest, shutdown) = {
            let Ok(state) = shared.mailbox.lock() else {
                transport.release();
                return;
            };
            (
                state.epoch,
                state.attempt,
                state.recover,
                state.snapshot.owner,
                state.latest,
                state.shutdown,
            )
        };
        if shutdown {
            transport.release();
            if let Ok(mut state) = shared.mailbox.lock() {
                state.snapshot = Snapshot {
                    state: State::Stopped,
                    ..Snapshot::default()
                };
            }
            return;
        }
        if active_epoch.is_some_and(|old| old != epoch) {
            transport.release();
            active_epoch = None;
            published = None;
        }
        if owner.is_none() {
            if let Ok(mut state) = shared.mailbox.lock()
                && state.epoch == epoch
                && state.snapshot.owner.is_none()
            {
                state.snapshot = Snapshot::default();
            }
        } else if let Some(publication) = latest {
            if active_epoch.is_none() && attempted != Some((epoch, attempt)) {
                attempted = Some((epoch, attempt));
                if recover
                    && !shared.mailbox.lock().is_ok_and(|state| {
                        state.epoch == epoch && state.attempt == attempt && !state.shutdown
                    })
                {
                    continue;
                }
                match transport.acquire(recover) {
                    Ok(()) => {
                        active_epoch = Some(epoch);
                        published = None;
                    }
                    Err(error) => {
                        if let Ok(mut state) = shared.mailbox.lock()
                            && state.epoch == epoch
                            && state.attempt == attempt
                            && !state.shutdown
                        {
                            state.snapshot.state = match error {
                                NativeError::ExistingMaster
                                | NativeError::ExistingMasterWithIpc => State::Blocked {
                                    recovery_available: error == NativeError::ExistingMasterWithIpc,
                                },
                                _ => State::Unavailable(error),
                            };
                        }
                    }
                }
            }
            if active_epoch == Some(epoch) {
                // A disable can race one in-flight native call. Check before it
                // and again before publishing its receipt; no stale receipt wins.
                let current = shared
                    .mailbox
                    .lock()
                    .is_ok_and(|state| state.epoch == epoch && !state.shutdown);
                if current {
                    if published != Some(publication.revision) {
                        transport.publish(&publication.payload);
                        published = Some(publication.revision);
                    }
                    match transport.clients() {
                        Ok(clients) => {
                            if let Ok(mut state) = shared.mailbox.lock()
                                && state.epoch == epoch
                                && state.snapshot.owner == owner
                                && !state.shutdown
                            {
                                state.snapshot.published = published;
                                state.snapshot.clients = clients;
                                state.snapshot.state = if state.snapshot.desired == published {
                                    State::Active
                                } else {
                                    State::Pending
                                };
                            }
                        }
                        Err(error) => {
                            transport.release();
                            active_epoch = None;
                            published = None;
                            if let Ok(mut state) = shared.mailbox.lock()
                                && state.epoch == epoch
                                && !state.shutdown
                            {
                                state.snapshot.state = State::Unavailable(error);
                                state.snapshot.published = None;
                                state.snapshot.clients = 0;
                            }
                        }
                    }
                }
            }
        }
        #[cfg(test)]
        let Ok(mut state) = shared.mailbox.lock() else {
            transport.release();
            return;
        };
        #[cfg(not(test))]
        let Ok(state) = shared.mailbox.lock() else {
            transport.release();
            return;
        };
        if state.shutdown
            || state.epoch != epoch
            || state.attempt != attempt
            || state.latest.map(|p| p.revision) != latest.map(|p| p.revision)
        {
            continue;
        }
        #[cfg(test)]
        {
            state.last_idle_latest = latest.map(|p| p.revision);
            state.idle_passes = state.idle_passes.saturating_add(1);
            state.last_wait_unbounded = active_epoch.is_none();
            shared.idle.notify_all();
        }
        let poisoned = if active_epoch.is_some() {
            shared
                .ready
                .wait_timeout(state, Duration::from_millis(100))
                .is_err()
        } else {
            shared.ready.wait(state).is_err()
        };
        if poisoned {
            transport.release();
            return;
        }
    }
}

#[cfg(test)]
mod tests;
