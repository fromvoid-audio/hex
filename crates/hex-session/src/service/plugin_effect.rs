//! Canonical worker's explicit plugin insertion request, not native custody.
use super::*;
pub use hex_lattice_contract::NativeDeviceState;
use std::{path::Component, sync::atomic::AtomicU8};

/// Copied catalog identity. Filesystem, helper and native class revalidation
/// belong to the injected audio owner before it returns a captured state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginEffectSelection {
    pub module: PathBuf,
    pub executable: String,
    pub cid: [u8; 16],
    pub executable_sha256: String,
    pub info_sha256: String,
}
impl PluginEffectSelection {
    pub fn validate(&self) -> Result<(), String> {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if !self.module.is_absolute()
            || self.module.as_os_str().len() > 4096
            || self.module.as_os_str().as_encoded_bytes().contains(&0)
            || self.module.components().count() > 64
            || self
                .module
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            || self.module.extension().and_then(|s| s.to_str()) != Some("vst3")
            || self.cid == [0; 16]
            || !digest(&self.executable_sha256)
            || !digest(&self.info_sha256)
            || self.executable.is_empty()
            || self.executable.len() > 4096
            || Path::new(&self.executable).is_absolute()
            || self
                .executable
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == ".." || s.contains('\0'))
        {
            return Err("invalid selected vst3 source metadata".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEffectDestination {
    Track(hex_project::TrackId),
    Return(hex_project::ReturnId),
    Main,
}
impl PluginEffectDestination {
    fn devices(self, project: &Project) -> Result<&[hex_project::Device], String> {
        match self {
            Self::Track(id) => project
                .track(id)
                .map(|track| track.devices())
                .ok_or_else(|| "effect destination no longer exists".into()),
            Self::Return(id) => project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .map(|track| track.devices())
                .ok_or_else(|| "effect destination no longer exists".into()),
            Self::Main => Ok(project.main_devices()),
        }
    }
    pub fn owner(self) -> hex_engine::ProcessorChainOwner {
        match self {
            Self::Track(id) => hex_engine::ProcessorChainOwner::Track(id.value()),
            Self::Return(id) => hex_engine::ProcessorChainOwner::Return(id.value()),
            Self::Main => hex_engine::ProcessorChainOwner::Main,
        }
    }
}

/// Per-request cancellation only, following the existing export/import atomic
/// admission pattern. It owns no native token, descriptor, source or endpoint.
/// Cancellation can win until final publication admission begins.
#[derive(Debug, Default)]
pub struct PluginEffectControl {
    // 0 preparing, 1 cancelled, 2 admitting, 3 finished, 4 finished after cancellation.
    phase: AtomicU8,
    claimed: AtomicBool,
}
impl PluginEffectControl {
    pub fn cancel(&self) -> bool {
        matches!(
            self.phase
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(1)
        )
    }
    pub fn is_cancelled(&self) -> bool {
        matches!(self.phase.load(Ordering::Acquire), 1 | 4)
    }
    pub(super) fn begin_commit(&self) -> bool {
        self.phase
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn finish(&self) {
        let _ = self
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                Some(if phase == 1 { 4 } else { 3 })
            });
    }
}

#[derive(Clone, Debug)]
pub struct PluginEffectInsertion {
    pub context: hex_engine::AudioSourceContext,
    pub selection: PluginEffectSelection,
    pub destination: PluginEffectDestination,
    pub before: Option<hex_project::DeviceId>,
    pub control: Arc<PluginEffectControl>,
}
impl PluginEffectInsertion {
    pub fn validate(
        &self,
        context: hex_engine::AudioSourceContext,
        project: &Project,
    ) -> Result<(), String> {
        if self.context != context || self.control.phase.load(Ordering::Acquire) != 0 {
            return Err("vst3 effect insertion cancelled · source changed".into());
        }
        self.selection.validate()?;
        let devices = self.destination.devices(project)?;
        if self
            .before
            .is_some_and(|id| !devices.iter().any(|device| device.id() == id))
        {
            return Err("effect insertion anchor no longer belongs to its destination".into());
        }
        Ok(())
    }
    pub(super) fn command(
        &self,
        state: hex_lattice_contract::NativeDeviceState,
    ) -> Result<Command, String> {
        if state.type_id() != hex_lattice_contract::DeviceTypeId::from_static("hex.vst3") {
            return Err("native preparation returned a different device provider".into());
        }
        Ok(match self.destination {
            PluginEffectDestination::Track(track) => Command::InsertDevice {
                track,
                before: self.before,
                state,
            },
            PluginEffectDestination::Return(return_track) => Command::InsertReturnDevice {
                return_track,
                before: self.before,
                state,
            },
            PluginEffectDestination::Main => Command::InsertMainDevice {
                before: self.before,
                state,
            },
        })
    }
}

/// Synchronous worker borrow. The planned ID is allocated only by the eventual
/// canonical command; stale revisions must be cancelled before that command.
pub struct PluginEffectPreparation<'a> {
    pub insertion: &'a PluginEffectInsertion,
    pub project: &'a Project,
    pub prepared: Option<&'a PreparedRevision>,
    pub device_id: u64,
}
pub enum PluginEffectPreparationState {
    Preparing,
    /// Actual native facts/presentation/ACK and callback endpoint installation
    /// precede this state. The graph still requires normal admission/commit.
    Ready(hex_lattice_contract::NativeDeviceState),
}

/// Exact request terminal, delivered with the canonical view after commit or
/// refusal. A consumed FIFO request alone does not complete native preparation.
#[derive(Clone, Debug)]
pub struct PluginEffectCompletion {
    pub control: Arc<PluginEffectControl>,
    pub accepted: bool,
}

/// The canonical worker retains at most one preparation. Native ownership stays
/// with the sink through its matching finish/cancel hook, including failures.
pub(super) struct Pending {
    pub insertion: PluginEffectInsertion,
    device_id: u64,
    native_started: bool,
}
impl Pending {
    pub fn new(insertion: PluginEffectInsertion, project: &Project) -> Result<Self, String> {
        if insertion.control.claimed.swap(true, Ordering::AcqRel) {
            return Err("vst3 effect request was already submitted".into());
        }
        Ok(Self {
            insertion,
            device_id: project.next_identity(),
            native_started: false,
        })
    }
    pub fn prepare(
        &mut self,
        session: &Session,
        context: hex_engine::AudioSourceContext,
        prepared: Option<&PreparedRevision>,
        sink: &mut dyn WorkerAudioSink,
    ) -> Result<PluginEffectPreparationState, String> {
        self.insertion.validate(context, session.project())?;
        if session.audio_mode() != DocumentAudioMode::Resident
            || session.project().next_identity() != self.device_id
            || !sink.resident_publication_enabled()
        {
            return Err(
                "vst3 effect insertion requires its original resident audio context".into(),
            );
        }
        self.native_started = true;
        sink.prepare_plugin_effect(PluginEffectPreparation {
            insertion: &self.insertion,
            project: session.project(),
            prepared,
            device_id: self.device_id,
        })
    }
    pub fn finish(
        self,
        sink: Option<&mut (dyn WorkerAudioSink + 'static)>,
        accepted: bool,
    ) -> PluginEffectCompletion {
        if self.native_started
            && let Some(sink) = sink
        {
            sink.finish_plugin_effect(&self.insertion.control, accepted);
        }
        self.insertion.control.finish();
        PluginEffectCompletion {
            control: self.insertion.control,
            accepted,
        }
    }
}
