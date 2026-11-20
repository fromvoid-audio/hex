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
    RetiringEditorReplayDisposition,
};
use std::time::{Duration, Instant};

pub(in crate::native_audio) struct ReplayCapture {
    pub(super) binding: Binding,
    pub(super) publication: AudioSourceContext,
    pub(super) identity: NativeProcessorIdentity,
    source: AppliedAudioSource,
    transport_epoch: u64,
    playing: bool,
    fact: NativeWorkerStateAfterGuardedWrite,
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
            control.begin_native_source_guarded_editor_restore_on_control(
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
        Ok(Some(NativeParameterContinuationRequest {
            source: self.source,
            transport_epoch: self.transport_epoch,
            playing: self.playing,
            successor,
            identity: self.identity,
            binding: self.binding,
            previous_state: &self.before,
            capture: NativeParameterContinuationCapture::Guarded {
                fact: &self.fact,
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
        control: Option<&mut PublicationControl>,
        batch: &RetiringEditorBatch,
        disposition: RetiringEditorReplayDisposition,
    ) -> Result<(), String> {
        self.retiring_replay_capture = None;
        let restore = self.retiring_replay_restore.take();
        let (accepted, recovery) = match disposition {
            RetiringEditorReplayDisposition::Refused => (
                false,
                restore.map_or(Ok(()), |restore| restore.restore(control)),
            ),
            RetiringEditorReplayDisposition::Published { receipt_accepted } => {
                (receipt_accepted, Ok(()))
            }
        };
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
        points: &[(u32, f64)],
        publication: AudioSourceContext,
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
                    if destination.is_instrument() {
                        return Err(
                            "vst3 instrument replay requires a separate continuation".into()
                        );
                    }
                    target = Some((destination.owner(), state));
                }
                Ok(())
            })?;
        let (owner, canonical) = target.ok_or("vst3 replay canonical target changed")?;
        let native = self
            .sources
            .iter()
            .find(|native| native.binding == binding)
            .ok_or("vst3 replay successor is unavailable")?;
        if native.owner != owner || native.authored.is_some() {
            return Err("vst3 replay requires an ordinary effect without authored input".into());
        }
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
        let values = recovery_values(control, binding, points, cancellation)?;
        let before = capture(control, binding, cancellation)?;
        let token = admit(control, cancellation, |control| {
            // A replacement helper is fresh. Its own editor input takes
            // precedence over any retained value from the predecessor.
            control.begin_native_source_guarded_editor_write_on_control(binding, 0, points)
        })?;
        use NativeWorkerGuardedEditorWriteProgress as Progress;
        let outcome = guarded_terminal(control, binding, &token);
        if outcome == Ok(Progress::Conflict) {
            return Err("vst3 replacement accepted a newer editor edit".into());
        }
        // A confirmed write, a changed readback and an unknown terminal all
        // retain this operation until Session settles it. Only the first two
        // can authorize restoration on the worker.
        let restore = self.retiring_replay_restore.insert(ReplayRestore {
            source,
            transport_epoch,
            playing,
            binding,
            written: token,
            values,
            before: before.clone(),
        });
        match outcome? {
            Progress::Applied => {}
            Progress::ReadbackChanged => {
                return Err("vst3 replacement returned a different replay value".into());
            }
            _ => return Err("vst3 retiring editor write lost its successor".into()),
        }
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
                Ok(NativeWorkerControllerProgress::EditorFrontier(0)) => break,
                _ => return Err("vst3 replacement editor input changed during capture".into()),
            }
        }
        edits::check_cancel(cancellation)?;
        let after = admit(control, cancellation, |control| {
            control.approve_native_guarded_capture_on_control(binding, &fact)
        })?
        .with_host_metadata_from(canonical)
        .map_err(|error| error.to_string())?;
        self.retiring_replay_capture = Some(ReplayCapture {
            binding,
            publication,
            identity,
            source,
            transport_epoch,
            playing,
            fact,
            before: canonical.clone(),
            after: after.clone(),
        });
        Ok(PluginParameterCapture { before, after })
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
