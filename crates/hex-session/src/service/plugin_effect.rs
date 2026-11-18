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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginEffectDestination {
    Track(hex_project::TrackId),
    Instrument(hex_project::TrackId),
    ReplaceInstrument {
        track: hex_project::TrackId,
        instrument: hex_project::DeviceId,
    },
    NewTrack {
        before_track: Option<hex_project::TrackId>,
        slot: hex_project::TrackDeviceSlot,
        gain: f32,
        /// The captured next identity. The worker checks it before native preparation.
        track_identity: u64,
    },
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
            Self::Instrument(_) | Self::ReplaceInstrument { .. } | Self::NewTrack { .. } => {
                Err("effect destination requires an existing chain".into())
            }
        }
    }
    pub fn is_instrument(self) -> bool {
        matches!(
            self,
            Self::Instrument(_)
                | Self::ReplaceInstrument { .. }
                | Self::NewTrack {
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    ..
                }
        )
    }
    pub fn owner(self) -> hex_engine::ProcessorChainOwner {
        match self {
            Self::Track(id) | Self::Instrument(id) | Self::ReplaceInstrument { track: id, .. } => {
                hex_engine::ProcessorChainOwner::Track(id.value())
            }
            Self::NewTrack { track_identity, .. } => {
                hex_engine::ProcessorChainOwner::Track(track_identity)
            }
            Self::Return(id) => hex_engine::ProcessorChainOwner::Return(id.value()),
            Self::Main => hex_engine::ProcessorChainOwner::Main,
        }
    }
}

/// Per-request intent and cancellation, following the existing export/import atomic
/// admission pattern. It owns no native token, descriptor, source or endpoint.
/// Cancellation can win until final publication admission begins.
#[derive(Debug, Default)]
pub struct PluginEffectControl {
    // 0 preparing, 1 cancelled, 2 admitting, 3 finished, 4 finished after cancellation.
    phase: AtomicU8,
    claimed: AtomicBool,
    open_editor: bool,
}
impl PluginEffectControl {
    pub fn opening_editor() -> Self {
        Self {
            open_editor: true,
            ..Self::default()
        }
    }

    pub fn opens_editor(&self) -> bool {
        self.open_editor
    }

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
    /// The catalog label for naming a newly created instrument track.
    pub track_device_name: Option<String>,
    pub control: Arc<PluginEffectControl>,
}
impl PluginEffectInsertion {
    pub fn validate(
        &self,
        context: hex_engine::AudioSourceContext,
        project: &Project,
    ) -> Result<(), String> {
        if self.context != context || self.control.phase.load(Ordering::Acquire) != 0 {
            return Err("vst3 insertion cancelled · source changed".into());
        }
        self.selection.validate()?;
        if let PluginEffectDestination::NewTrack {
            before_track,
            gain,
            track_identity,
            ..
        } = self.destination
        {
            if self.before.is_some()
                || before_track.is_some_and(|track| project.track(track).is_none())
                || track_identity != project.next_identity()
                || project.tracks().len() >= hex_project::MAX_TRACKS
                || !gain.is_finite()
                || !(0.0..=2.0).contains(&gain)
            {
                return Err("vst3 track insertion target is no longer available".into());
            }
            let name = self.track_device_name.as_deref().unwrap_or("vst3");
            if name.trim().is_empty()
                || name.len() > hex_project::MAX_NAME_BYTES
                || name.chars().any(char::is_control)
            {
                return Err("invalid vst3 track device name".into());
            }
            return Ok(());
        }
        if let PluginEffectDestination::Instrument(track)
        | PluginEffectDestination::ReplaceInstrument { track, .. } = self.destination
        {
            let track = project
                .track(track)
                .ok_or("instrument destination no longer exists")?;
            if track.is_group() || self.before.is_some() {
                return Err("select an ordinary track for the vst3 instrument".into());
            }
            let current = track.instrument().map(hex_project::Device::id);
            match self.destination {
                PluginEffectDestination::ReplaceInstrument { instrument, .. }
                    if current != Some(instrument) || !track.receives_source_audio() =>
                {
                    return Err("instrument replacement target changed".into());
                }
                PluginEffectDestination::Instrument(_) if current.is_some() => {
                    return Err("instrument slot is already occupied".into());
                }
                _ => {}
            }
            if track
                .source_id()
                .and_then(|id| project.native_source(id))
                .is_some_and(|source| !source.post_instrument_effects().is_empty())
            {
                return Err("vst3 instruments cannot host effects inside the source chain".into());
            }
            return Ok(());
        }
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
            PluginEffectDestination::Instrument(track) => {
                Command::InsertInstrument { track, state }
            }
            PluginEffectDestination::ReplaceInstrument { track, instrument } => {
                Command::ReplaceTrackInstrument {
                    track,
                    replacing: instrument,
                    state,
                }
            }
            PluginEffectDestination::NewTrack {
                before_track,
                slot,
                gain,
                ..
            } => super::device_track_command(
                before_track,
                slot,
                state,
                self.track_device_name
                    .clone()
                    .unwrap_or_else(|| "vst3".into()),
                Some(gain),
            ),
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
    next_identity: u64,
    native_started: bool,
}
impl Pending {
    pub fn new(insertion: PluginEffectInsertion, project: &Project) -> Result<Self, String> {
        if insertion.control.claimed.swap(true, Ordering::AcqRel) {
            return Err("vst3 insertion request was already submitted".into());
        }
        Ok(Self {
            insertion,
            next_identity: project.next_identity(),
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
            || session.project().next_identity() != self.next_identity
            || !sink.resident_publication_enabled()
        {
            return Err("vst3 insertion requires its original resident audio context".into());
        }
        // The new-track command allocates the track before its device.
        let device_id = self
            .next_identity
            .checked_add(u64::from(matches!(
                self.insertion.destination,
                PluginEffectDestination::NewTrack { .. }
            )))
            .ok_or("project id space exhausted")?;
        self.native_started = true;
        sink.prepare_plugin_effect(PluginEffectPreparation {
            insertion: &self.insertion,
            project: session.project(),
            prepared,
            device_id,
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
