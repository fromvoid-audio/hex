//! Canonical worker's explicit plugin insertion request, not native custody.
use super::*;
pub use hex_lattice_contract::NativeDeviceState;
use std::{
    path::Component,
    sync::{Mutex, PoisonError, atomic::AtomicU8},
};

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
    /// The captured direct hosted leaf on one ordinary Resident track.
    ReplaceTrackEffect {
        track: hex_project::TrackId,
        replacing: hex_project::DeviceId,
    },
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
    ReplaceMainEffect {
        replacing: hex_project::DeviceId,
    },
    ReplaceReturnEffect {
        return_track: hex_project::ReturnId,
        replacing: hex_project::DeviceId,
    },
}
impl PluginEffectDestination {
    fn devices(self, project: &Project) -> Result<&[hex_project::Device], String> {
        match self {
            Self::Track(id) => project
                .track(id)
                .map(|track| track.devices())
                .ok_or_else(|| "effect destination no longer exists".into()),
            Self::ReplaceTrackEffect { track, .. } => project
                .track(track)
                .filter(|target| !target.is_group())
                .map(hex_project::Track::devices)
                .ok_or_else(|| "effect replacement requires an ordinary track".into()),
            Self::Return(id)
            | Self::ReplaceReturnEffect {
                return_track: id, ..
            } => project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .map(|track| track.devices())
                .ok_or_else(|| "effect destination no longer exists".into()),
            Self::Main | Self::ReplaceMainEffect { .. } => Ok(project.main_devices()),
            Self::Instrument(_) | Self::ReplaceInstrument { .. } | Self::NewTrack { .. } => {
                Err("effect destination requires an existing chain".into())
            }
        }
    }

    fn replaced_effect(self) -> Option<hex_project::DeviceId> {
        match self {
            Self::ReplaceTrackEffect { replacing, .. }
            | Self::ReplaceMainEffect { replacing }
            | Self::ReplaceReturnEffect { replacing, .. } => Some(replacing),
            _ => None,
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
            Self::Track(id)
            | Self::ReplaceTrackEffect { track: id, .. }
            | Self::Instrument(id)
            | Self::ReplaceInstrument { track: id, .. } => {
                hex_engine::ProcessorChainOwner::Track(id.value())
            }
            Self::NewTrack { track_identity, .. } => {
                hex_engine::ProcessorChainOwner::Track(track_identity)
            }
            Self::Return(id)
            | Self::ReplaceReturnEffect {
                return_track: id, ..
            } => hex_engine::ProcessorChainOwner::Return(id.value()),
            Self::Main | Self::ReplaceMainEffect { .. } => hex_engine::ProcessorChainOwner::Main,
        }
    }
}

/// Per-request cancellation for hosted and built-in insertions. It owns no
/// native token, descriptor, source or endpoint.
/// Cancellation can win until final publication admission begins.
#[derive(Debug, Default)]
pub struct PluginEffectControl {
    // 0 preparing, 1 cancelled, 2 admitting, 3 finished, 4 finished after cancellation.
    phase: AtomicU8,
    claimed: AtomicBool,
    open_editor: bool,
    // Held only by editor/worker control code during the bounded receipt update.
    install_receipt: Mutex<Option<Box<dyn DeviceInsertAudioReceipt>>>,
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

    pub(super) fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    pub fn cancel(&self) -> bool {
        let receipt = self
            .install_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(receipt) = receipt.as_ref() {
            return match self.phase.load(Ordering::Acquire) {
                1 => true,
                0 if receipt.cancel() => {
                    self.phase.store(1, Ordering::Release);
                    true
                }
                _ => false,
            };
        }
        matches!(
            self.phase
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(1)
        )
    }

    /// Bind the exact receipt before the audio owner makes its graph visible.
    /// A prior cancellation must fence this graph before it reaches the callback.
    pub fn bind_device_insert_receipt(&self, receipt: Box<dyn DeviceInsertAudioReceipt>) {
        let mut bound = self
            .install_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.phase.load(Ordering::Acquire) != 0 || bound.is_some() {
            receipt.cancel();
        } else {
            *bound = Some(receipt);
        }
    }

    /// Free the exact refused receipt so the confirmed successor can bind.
    /// Until then the same prepared SDK operation remains live and cancellable.
    pub(super) fn release_refused_insert_receipt(&self, refused: ResidentGraphSubmission) -> bool {
        let mut bound = self
            .install_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.phase.load(Ordering::Acquire) != 0
            || !bound.as_ref().is_some_and(|receipt| {
                receipt.submission() == refused
                    && receipt.state()
                        == DeviceInsertAudioState::Refused(
                            hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
                        )
            })
        {
            return false;
        }
        *bound = None;
        true
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
        let mut receipt = self
            .install_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                Some(if phase == 1 { 4 } else { 3 })
            });
        *receipt = None;
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
        if let Some(replacing) = self.destination.replaced_effect()
            && (self.before.is_some()
                || !devices.iter().any(|device| {
                    device.id() == replacing
                        && device.parent().is_none()
                        && device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID
                }))
        {
            return Err("vst3 effect replacement target changed".into());
        }
        if let PluginEffectDestination::ReplaceTrackEffect { replacing, .. } = self.destination
            && devices
                .iter()
                .any(|device| device.parent() == Some(replacing))
        {
            return Err("vst3 effect replacement requires a single effect".into());
        }
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
            PluginEffectDestination::ReplaceTrackEffect { track, replacing } => {
                Command::ReplaceTrackDevice {
                    track,
                    replacing,
                    state,
                }
            }
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
            PluginEffectDestination::ReplaceMainEffect { replacing } => {
                Command::ReplaceMainDevice { replacing, state }
            }
            PluginEffectDestination::ReplaceReturnEffect {
                return_track,
                replacing,
            } => Command::ReplaceReturnDevice {
                return_track,
                replacing,
                state,
            },
        })
    }
}

/// Synchronous worker borrow. The original insertion identifies native
/// preparation. The validated canonical context identifies the predecessor for
/// publication after any accepted independent EQ write. The eventual command
/// allocates the planned ID.
pub struct PluginEffectPreparation<'a> {
    pub insertion: &'a PluginEffectInsertion,
    canonical_context: hex_engine::AudioSourceContext,
    pub project: &'a Project,
    pub prepared: Option<&'a PreparedRevision>,
    pub device_id: u64,
}
impl PluginEffectPreparation<'_> {
    pub fn canonical_context(&self) -> hex_engine::AudioSourceContext {
        self.canonical_context
    }
}
pub enum PluginEffectPreparationState {
    Preparing,
    /// Actual native facts/presentation/ACK and callback endpoint installation
    /// precede this state. The graph still requires normal admission/commit.
    Ready(hex_lattice_contract::NativeDeviceState),
}

/// Exact insertion terminal, delivered with the canonical view after commit
/// or refusal. FIFO consumption alone does not complete device preparation.
#[derive(Clone, Debug)]
pub struct PluginEffectCompletion {
    pub control: Arc<PluginEffectControl>,
    pub accepted: bool,
}

impl PluginEffectCompletion {
    pub(super) fn finish(control: Arc<PluginEffectControl>, accepted: bool) -> Self {
        control.finish();
        Self { control, accepted }
    }
}

/// The canonical worker retains at most one preparation. Native ownership stays
/// with the sink through its matching finish/cancel hook, including failures.
pub(super) struct Pending {
    pub insertion: PluginEffectInsertion,
    canonical_context: hex_engine::AudioSourceContext,
    next_identity: u64,
    native_started: bool,
    rebased_at: Option<std::time::Instant>,
}
impl Pending {
    pub fn new(insertion: PluginEffectInsertion, project: &Project) -> Result<Self, String> {
        if !insertion.control.claim() {
            return Err("vst3 insertion request was already submitted".into());
        }
        Ok(Self {
            canonical_context: insertion.context,
            insertion,
            next_identity: project.next_identity(),
            native_started: false,
            rebased_at: None,
        })
    }
    pub(super) fn rebased_predecessor_ready(
        &self,
        sink: &mut dyn WorkerAudioSink,
    ) -> Result<bool, &'static str> {
        let Some(rebased_at) = self.rebased_at else {
            return Ok(true);
        };
        let ready = sink.verified_hosted_predecessor_ready(self.canonical_context)?;
        if !ready && rebased_at.elapsed() >= super::publication_wait::DEADLINE {
            return Err("vst3 insertion cancelled · accepted audio publication timed out");
        }
        Ok(ready)
    }

    pub fn matches_context(&self, context: hex_engine::AudioSourceContext) -> bool {
        self.canonical_context == context
    }

    pub fn validate(
        &self,
        context: hex_engine::AudioSourceContext,
        project: &Project,
    ) -> Result<(), String> {
        if !self.matches_context(context) {
            return Err("vst3 insertion cancelled · source changed".into());
        }
        self.insertion.validate(self.insertion.context, project)
    }

    /// Existing scalar, paired and grouped EQ parameter gestures can publish
    /// an overlay while the same native effect endpoint prepares. Node creation
    /// retains the ordinary topology conflict rule.
    pub fn independent_eq_gesture(
        &self,
        project: &Project,
        target: crate::device_gesture::GestureTarget,
    ) -> Option<hex_project::DeviceId> {
        use crate::device_gesture::GestureTarget;
        let device = match target {
            GestureTarget::DeviceParameter { device, .. }
            | GestureTarget::DeviceParameterPair { device, .. }
            | GestureTarget::DeviceParameterSnapshot { device, .. } => device,
            _ => return None,
        };
        self.independent_eq_device(project, device)
            .then_some(device)
    }

    fn independent_eq_device(&self, project: &Project, device: hex_project::DeviceId) -> bool {
        self.insertion.destination.devices(project).is_ok()
            && project.next_identity() == self.next_identity
            && project.device(device).is_some_and(|device| {
                device.state().type_id()
                    == hex_lattice_contract::DeviceTypeId::from_static("hex.eq")
            })
    }

    /// Call only after the worker commits an existing EQ parameter edit.
    /// Parameter read selection can change without changing the destination,
    /// insertion anchor or reserved device identity.
    pub fn rebase_after_accepted_eq_write(
        &mut self,
        predecessor: hex_engine::AudioSourceContext,
        project: &Project,
        device: hex_project::DeviceId,
    ) {
        if self.validate(predecessor, project).is_ok()
            && self.independent_eq_device(project, device)
            && let Some(editor_revision) = predecessor.editor_revision.checked_add(1)
        {
            self.canonical_context = hex_engine::AudioSourceContext {
                editor_revision,
                ..predecessor
            };
            self.rebased_at = Some(std::time::Instant::now());
        }
    }

    pub fn prepare(
        &mut self,
        session: &Session,
        context: hex_engine::AudioSourceContext,
        prepared: Option<&PreparedRevision>,
        sink: &mut dyn WorkerAudioSink,
    ) -> Result<PluginEffectPreparationState, String> {
        self.validate(context, session.project())?;
        let replaces_effect = self.insertion.destination.replaced_effect().is_some();
        let supported = match session.audio_mode() {
            DocumentAudioMode::Resident => {
                sink.resident_publication_enabled()
                    && (!replaces_effect || sink.confirms_device_inserts())
            }
            DocumentAudioMode::VerifiedStreaming => {
                !replaces_effect
                    && self
                        .insertion
                        .destination
                        .devices(session.project())
                        .is_ok()
                    && sink.confirms_verified_device_inserts()
            }
        };
        if !supported || session.project().next_identity() != self.next_identity {
            return Err("vst3 insertion requires its original supported audio context".into());
        }
        // The new-track command allocates the track before its device.
        let device_id = self
            .next_identity
            .checked_add(u64::from(matches!(
                self.insertion.destination,
                PluginEffectDestination::NewTrack { .. }
            )))
            .ok_or("project id space exhausted")?;
        if !self.rebased_predecessor_ready(sink)? {
            return Ok(PluginEffectPreparationState::Preparing);
        }
        self.native_started = true;
        sink.prepare_plugin_effect(PluginEffectPreparation {
            insertion: &self.insertion,
            canonical_context: context,
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
        PluginEffectCompletion::finish(self.insertion.control, accepted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::VST3_DEVICE_TYPE_ID;

    fn replacement_request(
        context: hex_engine::AudioSourceContext,
        destination: PluginEffectDestination,
    ) -> PluginEffectInsertion {
        PluginEffectInsertion {
            context,
            selection: PluginEffectSelection {
                module: PathBuf::from("/fixtures/effect.vst3"),
                executable: "Contents/MacOS/effect".into(),
                cid: [1; 16],
                executable_sha256: "a".repeat(64),
                info_sha256: "b".repeat(64),
            },
            destination,
            before: None,
            track_device_name: None,
            control: Arc::default(),
        }
    }

    #[test]
    fn main_effect_replacement_validates_the_old_slot_and_builds_a_fresh_edit() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let state = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap();
        let hex_project::Created::Device(replacing) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("main effect");
        };
        let context = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 2,
        };
        let mut request = replacement_request(
            context,
            PluginEffectDestination::ReplaceMainEffect { replacing },
        );
        request.validate(context, editor.project()).unwrap();
        let command = request.command(state.clone()).unwrap();
        assert!(
            matches!(command, Command::ReplaceMainDevice { replacing: id, .. } if id == replacing)
        );
        request.before = Some(replacing);
        assert!(request.validate(context, editor.project()).is_err());
        request.before = None;
        editor.execute(command).unwrap();
        assert!(request.validate(context, editor.project()).is_err());
    }

    #[test]
    fn track_effect_replacement_requires_the_exact_direct_hosted_leaf() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 12).unwrap();
        let mut add_track = |name: &str| {
            let hex_project::Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            id
        };
        let target = add_track("target");
        let other = add_track("other");
        let state = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap();
        let insert = |editor: &mut hex_project::Editor, state| {
            let hex_project::Created::Device(id) = editor
                .execute(Command::InsertDevice {
                    track: target,
                    before: None,
                    state,
                })
                .unwrap()
            else {
                panic!("track effect");
            };
            id
        };
        let replacing = insert(&mut editor, state.clone());
        let parent = insert(&mut editor, state.clone());
        let hex_project::Created::Device(child) = editor
            .execute(Command::InsertNestedDevice {
                parent,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("nested track effect");
        };
        let context = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 2,
        };
        let destination =
            |track, replacing| PluginEffectDestination::ReplaceTrackEffect { track, replacing };
        let mut request = replacement_request(context, destination(target, replacing));
        for invalid in [
            destination(other, replacing),
            destination(target, parent),
            destination(target, child),
        ] {
            request.destination = invalid;
            assert!(request.validate(context, editor.project()).is_err());
        }
        request.destination = destination(target, replacing);
        request.validate(context, editor.project()).unwrap();
        assert_eq!(
            request.destination.owner(),
            hex_engine::ProcessorChainOwner::Track(target.value())
        );
        let command = request.command(state).unwrap();
        assert!(
            matches!(command, Command::ReplaceTrackDevice { track, replacing: id, .. }
            if track == target && id == replacing)
        );
        editor.execute(command).unwrap();
        assert!(request.validate(context, editor.project()).is_err());
    }

    #[test]
    fn return_effect_replacement_requires_the_exact_owner_and_device() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut add_return = |name: &str| {
            let hex_project::Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: name.into() })
                .unwrap()
            else {
                panic!("return track");
            };
            id
        };
        let target = add_return("wet");
        let other = add_return("other");
        let state = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap();
        let hex_project::Created::Device(replacing) = editor
            .execute(Command::InsertReturnDevice {
                return_track: target,
                before: None,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("return effect");
        };
        let context = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 2,
        };
        let correct = PluginEffectDestination::ReplaceReturnEffect {
            return_track: target,
            replacing,
        };
        let mut request = replacement_request(
            context,
            PluginEffectDestination::ReplaceReturnEffect {
                return_track: other,
                replacing,
            },
        );
        assert!(request.validate(context, editor.project()).is_err());
        request.destination = correct;
        request.validate(context, editor.project()).unwrap();
        assert_eq!(
            request.destination.owner(),
            hex_engine::ProcessorChainOwner::Return(target.value())
        );
        let command = request.command(state).unwrap();
        assert!(
            matches!(command, Command::ReplaceReturnDevice { return_track, replacing: id, .. }
            if return_track == target && id == replacing)
        );
        editor.execute(command).unwrap();
        assert!(request.validate(context, editor.project()).is_err());
    }
}
