//! One live VST3 gesture on the exact original output and source. The caller
//! keeps canonical history and the actual native source stays in Root.
use super::*;
use hex_audio::{
    plugin_effects::ManualBaseCapture,
    plugin_worker::{
        NativeParameterHold, NativeWorkerCapture, NativeWorkerCaptureProgress,
        NativeWorkerParameterChange, NativeWorkerParameterProgress,
        NativeWorkerSequencedCaptureProgress, NativeWorkerStateAfterParameter,
    },
    prepared::{AppliedAudioSource, PublicationControl},
};
use hex_session::{
    device_gesture::{
        GestureKey, GestureSnapshot, GestureTarget, PluginGesturePreparation,
        PluginParameterOrigin, PluginParameterValue,
    },
    service::{
        PluginParameterCapture, PluginParameterGestureError as Error, PluginParameterGestureInput,
        PluginParameterGestureResult as Prepared, PluginParameterOverride, RequestCancellation,
    },
};
use std::sync::atomic::Ordering::Acquire;
use std::time::{Duration, Instant};

mod delivery;
mod manual_override;
pub(super) mod metadata;
use manual_override::LiveOverride;

struct PendingPoint {
    token: NativeWorkerParameterChange,
    value: PluginParameterValue,
    sequence: u64,
    opening_hold: bool,
    delivery: Option<delivery::PendingDelivery>,
    processed: bool,
    sdk_finished: bool,
    #[cfg(all(test, feature = "allocation-probe"))]
    hold_sdk_ack: bool,
}

pub(super) struct Gesture {
    key: GestureKey,
    origin: PluginParameterOrigin,
    pub(super) binding: Binding,
    output: u64,
    steps: u32,
    last: PluginParameterValue,
    sequence: u64,
    pending: Option<PendingPoint>,
    live: Option<LiveOverride>,
    capture: Option<NativeWorkerCapture>,
    capture_sequenced: bool,
    last_input: Option<NativeWorkerParameterChange>,
    last_input_idle: bool,
    sequenced_capture: Option<NativeWorkerStateAfterParameter>,
    before: Option<NativeDeviceState>,
    pub(super) commit: Option<(AudioSourceContext, u64)>,
    pub(super) next_program: Option<std::num::NonZeroU64>,
    commit_accepted: bool,
    capture_class: Option<[u8; 16]>,
    deliveries: Vec<hex_session::service::PluginParameterObservation>,
    retired_delivery: Option<hex_audio::foreign_parameter_delivery::Retired>,
    recording: bool,
    recording_base: Option<PluginParameterValue>,
    recorded_release: bool,
    recording_context: Option<delivery::Context>,
    recording_closed: bool,
    recording_interrupted: bool,
}

fn refused(reason: impl Into<String>) -> Error {
    Error::Refused(reason.into())
}
fn lost() -> Error {
    Error::SourceLost("vst3 parameter source changed".into())
}

/// Stopped unlaned instruments retain full-state capture. Running instruments
/// need isolation even before their first lane, to preserve the original MIDI
/// worker. Explicitly recorded effects also isolate their live worker.
/// Saved identity chooses the mode; SDK/Root still admit the target.
fn isolation_class(
    input: &PluginParameterGestureInput<'_>,
    playing: bool,
) -> Result<Option<[u8; 16]>, Error> {
    if let Some(class) = input.isolated_class() {
        Ok(Some(class))
    } else if (input.is_instrument() || input.records_touch()) && playing {
        Hosting::saved_identity_on_control(input.canonical_state())
            .map(|identity| Some(identity.class.bytes()))
            .map_err(refused)
    } else {
        Ok(None)
    }
}

fn admit<T>(
    control: &mut PublicationControl,
    mut begin: impl FnMut(&mut PublicationControl) -> Result<T, &'static str>,
) -> Result<T, Error> {
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        match begin(control) {
            Err(
                "native preparation is busy"
                | "native parameter mailbox is busy"
                | "native capture mailbox is busy"
                | "native controller mailbox is busy"
                | "native controller query is already pending"
                | "native source is busy",
            ) if Instant::now() < deadline => edits::poll_control(control),
            Err(reason) if source_gone(reason) => return Err(lost()),
            result => return result.map_err(refused),
        }
    }
}

impl Plugins {
    /// Borrow the pending gesture and the still-canonical predecessor for one
    /// prospective publication. Neither borrowed state transfers source custody.
    pub(in crate::native_audio) fn sequenced_publication_request<'a>(
        &'a self,
        control: Option<&PublicationControl>,
        desired: Option<&'a CanonicalAudioSourceSnapshot>,
        successor: AudioSourceContext,
        playback: &hex_engine::playback::Playback,
    ) -> Result<Option<hex_audio::prepared::NativeParameterContinuationRequest<'a>>, String> {
        let Some(gesture) = self.parameter_gesture.as_ref() else {
            return Ok(None);
        };
        let Some((predecessor, sequence)) = gesture.commit else {
            return Ok(None);
        };
        let Some(fact) = gesture.sequenced_capture.as_ref() else {
            return Ok(None);
        };
        if gesture.live.is_some()
            || gesture.recording
            || sequence != gesture.sequence
            || successor.project_generation != predecessor.project_generation
            || predecessor.editor_revision.checked_add(1) != Some(successor.editor_revision)
        {
            return Ok(None);
        }
        let Some(node) = playback.foreign_nodes_on_control().find(|node| {
            node.device_id == gesture.origin.device.value()
                && node.configuration.as_deref().is_some_and(|state| {
                    state.shares_payload_storage_with(fact.state()) && *state == *fact.state()
                })
        }) else {
            return Ok(None);
        };
        if !playback.has_foreign_binding_on_control(node.owner, node.device_id, gesture.binding) {
            return Ok(None);
        }
        let desired = desired
            .filter(|snapshot| snapshot.context() == predecessor)
            .ok_or("vst3 continuation predecessor is unavailable")?;
        let mut previous = None;
        desired
            .effect_preparation()
            .visit_plugin_devices(|destination, device, state| {
                if destination.owner() == node.owner && device == node.device_id {
                    previous = Some(state);
                }
                Ok(())
            })?;
        let previous = previous.ok_or("vst3 continuation predecessor differs")?;
        let control = control.ok_or("vst3 continuation output unavailable")?;
        let playing = control.status.playing.load(Acquire);
        let (source, transport_epoch) = control
            .status
            .native_parameter_source(playing)
            .filter(|(source, _)| {
                source.context == predecessor && source.output_identity == gesture.output
            })
            .ok_or("vst3 continuation source changed")?;
        let identity =
            hex_audio::prepared::NativeProcessorIdentity::new(node.owner, node.device_id)
                .map_err(|_| "vst3 continuation identity differs")?;
        Ok(Some(
            hex_audio::prepared::NativeParameterContinuationRequest {
                source,
                transport_epoch,
                playing,
                successor,
                identity,
                binding: gesture.binding,
                previous_state: previous,
                capture: fact,
                parameter: gesture.origin.parameter,
                normalized: gesture.sdk_value(gesture.last),
            },
        ))
    }

    pub(in crate::native_audio) fn parameter_requires_restore(
        &mut self,
        control: &mut PublicationControl,
        context: AudioSourceContext,
        device: u64,
    ) -> Result<bool, String> {
        if self
            .restore
            .as_ref()
            .is_some_and(super::restore::Restore::submitted)
        {
            self.poll_restore(control, Some(context));
        }
        if self.restore.is_some() {
            return Err("wait for stopped vst3 restoration".into());
        }
        if control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(false);
        }
        let source = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.project_generation == context.project_generation
                    && source.device == device
            })
            .ok_or("vst3 parameter source changed")?;
        if control
            .status
            .recording_source()
            .is_none_or(|source| source.context != context)
        {
            return Err("vst3 parameter source changed".into());
        }
        Ok(control
            .native_source_endpoint_status(source.binding)
            .is_some_and(|(_, live)| !live))
    }
    pub(in crate::native_audio) fn parameter_gesture(
        &mut self,
        control: &mut PublicationControl,
        input: PluginParameterGestureInput<'_>,
        cancellation: Option<&RequestCancellation>,
        clock: Option<(u32, hex_session::service::ResidentRuntimeLoop)>,
    ) -> Result<Prepared, Error> {
        if input.requires_delivery_confirmation() {
            self.confirmed_parameters
                .try_reserve(2)
                .map_err(|_| refused("could not reserve confirmed vst3 observations"))?;
        }
        if !self.confirmed_parameters.is_empty()
            && !matches!(input.operation(), PluginGesturePreparation::Restore { .. })
        {
            return Err(refused("retain confirmed vst3 points before another input"));
        }
        if let PluginGesturePreparation::Begin {
            origin, snapshot, ..
        } = input.operation()
        {
            let restored_revision = self
                .restore
                .as_ref()
                .and_then(super::restore::Restore::submitted_revision);
            let deadline = Instant::now() + Duration::from_secs(3);
            while self
                .restore
                .as_ref()
                .is_some_and(super::restore::Restore::submitted)
            {
                self.poll_restore(control, Some(input.context()));
                if self.restore.is_none() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(refused("stopped vst3 restoration has not been installed"));
                }
                edits::poll_control(control);
            }
            if restored_revision.is_some_and(|revision| {
                control
                    .status
                    .applied_revision
                    .load(std::sync::atomic::Ordering::Acquire)
                    != revision
            }) {
                return Err(refused("stopped vst3 restoration was refused"));
            }
            if self.parameter_gesture.is_some() || self.restore.is_some() {
                return Err(refused("vst3 controller is busy"));
            }
            self.yield_parameters_to_control(control, input.context(), cancellation)
                .map_err(refused)?;
            let inventory = self
                .observed_parameters
                .iter()
                .find(|inventory| {
                    inventory.query.context == origin.context
                        && inventory.query.device == origin.device
                        && control.output_identity() == Some(inventory.query.output_identity)
                })
                .ok_or_else(|| refused("refresh the vst3 parameter list"))?;
            let row = inventory
                .rows
                .iter()
                .find(|row| {
                    row.id == origin.parameter
                        && row.editable
                        && row.page_first == origin.page_first
                        && row.metadata_revision == origin.metadata_revision
                        && row.normalized == origin.initial.normalized()
                })
                .ok_or_else(|| refused("vst3 parameter metadata changed · refresh the list"))?;
            let source = self
                .sources
                .iter()
                .rev()
                .find(|source| {
                    source.accepted
                        && !source.retiring
                        && source.project_generation == origin.context.project_generation
                        && source.device == origin.device.value()
                })
                .ok_or_else(lost)?;
            if source
                .authored
                .as_ref()
                .and_then(AuthoredSelection::dense_parameter)
                == Some(origin.parameter)
            {
                return Err(refused("vst3 parameter is owned by its modulation lane"));
            }
            let binding = source.binding;
            if !edits::original_state(control, binding, cancellation)
                .map_err(refused)?
                .processing_eq(input.canonical_state())
                && !edits::captured_state_approved(
                    control,
                    binding,
                    input.canonical_state(),
                    cancellation,
                )
                .map_err(refused)?
            {
                return Err(lost());
            }
            let steps = row.steps;
            let automation_class = row.automation_class;
            let output = inventory.query.output_identity;
            if binding.root != inventory.binding.root
                || binding.instance != inventory.binding.instance
                || binding.graph != inventory.binding.graph
            {
                return Err(lost());
            }
            // Revalidate capability on the actual original SDK source,
            // including after renewal. One fresh target page is independent
            // of PCM value revisions and the total inventory size.
            let (current_value, _) = validate_current_parameter(
                control,
                binding,
                origin,
                steps,
                input.reading_class().is_some() || input.records_touch(),
            )?;
            let mut uses_hold = self
                .sources
                .iter()
                .find(|source| source.binding == binding)
                .and_then(|source| source.authored.as_ref())
                .is_some_and(|armed| {
                    automation_class.is_some_and(|class| armed.contains(class, origin.parameter))
                });
            let playing = control.status.playing.load(Acquire);
            let isolated_class = isolation_class(&input, playing)?;
            if (input.is_instrument() || input.records_touch()) && playing && !uses_hold {
                let class = isolated_class
                    .filter(|class| automation_class == Some(*class))
                    .ok_or_else(|| refused("create an automation lane for this parameter before editing during playback"))?;
                let selection = metadata::extend(
                    self.host
                        .as_ref()
                        .ok_or_else(|| refused("qualified vst3 helper unavailable"))?,
                    control,
                    binding,
                    class,
                    origin,
                    steps,
                    cancellation,
                )?;
                self.sources
                    .iter_mut()
                    .find(|source| source.binding == binding)
                    .ok_or_else(lost)?
                    .authored = Some(selection);
                uses_hold = true;
            }
            let live = if let Some(class) = isolated_class {
                let (source, transport_epoch) = control
                    .status
                    .native_parameter_source(playing)
                    .filter(|(source, _)| source.context == input.context())
                    .filter(|_| input.reading_class().is_none() || automation_class == Some(class))
                    .ok_or_else(|| refused("vst3 parameter source is not ready"))?;
                Some(LiveOverride::new(
                    source,
                    transport_epoch,
                    class,
                    input.reading_class().is_some(),
                    uses_hold,
                    playing,
                ))
            } else {
                None
            };
            let mut origin = origin;
            if live
                .as_ref()
                .is_some_and(|live| live.reading && !live.playing)
            {
                // A list can predate playback. Cancel must restore the actual
                // stopped worker value observed at Begin, including after a
                // moving authored envelope changed it beneath that list.
                origin.initial = current_value;
            }
            let mut gesture = Gesture {
                key: snapshot.key,
                origin,
                binding,
                output,
                steps,
                last: origin.initial,
                sequence: 0,
                pending: None,
                live,
                capture: None,
                capture_sequenced: false,
                last_input: None,
                last_input_idle: false,
                sequenced_capture: None,
                before: None,
                commit: None,
                next_program: None,
                commit_accepted: false,
                capture_class: automation_class,
                deliveries: Vec::new(),
                retired_delivery: None,
                recording: input.records_touch(),
                recording_base: None,
                recorded_release: false,
                recording_context: None,
                recording_closed: false,
                recording_interrupted: false,
            };
            if !gesture.current(self, control, input.context()) {
                return Err(lost());
            }
            if gesture.recording {
                let live = gesture
                    .live
                    .as_ref()
                    .filter(|live| live.playing && live.uses_hold)
                    .ok_or_else(|| refused("recording requires the eligible live vst3 target"))?;
                if !live.reading {
                    let metadata =
                        edits::source_metadata(control, binding, cancellation).map_err(refused)?;
                    if Some(metadata.selection().cid) != automation_class {
                        return Err(lost());
                    }
                    let normalized = metadata
                        .authored_base_values()
                        .find_map(|(id, base)| (id == origin.parameter).then_some(base))
                        .ok_or_else(|| refused("vst3 recording target has no verified base"))?;
                    gesture.recording_base =
                        Some(PluginParameterValue::new(normalized).map_err(refused)?);
                }
                let capture = delivery::Context::new(&gesture, control, input.context(), clock)?;
                gesture.recording_context = Some(capture);
                self.confirmed_parameters
                    .push_back(capture.ready(gesture.key));
            }
            self.parameter_gesture = Some(gesture);
            return Ok(Prepared::Applied);
        }
        let mut gesture = self.parameter_gesture.take().ok_or_else(lost)?;
        let result = if !gesture.current(self, control, input.context()) {
            Err(lost())
        } else {
            gesture.apply(control, self.host.as_ref(), &input, cancellation, clock)
        };
        self.confirmed_parameters
            .extend(gesture.deliveries.drain(..));
        let source_lost = matches!(&result, Err(Error::SourceLost(_)));
        let restored =
            result.is_ok() && matches!(input.operation(), PluginGesturePreparation::Restore { .. });
        if source_lost && let Some(live) = gesture.live.as_mut() {
            live.invalidated = true;
            live.discard = true;
        }
        // Live refusal can still own a queued input, scratch or hold. Keep it
        // on this worker's regular poll path until its actual terminal.
        if !restored && (!source_lost || gesture.live.is_some()) {
            self.parameter_gesture = Some(gesture);
        }
        result
    }

    pub(in crate::native_audio) fn finish_parameter_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        if let Some(gesture) = self.parameter_gesture.as_mut()
            && gesture.key == key
            && gesture.commit == Some((context, sequence))
        {
            if accepted {
                self.observed_parameters.clear();
                if gesture.next_program.is_some()
                    && gesture
                        .live
                        .as_ref()
                        .is_some_and(|live| live.hold.is_some())
                {
                    gesture.commit_accepted = true;
                } else {
                    self.parameter_gesture = None;
                }
            } else if gesture.recording && gesture.recorded_release {
                // An actual recorded restoration already ended the hold.
                // Failed take publication must not retain a dead controller
                // gesture or prevent the next Begin on this live source.
                self.parameter_gesture = None;
            } else {
                // A retained refusal still owns its acknowledged preview.
                // Cancel/source loss releases it; rejected publication does not.
                gesture.commit = None;
                gesture.next_program = None;
            }
        }
    }
}

impl Gesture {
    /// A capture fact only selects the existing source for this exact pending
    /// canonical edit; the audio owner independently authenticates publication.
    #[allow(clippy::too_many_arguments)] // Exact gesture, target, predecessor, source and cancellation.
    pub(super) fn sequenced_candidate_source(
        &self,
        control: &mut PublicationControl,
        successor: AudioSourceContext,
        predecessor: Option<AudioSourceContext>,
        binding: Binding,
        device: u64,
        state: &NativeDeviceState,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<bool, String> {
        let Some((committed, sequence)) = self.commit else {
            return Ok(false);
        };
        if self.live.is_some()
            || self.recording
            || self.binding != binding
            || self.origin.device.value() != device
        {
            return Ok(false);
        }
        let Some(fact) = self.sequenced_capture.as_ref() else {
            // Idle points use the existing quiescent SDK flush, which has no
            // processed-quantum capture fact. Keep its existing replacement path.
            return if self.last_input.is_none() && !control.status.playing.load(Acquire) {
                Ok(false)
            } else {
                Err("vst3 continuation capture is unavailable".into())
            };
        };
        if self.sequence != sequence
            || predecessor != Some(committed)
            || successor.project_generation != committed.project_generation
            || committed.editor_revision.checked_add(1) != Some(successor.editor_revision)
            || fact.parameter() != self.origin.parameter
            || fact.normalized_input().to_bits() != self.sdk_value(self.last).to_bits()
            || !state.shares_payload_storage_with(fact.state())
            || *state != *fact.state()
        {
            return Err("vst3 continuation capture or context changed".into());
        }
        let playing = control.status.playing.load(Acquire);
        if control
            .status
            .native_parameter_source(playing)
            .is_none_or(|(source, _)| {
                source.context != committed || source.output_identity != self.output
            })
        {
            return Err("vst3 continuation source changed".into());
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            edits::check_cancel(cancellation)?;
            match control.native_sequenced_capture_is_current_on_control(binding, fact) {
                Ok(true) => return Ok(true),
                Ok(false) => return Err("vst3 continuation input changed".into()),
                Err(
                    "native preparation is busy"
                    | "native source is busy"
                    | "native capture mailbox is busy",
                ) if Instant::now() < deadline => edits::poll_control(control),
                Err(reason) => return Err(reason.to_owned()),
            }
        }
    }

    fn current(
        &self,
        plugins: &Plugins,
        control: &PublicationControl,
        context: AudioSourceContext,
    ) -> bool {
        control.output_identity() == Some(self.output)
            && context.project_generation == self.origin.context.project_generation
            && plugins
                .sources
                .iter()
                .rev()
                .find(|source| {
                    source.accepted
                        && !source.retiring
                        && source.project_generation == context.project_generation
                        && source.device == self.origin.device.value()
                })
                .is_some_and(|source| source.binding == self.binding)
    }

    fn apply(
        &mut self,
        control: &mut PublicationControl,
        host: Option<&Hosting>,
        input: &PluginParameterGestureInput<'_>,
        cancellation: Option<&RequestCancellation>,
        clock: Option<(u32, hex_session::service::ResidentRuntimeLoop)>,
    ) -> Result<Prepared, Error> {
        let context = input.context();
        if self.recording != input.records_touch() {
            return Err(refused("vst3 gesture recording mode changed"));
        }
        let (snapshot, value, restore, commit) = match input.operation() {
            PluginGesturePreparation::Preview(snapshot) => {
                (snapshot, snapshot.plugin_value, false, false)
            }
            PluginGesturePreparation::Restore { snapshot, initial } => {
                (snapshot, Some(initial), true, false)
            }
            PluginGesturePreparation::Commit(snapshot) => {
                (snapshot, snapshot.plugin_value, false, true)
            }
            PluginGesturePreparation::Begin { .. } => {
                return Err(refused("vst3 gesture already began"));
            }
        };
        let value = value.ok_or_else(|| refused("vst3 gesture has no value"))?;
        if !self.matches(snapshot) || (!restore && snapshot.sequence < self.sequence) {
            return Err(refused("vst3 parameter gesture changed"));
        }
        if self.live.is_some() {
            if restore {
                if self.recording {
                    return self.release_recorded(control, context, clock);
                }
                return self.restore_override(control);
            }
            let live = self.live.as_ref().ok_or_else(lost)?;
            if live.invalidated
                || isolation_class(input, live.playing)? != Some(live.class)
                || input.reading_class().is_some() != live.reading
                || control.status.native_parameter_source(live.playing)
                    != Some((live.source, live.transport_epoch))
            {
                return Err(lost());
            }
        }
        if self.capture.is_some() && !commit {
            self.capture(control)?;
        }
        self.drain_point(control)?;
        if commit {
            if snapshot.sequence != self.sequence || value != self.last {
                return Err(refused("vst3 final point has not been processed"));
            }
            if self.recording {
                let result = self.release_recorded(control, context, clock);
                if result.is_ok() {
                    self.commit = Some((context, snapshot.sequence));
                }
                return result;
            }
            if self.live.is_some() {
                self.commit = Some((context, snapshot.sequence));
                return self.capture_override(
                    control,
                    host.ok_or_else(|| refused("qualified vst3 helper unavailable"))?,
                    input.canonical_state(),
                    cancellation,
                );
            }
            let before = self
                .before
                .clone()
                .ok_or_else(|| refused("vst3 has no original capture"))?;
            let after = self.capture_after_input(control)?;
            self.commit = Some((context, snapshot.sequence));
            return Ok(Prepared::Captured(Box::new(PluginParameterCapture {
                before,
                after,
            })));
        }
        if value != self.last {
            if self.live.is_none() && self.before.is_none() {
                self.before = Some(self.capture(control)?);
            }
            let actual = self.sdk_value(value);
            let source = if input.requires_delivery_confirmation() {
                self.deliveries
                    .try_reserve(2)
                    .map_err(|_| refused("could not reserve vst3 delivery observations"))?;
                Some(delivery::Context::new(self, control, context, clock)?)
            } else {
                None
            };
            let (token, opening_hold, delivery) = if let Some(live) = self
                .live
                .as_ref()
                .filter(|live| live.uses_hold && live.playing)
            {
                let command = live.hold.as_ref().map_or(
                    NativeParameterHold::Begin {
                        id: self.origin.parameter,
                        normalized: actual,
                    },
                    |hold| NativeParameterHold::Change {
                        hold,
                        normalized: actual,
                    },
                );
                let (token, delivery) = if let Some(source) = source {
                    let (token, delivery) = admit(control, |control| {
                        control.native_source_parameter_hold_with_delivery_on_control(
                            source.applied,
                            self.binding,
                            command,
                        )
                    })?;
                    (
                        token,
                        Some(delivery::PendingDelivery {
                            target: delivery,
                            context: source,
                            receipt: None,
                            kind: delivery::Kind::Point,
                        }),
                    )
                } else {
                    (
                        admit(control, |control| {
                            control.native_source_parameter_hold_on_control(
                                live.source,
                                self.binding,
                                command,
                            )
                        })?,
                        None,
                    )
                };
                (token, live.hold.is_none(), delivery)
            } else {
                let (token, delivery) = if let Some(source) = source {
                    let (token, delivery) = admit(control, |control| {
                        control.begin_native_source_parameter_change_with_delivery_on_control(
                            source.applied,
                            self.binding,
                            self.origin.parameter,
                            actual,
                        )
                    })?;
                    (
                        token,
                        Some(delivery::PendingDelivery {
                            target: delivery,
                            context: source,
                            receipt: None,
                            kind: delivery::Kind::Point,
                        }),
                    )
                } else {
                    (
                        admit(control, |control| {
                            control.begin_native_source_parameter_change_on_control(
                                self.binding,
                                self.origin.parameter,
                                actual,
                            )
                        })?,
                        None,
                    )
                };
                (token, false, delivery)
            };
            self.pending = Some(PendingPoint {
                token,
                value,
                sequence: snapshot.sequence,
                opening_hold,
                delivery,
                processed: false,
                sdk_finished: false,
                #[cfg(all(test, feature = "allocation-probe"))]
                hold_sdk_ack: matches!(
                    std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").as_deref(),
                    Ok("delivery_retire" | "touch_close_pending" | "touch_close_queued")
                ),
            });
            self.drain_point(control)?;
        }
        self.sequence = snapshot.sequence;
        Ok(Prepared::Applied)
    }

    fn matches(&self, snapshot: GestureSnapshot) -> bool {
        snapshot.key == self.key
            && snapshot.target
                == GestureTarget::PluginParameter {
                    device: self.origin.device,
                    parameter: self.origin.parameter,
                }
    }

    fn sdk_value(&self, value: PluginParameterValue) -> f64 {
        if self.steps == 0 {
            value.normalized()
        } else {
            let steps = f64::from(self.steps);
            (value.normalized() * steps).round() / steps
        }
    }

    fn poll_point(&mut self, control: &mut PublicationControl) -> Result<bool, Error> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.delivery.is_some())
        {
            return self.poll_delivered_point(control);
        }
        let Some(pending) = self.pending.as_ref() else {
            return Ok(true);
        };
        match poll_input(control, self.binding, &pending.token) {
            Ok(
                progress @ (NativeWorkerParameterProgress::Processed(_)
                | NativeWorkerParameterProgress::Flushed),
            ) => {
                if let Some(pending) = self.pending.take() {
                    self.last = pending.value;
                    self.sequence = pending.sequence;
                    if pending.opening_hold
                        && let Some(live) = self.live.as_mut()
                    {
                        live.hold = Some(pending.token);
                    } else if self.live.is_none() {
                        self.last_input_idle =
                            matches!(progress, NativeWorkerParameterProgress::Flushed);
                        self.last_input = Some(pending.token);
                    }
                }
                Ok(true)
            }
            Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {
                Ok(false)
            }
            Ok(NativeWorkerParameterProgress::Failed) => {
                self.pending = None;
                Err(lost())
            }
            Ok(
                NativeWorkerParameterProgress::Refused
                | NativeWorkerParameterProgress::Withdrawn
                | NativeWorkerParameterProgress::Released { .. },
            ) => {
                self.pending = None;
                Err(refused("vst3 input was not applied"))
            }
            Err(reason) if source_gone(reason) => {
                self.pending = None;
                Err(lost())
            }
            Err(reason) => Err(refused(reason)),
        }
    }

    fn drain_point(&mut self, control: &mut PublicationControl) -> Result<(), Error> {
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            if self.poll_point(control)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                if let Some(pending) = self.pending.as_ref()
                    && control.withdraw_native_source_parameter_change_on_control(
                        self.binding,
                        &pending.token,
                    ) == Ok(true)
                {
                    continue;
                }
                return Err(refused("vst3 parameter input is still pending"));
            }
            edits::poll_control(control);
        }
    }

    fn capture(&mut self, control: &mut PublicationControl) -> Result<NativeDeviceState, Error> {
        self.capture_inner(control, false)
    }

    fn capture_after_input(
        &mut self,
        control: &mut PublicationControl,
    ) -> Result<NativeDeviceState, Error> {
        self.capture_inner(control, true)
    }

    fn capture_inner(
        &mut self,
        control: &mut PublicationControl,
        after_input: bool,
    ) -> Result<NativeDeviceState, Error> {
        if self.capture.is_none() {
            self.sequenced_capture = None;
            self.capture_sequenced = after_input
                && self.last_input.is_some()
                && control
                    .native_source_endpoint_status(self.binding)
                    .is_some_and(|(_, live)| live);
            self.capture = Some(if self.capture_sequenced {
                let input = self.last_input.as_ref().ok_or_else(lost)?;
                admit(control, |control| {
                    if self.last_input_idle {
                        control.begin_native_capture_after_idle_parameter_on_control(
                            self.binding,
                            input,
                        )
                    } else {
                        control.begin_native_capture_after_parameter_on_control(self.binding, input)
                    }
                })?
            } else {
                admit(control, |control| {
                    control.begin_native_source_state_capture_on_control(self.binding)
                })?
            });
        }
        let request = self
            .capture
            .as_ref()
            .ok_or_else(|| refused("vst3 capture unavailable"))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = if self.capture_sequenced {
                control
                    .poll_native_capture_after_parameter_on_control(self.binding, request)
                    .map(|progress| match progress {
                        NativeWorkerSequencedCaptureProgress::Pending => {
                            NativeWorkerCaptureProgress::Pending
                        }
                        NativeWorkerSequencedCaptureProgress::Failed => {
                            NativeWorkerCaptureProgress::Failed
                        }
                        NativeWorkerSequencedCaptureProgress::Refused(reason) => {
                            NativeWorkerCaptureProgress::Refused(reason)
                        }
                        NativeWorkerSequencedCaptureProgress::Captured(fact) => {
                            let state = fact.state().clone();
                            self.sequenced_capture = Some(fact);
                            NativeWorkerCaptureProgress::Captured(state)
                        }
                    })
            } else {
                control.poll_native_source_state_capture_on_control(self.binding, request)
            };
            match result {
                Ok(NativeWorkerCaptureProgress::Captured(state)) => {
                    self.capture = None;
                    self.capture_sequenced = false;
                    if let Some(fact) = self.sequenced_capture.as_ref()
                        && (fact.parameter() != self.origin.parameter
                            || fact.normalized_input().to_bits()
                                != self.sdk_value(self.last).to_bits()
                            || !admit(control, |control| {
                                control.native_sequenced_capture_is_current_on_control(
                                    self.binding,
                                    fact,
                                )
                            })?)
                    {
                        self.sequenced_capture = None;
                        return Err(refused("vst3 input changed during capture"));
                    }
                    let state = if let Some(fact) = self.sequenced_capture.as_ref() {
                        admit(control, |control| {
                            control.approve_native_sequenced_capture_on_control(self.binding, fact)
                        })?
                    } else {
                        state
                    };
                    return Ok(state);
                }
                Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {}
                Ok(NativeWorkerCaptureProgress::Refused(reason)) => {
                    self.capture = None;
                    self.capture_sequenced = false;
                    return Err(refused(reason.to_string()));
                }
                Ok(NativeWorkerCaptureProgress::Failed)
                | Err("native capture output is closing") => {
                    self.capture = None;
                    self.capture_sequenced = false;
                    return Err(lost());
                }
                Err("native sequenced capture was superseded") => {
                    self.capture = None;
                    self.capture_sequenced = false;
                    return Err(refused("vst3 input changed during capture"));
                }
                Err(reason) => return Err(refused(reason)),
            }
            if Instant::now() >= deadline {
                return Err(refused("vst3 state capture is still settling"));
            }
            edits::poll_control(control);
        }
    }
}

fn poll_input(
    control: &mut PublicationControl,
    binding: Binding,
    token: &NativeWorkerParameterChange,
) -> Result<NativeWorkerParameterProgress, &'static str> {
    match control.poll_native_source_parameter_change_on_control(binding, token) {
        Ok(NativeWorkerParameterProgress::Failed) => {
            control.drain_stopped_native_source_parameter_change_on_control(binding, token)
        }
        other => other,
    }
}

fn source_gone(reason: &str) -> bool {
    matches!(
        reason,
        "native parameter output is closing"
            | "native parameter hold output differs or is closing"
            | "native stopped parameter source differs"
            | "native parameter source is not live"
            | "native capture is closing"
            | "native capture source is closing"
            | "native source differs"
    )
}

/// Revalidate only the bounded target page on the actual original source. The
/// native worker owns query deadlines and every admitted token is drained.
fn validate_current_parameter(
    control: &mut PublicationControl,
    binding: Binding,
    origin: PluginParameterOrigin,
    steps: u32,
    reading: bool,
) -> Result<(PluginParameterValue, u64), Error> {
    use hex_audio::plugin_worker::{ControllerRequest, NativeWorkerControllerProgress};
    let token = admit(control, |control| {
        control.begin_native_source_controller_query_on_control(
            binding,
            ControllerRequest::SelectedParameters {
                first: origin.page_first,
            },
        )
    })?;
    let page = loop {
        match control.poll_native_source_controller_query_on_control(binding, &token) {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                edits::poll_control(control)
            }
            Ok(NativeWorkerControllerProgress::Page(page)) => break page,
            _ => {
                return Err(refused(
                    "vst3 parameter metadata changed · refresh the list",
                ));
            }
        }
    };
    if page.first != origin.page_first {
        return Err(refused(
            "vst3 parameter metadata changed · refresh the list",
        ));
    }
    let parameter = page
        .as_slice()
        .iter()
        .find(|parameter| parameter.id == origin.parameter)
        .ok_or_else(|| refused("vst3 parameter metadata changed · refresh the list"))?;
    if !parameter.is_editable()
        || parameter.step_count != steps
        || (reading && !parameter.supports_authored_automation())
        || (!reading && parameter.normalized != origin.initial.normalized())
    {
        return Err(refused(
            "vst3 parameter metadata changed · refresh the list",
        ));
    }
    PluginParameterValue::new(parameter.normalized)
        .map(|value| (value, page.revision))
        .map_err(refused)
}
