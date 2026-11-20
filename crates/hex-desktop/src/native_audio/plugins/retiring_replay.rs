//! Resolve one available retiring receipt against its original successor.
use super::*;
use hex_audio::{
    plugin_worker::{
        ControllerRequest, NativeWorkerCaptureProgress, NativeWorkerControllerProgress,
        NativeWorkerControllerQuery, NativeWorkerGuardedCaptureProgress,
        NativeWorkerGuardedEditorWriteProgress, NativeWorkerStateAfterGuardedWrite,
        ParameterPageQuery,
    },
    prepared::{
        AppliedAudioSource, NativeParameterContinuationCapture, NativeParameterContinuationRequest,
        NativeProcessorIdentity, PublicationControl,
    },
};
use hex_engine::playback::Playback;
use hex_session::service::{
    PluginParameterCapture, RequestCancellation, RetiringEditorBatch,
    RetiringEditorReplayDisposition, RetiringEditorReplayMode,
};
use std::time::{Duration, Instant};

pub(in crate::native_audio) struct ReplayCapture {
    pub(super) binding: Binding,
    pub(super) publication: AudioSourceContext,
    pub(super) identity: NativeProcessorIdentity,
    source: AppliedAudioSource,
    transport_epoch: u64,
    playing: bool,
    // An isolated capture cannot use the active worker's guarded-write fact.
    // Its exact producer hold remains owned through callback confirmation.
    fact: Option<NativeWorkerStateAfterGuardedWrite>,
    before: NativeDeviceState,
    after: NativeDeviceState,
}

pub(super) struct ReplayRestore {
    source: AppliedAudioSource,
    transport_epoch: u64,
    playing: bool,
    binding: Binding,
    written: NativeWorkerControllerQuery,
    values: Vec<(u32, f64)>,
    before: NativeDeviceState,
}

impl ReplayRestore {
    fn restore(self, control: Option<&mut PublicationControl>) -> Result<(), String> {
        let control = control
            .filter(|control| {
                control.status.native_parameter_source(self.playing)
                    == Some((self.source, self.transport_epoch))
            })
            .ok_or("vst3 replay recovery lost its original output or transport")?;
        let token = admit(control, None, |control| {
            control.begin_native_source_guarded_editor_write_after_on_control(
                self.binding,
                0,
                &self.written,
                &self.values,
            )
        })?;
        use NativeWorkerGuardedEditorWriteProgress as Progress;
        match guarded_terminal(control, self.binding, &token)? {
            Progress::Applied => {}
            Progress::Conflict => {
                return Err("vst3 replay recovery encountered a newer editor edit".into());
            }
            _ => return Err("vst3 replay recovery could not restore the controller values".into()),
        }
        if !capture(control, self.binding, None)?.processing_eq(&self.before) {
            return Err("vst3 replay recovery could not confirm the original opaque state".into());
        }
        Ok(())
    }
}

impl ReplayCapture {
    pub(super) fn matches_candidate(
        &self,
        control: &PublicationControl,
        state: &NativeDeviceState,
    ) -> bool {
        self.source_matches(control)
            && state.shares_payload_storage_with(&self.after)
            && *state == self.after
    }

    fn source_matches(&self, control: &PublicationControl) -> bool {
        control.status.native_parameter_source(self.playing)
            == Some((self.source, self.transport_epoch))
    }

    pub(super) fn isolated_source(
        &self,
        successor: AudioSourceContext,
    ) -> Option<(AppliedAudioSource, u64, bool)> {
        (self.fact.is_none() && self.publication == successor).then_some((
            self.source,
            self.transport_epoch,
            self.playing,
        ))
    }

    pub(super) fn publication_request<'a>(
        &'a self,
        control: Option<&PublicationControl>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        successor: AudioSourceContext,
        playback: &Playback,
    ) -> Result<Option<NativeParameterContinuationRequest<'a>>, String> {
        if successor != self.publication {
            return Err("vst3 replay publication context changed".into());
        }
        let control = control.ok_or("vst3 replay publication output is unavailable")?;
        if desired.is_none_or(|desired| desired.context() != self.source.context)
            || !self.source_matches(control)
            || !playback.has_foreign_binding_on_control(
                self.identity.owner(),
                self.identity.device_id(),
                self.binding,
            )
        {
            return Err("vst3 replay publication source changed".into());
        }
        let Some(fact) = &self.fact else {
            let matching = playback.foreign_nodes_on_control().any(|node| {
                node.owner == self.identity.owner()
                    && node.device_id == self.identity.device_id()
                    && node
                        .foreign_processor()
                        .is_some_and(|processor| processor.binding() == Some(self.binding))
                    && node.configuration.as_deref().is_some_and(|state| {
                        state.shares_payload_storage_with(&self.after) && *state == self.after
                    })
            });
            return if matching {
                Ok(None)
            } else {
                Err("vst3 isolated replay candidate state differs".into())
            };
        };
        Ok(Some(NativeParameterContinuationRequest {
            source: self.source,
            transport_epoch: self.transport_epoch,
            playing: self.playing,
            successor,
            identity: self.identity,
            binding: self.binding,
            previous_state: &self.before,
            capture: NativeParameterContinuationCapture::Guarded {
                fact,
                state: &self.after,
            },
        }))
    }
}

impl Plugins {
    fn replay_index(&self) -> Option<usize> {
        self.sources.iter().position(|source| {
            source
                .editor_retirement
                .batch
                .as_ref()
                .is_some_and(|pending| !pending.replay_attempted)
        })
    }

    pub(in crate::native_audio) fn retiring_editor_replay_ready(&self) -> bool {
        self.replay_index().is_some()
    }

    pub(in crate::native_audio) fn next_retiring_editor_replay(
        &mut self,
    ) -> Option<RetiringEditorBatch> {
        let index = self.replay_index()?;
        let pending = self.sources[index].editor_retirement.batch.as_mut()?;
        pending.replay_attempted = true;
        Some(pending.offer.clone())
    }

    pub(in crate::native_audio) fn finish_retiring_editor_replay(
        &mut self,
        mut control: Option<&mut PublicationControl>,
        batch: &RetiringEditorBatch,
        disposition: RetiringEditorReplayDisposition,
    ) -> Result<(), String> {
        self.retiring_replay_capture = None;
        let restore = self.retiring_replay_restore.take();
        let (accepted, recovery) = match disposition {
            RetiringEditorReplayDisposition::Refused => (
                false,
                restore.map_or(Ok(()), |restore| restore.restore(control.as_deref_mut())),
            ),
            RetiringEditorReplayDisposition::Confirmed { receipt_accepted } => {
                (receipt_accepted, Ok(()))
            }
        };
        self.release_replay_editor_hold(control);
        if !accepted
            && let Some(source) = self.sources.iter_mut().find(|source| {
                source.binding == batch.source
                    && source
                        .editor_retirement
                        .batch
                        .as_ref()
                        .is_some_and(|pending| pending.replay_attempted && pending.offer == *batch)
            })
        {
            source.editor_retirement.report_unresolved();
            if recovery.is_err()
                && let Some(pending) = source.editor_retirement.batch.as_mut()
            {
                pending.successor_unconfirmed = true;
            }
        }
        recovery
    }

    pub(super) fn replay_state_is_unconfirmed(&self, binding: Binding) -> bool {
        self.sources.iter().any(|source| {
            source
                .editor_retirement
                .batch
                .as_ref()
                .is_some_and(|pending| {
                    let successor = pending.offer.successor;
                    // Stopped renewal preserves the same helper and graph identity.
                    pending.successor_unconfirmed
                        && successor.root == binding.root
                        && successor.graph == binding.graph
                        && successor.instance == binding.instance
                })
        })
    }

    pub(in crate::native_audio) fn capture_retiring_editor_replay(
        &mut self,
        control: &mut PublicationControl,
        batch: &RetiringEditorBatch,
        mode: RetiringEditorReplayMode<'_>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<PluginParameterCapture, String> {
        let index = self
            .sources
            .iter()
            .position(|source| source.binding == batch.source)
            .ok_or("vst3 retiring replay source changed")?;
        let successor = self.retiring_successor(index)?;
        if successor.binding != batch.successor
            || successor.context != batch.successor_context
            || self.parameter_gesture.is_some()
            || self.editor_recording.is_some()
            || !self.sources[index]
                .editor_retirement
                .batch
                .as_ref()
                .is_some_and(|pending| pending.replay_attempted && pending.offer == *batch)
        {
            return Err("vst3 retiring replay successor changed".into());
        }
        let binding = successor.binding;
        let desired = desired
            .filter(|desired| desired.context() == successor.context)
            .ok_or("vst3 replay canonical source changed")?;
        let mut target = None;
        desired
            .effect_preparation()
            .visit_plugin_devices(|destination, device, state| {
                if device == batch.device {
                    target = Some((destination, state));
                }
                Ok(())
            })?;
        let (destination, canonical) = target.ok_or("vst3 replay canonical target changed")?;
        let owner = destination.owner();
        let native = self
            .sources
            .iter()
            .find(|native| native.binding == binding)
            .ok_or("vst3 replay successor is unavailable")?;
        if native.owner != owner {
            return Err("vst3 replay successor owner differs".into());
        }
        let authored = native.authored.is_some();
        let modulated = native
            .authored
            .as_ref()
            .and_then(AuthoredSelection::dense_parameter);
        let identity = NativeProcessorIdentity::new(owner, batch.device)
            .map_err(|_| "vst3 replay target identity differs")?;
        let playing = control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire);
        let (source, transport_epoch) = control
            .status
            .native_parameter_source(playing)
            .filter(|(source, _)| {
                source.context == successor.context && source.output_identity == binding.root
            })
            .ok_or("vst3 replay applied source changed")?;
        let (expected, apply, isolate) = match mode {
            RetiringEditorReplayMode::VerifyExisting { points } => (points, None, authored),
            RetiringEditorReplayMode::Apply {
                verify_existing,
                cohorts,
                publication,
                manual_base,
            } => (verify_existing, Some((publication, cohorts)), manual_base),
        };
        if isolate {
            if modulated.is_some() {
                return Err("vst3 replay requires a source without modulation".into());
            }
            self.hold_editor_for_replay(control, binding, cancellation)?;
            let host = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?;
            if !expected.is_empty() {
                let selection = edits::armed_selection(control, binding, cancellation)?
                    .ok_or("vst3 canonical replay targets are unavailable")?;
                let bases = edits::canonical_bases(
                    host,
                    control,
                    binding,
                    canonical,
                    &selection,
                    cancellation,
                    None,
                )?;
                if expected
                    .iter()
                    .any(|point| !bases.values().any(|base| base == *point))
                {
                    return Err(
                        "vst3 replacement canonical base does not contain the retiring edit".into(),
                    );
                }
            }
            let current = |control: &PublicationControl| {
                control.status.native_parameter_source(playing) == Some((source, transport_epoch))
            };
            if !current(control) {
                return Err("vst3 replay applied source changed".into());
            }
            let Some((publication, cohorts)) = apply else {
                return Ok(PluginParameterCapture {
                    before: canonical.clone(),
                    after: canonical.clone(),
                });
            };
            let after = capture_isolated_replay(
                host,
                control,
                canonical,
                cohorts,
                source,
                binding,
                playing,
                cancellation,
            )?
            .with_host_metadata_from(canonical)
            .map_err(|error| error.to_string())?;
            if !current(control) {
                return Err("vst3 replay applied source changed".into());
            }
            self.retiring_replay_capture = Some(ReplayCapture {
                binding,
                publication,
                identity,
                source,
                transport_epoch,
                playing,
                fact: None,
                before: canonical.clone(),
                after: after.clone(),
            });
            return Ok(PluginParameterCapture {
                before: canonical.clone(),
                after,
            });
        }
        if authored {
            return Err("vst3 replay requires an isolated canonical base".into());
        }
        if !expected.is_empty()
            && recovery_values(control, binding, expected, cancellation)?.as_slice() != expected
        {
            return Err("vst3 replacement does not already contain the retiring edit".into());
        }
        let mut available_recovery = if let Some((_, cohorts)) = apply {
            let mut targets: Vec<(u32, f64)> = Vec::new();
            for &point in cohorts.iter().flatten() {
                if !targets.iter().any(|target| target.0 == point.0) {
                    targets.push(point);
                }
            }
            recovery_values(control, binding, &targets, cancellation)?
        } else {
            Vec::new()
        };
        let before = capture(control, binding, cancellation)?;
        if !expected.is_empty() {
            let bound_before = before
                .clone()
                .with_host_metadata_from(canonical)
                .map_err(|error| error.to_string())?;
            if bound_before != *canonical {
                return Err("vst3 replay confirmation differs from canonical state".into());
            }
            confirm_unedited_successor(control, binding, cancellation)?;
            if control.status.native_parameter_source(playing) != Some((source, transport_epoch)) {
                return Err("vst3 replay applied source changed".into());
            }
        }
        let Some((publication, cohorts)) = apply else {
            return Ok(PluginParameterCapture {
                before: before.clone(),
                after: before,
            });
        };
        use NativeWorkerGuardedEditorWriteProgress as Progress;
        let deadline = Instant::now() + super::paused_monitor::PREPARATION_TIMEOUT;
        for points in cohorts {
            if Instant::now() >= deadline {
                return Err("vst3 retiring editor replay reached its preparation deadline".into());
            }
            if control.status.native_parameter_source(playing) != Some((source, transport_epoch)) {
                return Err("vst3 replay applied source changed".into());
            }
            let token = admit(control, cancellation, |control| {
                if let Some(previous) = &self.retiring_replay_restore {
                    control.begin_native_source_guarded_editor_write_after_on_control(
                        binding,
                        0,
                        &previous.written,
                        points,
                    )
                } else {
                    control.begin_native_source_guarded_editor_write_on_control(binding, 0, points)
                }
            })?;
            let outcome = guarded_terminal(control, binding, &token);
            if outcome == Ok(Progress::Conflict) {
                return Err("vst3 replacement accepted a newer editor edit".into());
            }
            // Preserve the first capture and the original values of exactly
            // the targets we might have changed, even after an unknown terminal.
            let mut restore = match self.retiring_replay_restore.take() {
                Some(mut restore) => {
                    restore.written = token;
                    restore
                }
                None => ReplayRestore {
                    source,
                    transport_epoch,
                    playing,
                    binding,
                    written: token,
                    values: Vec::new(),
                    before: before.clone(),
                },
            };
            restore.values.extend(
                available_recovery
                    .extract_if(.., |value| points.iter().any(|point| point.0 == value.0)),
            );
            self.retiring_replay_restore = Some(restore);
            match outcome? {
                Progress::Applied => {}
                Progress::ReadbackChanged => {
                    return Err("vst3 replacement returned a different replay value".into());
                }
                _ => return Err("vst3 retiring editor write lost its successor".into()),
            }
        }
        let restore = self
            .retiring_replay_restore
            .as_ref()
            .ok_or("vst3 retiring editor replay has no applied cohort")?;
        let capture = admit(control, cancellation, |control| {
            control.begin_native_capture_after_guarded_write_on_control(binding, &restore.written)
        })?;
        let fact = loop {
            use NativeWorkerGuardedCaptureProgress as Progress;
            match control.poll_native_capture_after_guarded_write_on_control(binding, &capture) {
                Ok(Progress::Pending) | Err("native preparation is busy") => {
                    edits::poll_control(control)
                }
                Ok(Progress::Captured(fact)) => break fact,
                Ok(Progress::Refused(reason)) => return Err(reason.to_string()),
                Ok(Progress::Failed) => {
                    return Err("vst3 retiring editor capture lost its successor".into());
                }
                Err(reason) => return Err(reason.into()),
            }
        };
        confirm_unedited_successor(control, binding, cancellation)?;
        let after = admit(control, cancellation, |control| {
            control.approve_native_guarded_capture_on_control(binding, &fact)
        })?
        .with_host_metadata_from(canonical)
        .map_err(|error| error.to_string())?;
        if control.status.native_parameter_source(playing) != Some((source, transport_epoch)) {
            return Err("vst3 replay applied source changed".into());
        }
        self.retiring_replay_capture = Some(ReplayCapture {
            binding,
            publication,
            identity,
            source,
            transport_epoch,
            playing,
            fact: Some(fact),
            before: canonical.clone(),
            after: after.clone(),
        });
        Ok(PluginParameterCapture { before, after })
    }
}

#[allow(clippy::too_many_arguments)] // Keep the exact native source and its canonical input explicit.
fn capture_isolated_replay(
    host: &Hosting,
    control: &mut PublicationControl,
    canonical: &NativeDeviceState,
    cohorts: &[Vec<(u32, f64)>],
    source: AppliedAudioSource,
    binding: Binding,
    playing: bool,
    cancellation: Option<&RequestCancellation>,
) -> Result<NativeDeviceState, String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut scratch = loop {
        edits::check_cancel(cancellation)?;
        match host.begin_ordered_manual_base_capture_on_control(
            control, source, binding, canonical, cohorts, playing,
        ) {
            Err(error)
                if matches!(
                    error.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) && Instant::now() < deadline =>
            {
                edits::poll_control(control)
            }
            result => break result?,
        }
    };
    loop {
        let mut cancelled = || {
            cancellation.is_some_and(RequestCancellation::is_cancelled)
                || Instant::now() >= deadline
        };
        if let Some(terminal) = scratch.poll_on_control(control, &mut cancelled) {
            return terminal;
        }
        edits::poll_control(control);
    }
}

fn confirm_unedited_successor(
    control: &mut PublicationControl,
    binding: Binding,
    cancellation: Option<&RequestCancellation>,
) -> Result<(), String> {
    let token = admit(control, cancellation, |control| {
        control.begin_native_source_controller_query_on_control(
            binding,
            ControllerRequest::ReadEditorFrontier,
        )
    })?;
    loop {
        match control.poll_native_source_controller_query_on_control(binding, &token) {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                edits::poll_control(control)
            }
            Ok(NativeWorkerControllerProgress::EditorFrontier(0)) => {
                return edits::check_cancel(cancellation);
            }
            _ => return Err("vst3 replacement editor input changed during capture".into()),
        }
    }
}

fn guarded_terminal(
    control: &mut PublicationControl,
    binding: Binding,
    token: &NativeWorkerControllerQuery,
) -> Result<NativeWorkerGuardedEditorWriteProgress, String> {
    loop {
        match control.poll_native_source_guarded_editor_write_on_control(binding, token) {
            Ok(NativeWorkerGuardedEditorWriteProgress::Pending)
            | Err("native preparation is busy") => edits::poll_control(control),
            result => return result.map_err(str::to_owned),
        }
    }
}

fn recovery_values(
    control: &mut PublicationControl,
    binding: Binding,
    points: &[(u32, f64)],
    cancellation: Option<&RequestCancellation>,
) -> Result<Vec<(u32, f64)>, String> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(points.len())
        .map_err(|_| "vst3 replay value allocation failed")?;
    values.resize(points.len(), None);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut query = ParameterPageQuery::default();
    while Instant::now() < deadline {
        let request = admit(control, cancellation, |control| {
            control.begin_native_source_controller_query_on_control(binding, query)
        })?;
        let page = loop {
            match control.poll_native_source_controller_query_on_control(binding, &request) {
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                    edits::poll_control(control)
                }
                Ok(NativeWorkerControllerProgress::Page(page)) => break page,
                _ => return Err("vst3 replay could not read the original controller values".into()),
            }
        };
        for parameter in page
            .as_slice()
            .iter()
            .filter(|parameter| parameter.is_editable())
        {
            if let Some(index) = points.iter().position(|&(id, _)| id == parameter.id) {
                values[index] = Some((parameter.id, parameter.normalized));
            }
        }
        if values.iter().all(Option::is_some) {
            return Ok(values.into_iter().flatten().collect());
        }
        let first = page.first + page.as_slice().len() as u16;
        if first == page.total {
            break;
        }
        query.first = first;
        query.revision = page.revision;
    }
    Err("vst3 replay could not determine every prior parameter value".into())
}

fn capture(
    control: &mut PublicationControl,
    binding: Binding,
    cancellation: Option<&RequestCancellation>,
) -> Result<NativeDeviceState, String> {
    let token = admit(control, cancellation, |control| {
        control.begin_native_source_state_capture_on_control(binding)
    })?;
    loop {
        match control.poll_native_source_state_capture_on_control(binding, &token) {
            Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {
                edits::poll_control(control)
            }
            Ok(NativeWorkerCaptureProgress::Captured(state)) => {
                edits::check_cancel(cancellation)?;
                return Ok(state);
            }
            Ok(NativeWorkerCaptureProgress::Refused(reason)) => return Err(reason.to_string()),
            Ok(NativeWorkerCaptureProgress::Failed) => {
                return Err("vst3 retiring editor capture lost its successor".into());
            }
            Err(reason) => return Err(reason.into()),
        }
    }
}

fn admit<T>(
    control: &mut PublicationControl,
    cancellation: Option<&RequestCancellation>,
    mut begin: impl FnMut(&mut PublicationControl) -> Result<T, &'static str>,
) -> Result<T, String> {
    let until = Instant::now() + Duration::from_millis(150);
    loop {
        edits::check_cancel(cancellation)?;
        match begin(control) {
            Err(
                "native preparation is busy"
                | "native capture mailbox is busy"
                | "native controller mailbox is busy"
                | "native parameter mailbox is busy",
            ) if Instant::now() < until => edits::poll_control(control),
            result => return result.map_err(str::to_owned),
        }
    }
}
