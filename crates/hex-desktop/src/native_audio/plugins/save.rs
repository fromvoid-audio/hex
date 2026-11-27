//! Existing canonical worker's bounded native capture batch. No native owners
//! or extra queue/reader move here. Every admitted token is drained, including
//! failed/cancelled captures, before its caller-owned metadata is released.
use super::*;
use hex_audio::plugin_worker::{
    ControllerRequest, NativeWorkerCapture, NativeWorkerCaptureProgress,
    NativeWorkerControllerProgress, StateCaptureRefusal,
};
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_session::service::{PluginStateCapture, PluginStateCaptureInput, RequestCancellation};
use std::time::{Duration, Instant};

/// Whole-capture attempts per device while the helper reports that the plugin
/// is still changing. Each refused attempt already spent the helper's settle
/// bound (about one second), so Save gives up after a few seconds.
const STILL_CHANGING_ATTEMPTS: u8 = 3;
const STILL_CHANGING: &str = "vst3 state kept changing · stop editing the plugin, then try again";

/// Disposition of one helper capture refusal: `Ok` retries the whole
/// capture for that device, `Err` is the specific failure. The plugin keeps
/// running either way; nothing partial is saved.
fn refused_capture(attempts: &mut u8, refusal: StateCaptureRefusal) -> Result<(), String> {
    match refusal {
        StateCaptureRefusal::StillChanging => {
            *attempts = attempts.saturating_add(1);
            if *attempts < STILL_CHANGING_ATTEMPTS {
                Ok(())
            } else {
                Err(STILL_CHANGING.to_owned())
            }
        }
        StateCaptureRefusal::ExceedsLimit { .. } => {
            Err(format!("vst3 state capture refused: {refusal}"))
        }
    }
}

struct Entry {
    source_index: usize,
    binding: Binding,
    owner: ProcessorChainOwner,
    device: u64,
    expected: NativeDeviceState,
    request: Option<NativeWorkerCapture>,
    captured: Option<NativeDeviceState>,
    /// Refused-as-still-changing attempts so far.
    refused: u8,
    done: bool,
    idle_editor: Option<IdleEditorRead>,
    editor_frontier: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IdleEditorRead {
    Previous,
    Save,
}

const SAVE_TRANSPORT_CHANGED: &str = "transport changed while preparing vst3 save · retry save";

impl NativeAudioSink {
    pub(in crate::native_audio) fn capture_current_plugin_states(
        &mut self,
        input: PluginStateCaptureInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        // Capture uses exact accepted native bindings, not the graph's loop
        // tag. A pending gain/loop publication may retain those same instances;
        // refusing it would break Save queued behind an accepted state edit.
        self.plugins.required_capture_sources(&input)?;
        self.plugins.capture_states(self.output.as_mut(), input)
    }

    pub(in crate::native_audio) fn finish_current_plugin_state_save(
        &mut self,
        context: AudioSourceContext,
        saved: bool,
    ) {
        self.plugins.finish_state_save(context, saved);
    }
}

impl Plugins {
    fn required_capture_sources(&self, input: &PluginStateCaptureInput<'_>) -> Result<(), String> {
        let mut required = false;
        input.visit_plugin_devices(|owner, device, _| {
            required = true;
            let mut sources = self.sources.iter().filter(|source| {
                source.project_generation == input.context().project_generation
                    && source.owner == owner
                    && source.device == device
            });
            if !sources.clone().any(|source| source.accepted)
                && sources.any(|source| !source.retiring)
            {
                return Err("vst3 capture source preparation is pending".into());
            }
            Ok(())
        })?;
        if required && self.hold_reopen(input.context()) {
            return Err("vst3 state restoration is pending".into());
        }
        Ok(())
    }

    fn capture_states(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        input: PluginStateCaptureInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        if input.flush_idle_editor_input() {
            self.pending_save_frontiers = None;
        }
        let cancellation = input.cancellation();
        let mut wanted = Vec::new();
        input.visit_plugin_devices(|owner, device, state| {
            if let Some((source_index, source)) =
                self.sources.iter().enumerate().rev().find(|(_, source)| {
                    source.accepted
                        && source.project_generation == input.context().project_generation
                        && source.owner == owner
                        && source.device == device
                })
            {
                if source.retiring {
                    return Err("vst3 capture source is retiring".into());
                }
                if self.replay_state_is_unconfirmed(source.binding) {
                    return Err(
                        "vst3 state capture refused · editor replay recovery is unconfirmed".into(),
                    );
                }
                wanted
                    .try_reserve(1)
                    .map_err(|_| "vst3 capture inventory allocation failed")?;
                wanted.push(Entry {
                    source_index,
                    binding: source.binding,
                    owner,
                    device,
                    expected: state.clone(),
                    request: None,
                    captured: None,
                    refused: 0,
                    done: false,
                    idle_editor: None,
                    editor_frontier: None,
                });
            }
            // No actual hosted instance: preserve its canonical opaque state.
            Ok(())
        })?;
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let control = output
            .and_then(|output| output.publication.control())
            .ok_or("vst3 capture output is unavailable")?;
        for entry in &wanted {
            if !edits::original_state(control, entry.binding, Some(cancellation))?
                .processing_eq(&entry.expected)
            {
                loop {
                    edits::check_cancel(Some(cancellation))?;
                    match control.native_source_approves_captured_state_on_control(
                        entry.binding,
                        &entry.expected,
                    ) {
                        Some(true) => break,
                        Some(false) => {
                            return Err("vst3 capture source differs from canonical device".into());
                        }
                        None => {
                            let _ =
                                edits::original_state(control, entry.binding, Some(cancellation))?;
                            edits::poll_control(control);
                        }
                    }
                }
            }
        }
        let idle_source = if input.flush_idle_editor_input() {
            self.flush_idle_editor_before_save(control, input.context(), cancellation, &mut wanted)?
        } else {
            None
        };
        if input.flush_idle_editor_input() {
            // Read each binding's editor admission frontier *before* state
            // capture begins so any edit arriving during or after Save will
            // strictly exceed the saved frontier and still trigger a warning
            // on close.
            for entry in &mut wanted {
                entry.editor_frontier =
                    self.read_editor_frontier_before_save(control, entry.binding, cancellation)?;
            }
        }
        let mut failure = None;
        while wanted.iter().any(|entry| !entry.done) {
            if cancellation.is_cancelled() && failure.is_none() {
                failure = Some("vst3 state capture cancelled".to_owned());
            }
            for entry in &mut wanted {
                if entry.done {
                    continue;
                }
                if entry.request.is_none() {
                    if failure.is_some() {
                        entry.done = true;
                        continue;
                    }
                    match control.begin_native_source_state_capture_on_control(entry.binding) {
                        Ok(request) => entry.request = Some(request),
                        Err("native preparation is busy" | "native capture mailbox is busy") => {
                            continue;
                        }
                        Err(reason) => {
                            failure.get_or_insert_with(|| reason.to_owned());
                            entry.done = true;
                            continue;
                        }
                    }
                }
                let Some(request) = entry.request.as_ref() else {
                    continue;
                };
                match control.poll_native_source_state_capture_on_control(entry.binding, request) {
                    Ok(NativeWorkerCaptureProgress::Pending)
                    | Err("native preparation is busy") => {}
                    Ok(NativeWorkerCaptureProgress::Captured(state)) => {
                        if failure.is_none() {
                            entry.captured = Some(state);
                        }
                        entry.done = true;
                        entry.request = None;
                    }
                    Ok(NativeWorkerCaptureProgress::Failed) => {
                        failure.get_or_insert_with(|| "vst3 state capture failed".to_owned());
                        entry.done = true;
                        entry.request = None;
                    }
                    Ok(NativeWorkerCaptureProgress::Refused(refusal)) => {
                        // The mailbox is free; a retry begins a fresh capture.
                        entry.request = None;
                        if let Err(reason) = refused_capture(&mut entry.refused, refusal) {
                            failure.get_or_insert(reason);
                            entry.done = true;
                        }
                    }
                    Err(reason) => {
                        failure.get_or_insert_with(|| reason.to_owned());
                        entry.done = true;
                        entry.request = None;
                    }
                }
            }
            if wanted.iter().any(|entry| !entry.done) {
                edits::poll_control(control);
            }
        }
        if cancellation.is_cancelled() && failure.is_none() {
            failure = Some("vst3 state capture cancelled".to_owned());
        }
        if let Some(reason) = failure {
            return Err(reason);
        }
        let mut captures = Vec::new();
        captures
            .try_reserve_exact(wanted.len())
            .map_err(|_| "vst3 captured state allocation failed")?;
        let mut staged_frontiers = Vec::new();
        if input.flush_idle_editor_input() {
            let _ = staged_frontiers.try_reserve_exact(wanted.len());
        }
        for mut entry in wanted {
            // All tokens have reached their actual terminals. A late source
            // close/replacement still refuses data before snapshot publication.
            let selection = edits::armed_selection(control, entry.binding, Some(cancellation))?;
            if let Some(selection) = selection.as_ref() {
                let host = self
                    .host
                    .as_ref()
                    .ok_or("qualified vst3 helper unavailable")?;
                let state = entry
                    .captured
                    .as_ref()
                    .ok_or("vst3 capture terminal is missing")?;
                let canonical_bases =
                    if edits::original_state(control, entry.binding, Some(cancellation))?
                        .processing_eq(&entry.expected)
                    {
                        None
                    } else {
                        Some(edits::canonical_bases(
                            host,
                            control,
                            entry.binding,
                            &entry.expected,
                            selection,
                            Some(cancellation),
                            None,
                        )?)
                    };
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                let mut scratch = loop {
                    if cancellation.is_cancelled() {
                        return Err("vst3 state capture cancelled".into());
                    }
                    match host.begin_saved_base_capture_on_control(
                        control,
                        entry.binding,
                        &entry.expected,
                        state,
                        selection,
                        canonical_bases.as_ref(),
                    ) {
                        Ok(scratch) => break scratch,
                        Err(error)
                            if matches!(
                                error.as_str(),
                                "native preparation is busy" | "native source is busy"
                            ) && std::time::Instant::now() < deadline =>
                        {
                            edits::poll_control(control)
                        }
                        Err(error) => return Err(error),
                    }
                };
                // Cancellation stops work, then keeps polling until the private
                // source actually disposes. Its Root retains every native owner.
                loop {
                    let mut cancelled =
                        || cancellation.is_cancelled() || std::time::Instant::now() >= deadline;
                    if let Some(terminal) = scratch.poll_on_control(control, &mut cancelled) {
                        entry.captured = Some(terminal.map_err(|error| error.to_string())?);
                        break;
                    }
                    edits::poll_control(control);
                }
            }
            if input.flush_idle_editor_input()
                && let Some(frontier) = entry.editor_frontier
            {
                staged_frontiers.push((entry.binding, frontier));
            }
            captures.push(PluginStateCapture {
                owner: entry.owner,
                device_id: entry.device,
                expected: entry.expected,
                captured: entry.captured.ok_or("vst3 capture terminal is missing")?,
            });
        }
        if idle_source
            .is_some_and(|source| control.status.native_parameter_source(false) != Some(source))
        {
            return Err(SAVE_TRANSPORT_CHANGED.into());
        }
        if input.flush_idle_editor_input() && !staged_frontiers.is_empty() {
            self.pending_save_frontiers =
                Some((input.context().project_generation, staged_frontiers));
        }
        Ok(captures)
    }

    fn read_editor_frontier_before_save(
        &self,
        control: &mut PublicationControl,
        binding: Binding,
        cancellation: &RequestCancellation,
    ) -> Result<Option<u64>, String> {
        if let Some(held) = self.held_editor_close_frontier(binding) {
            return Ok(Some(held));
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        let request = loop {
            edits::check_cancel(Some(cancellation))?;
            match control.begin_native_source_controller_query_on_control(
                binding,
                ControllerRequest::ReadEditorFrontier,
            ) {
                Ok(request) => break request,
                Err(
                    "native preparation is busy"
                    | "native source is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) if Instant::now() < deadline => {
                    edits::poll_control(control);
                }
                Err(_) => return Ok(None),
            }
        };
        loop {
            match control.poll_native_source_controller_query_on_control(binding, &request) {
                Ok(NativeWorkerControllerProgress::EditorFrontier(frontier)) => {
                    return Ok(Some(frontier));
                }
                Ok(NativeWorkerControllerProgress::Pending)
                | Err("native preparation is busy" | "native source is busy") => {
                    edits::poll_control(control);
                }
                _ => return Ok(None),
            }
        }
    }

    pub(in crate::native_audio) fn finish_state_save(
        &mut self,
        context: AudioSourceContext,
        saved: bool,
    ) {
        let Some((generation, staged)) = self.pending_save_frontiers.take() else {
            return;
        };
        if !saved || generation != context.project_generation {
            return;
        }
        for (binding, frontier) in staged {
            self.record_saved_editor_frontier(binding, frontier);
        }
    }

    /// A normal idle worker can apply pending SDK editor callbacks without
    /// PCM. Poll the already existing reader, in rounds across Save's exact
    /// sources, while the usual Session poll is unavailable. A stopped original
    /// worker drains through its joined SDK instance without restarting PCM.
    fn flush_idle_editor_before_save(
        &mut self,
        control: &mut PublicationControl,
        context: AudioSourceContext,
        cancellation: &RequestCancellation,
        wanted: &mut [Entry],
    ) -> Result<Option<(AppliedAudioSource, u64)>, String> {
        let Some(witness @ (source, _)) = control.status.native_parameter_source(false) else {
            return Ok(None);
        };
        if source.context.project_generation != context.project_generation
            || self.recording_binding().is_some()
        {
            return Ok(None);
        }
        for entry in wanted.iter_mut() {
            if entry.binding.root == source.output_identity {
                entry.idle_editor = Some(
                    if self.sources[entry.source_index].edit_recovery.is_some() {
                        IdleEditorRead::Previous
                    } else {
                        IdleEditorRead::Save
                    },
                );
            }
        }
        if wanted.iter().all(|entry| entry.idle_editor.is_none()) {
            return Ok(None);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while wanted.iter().any(|entry| entry.idle_editor.is_some()) {
            for entry in wanted
                .iter_mut()
                .filter(|entry| entry.idle_editor.is_some())
            {
                edits::check_cancel(Some(cancellation))?;
                if control.status.native_parameter_source(false) != Some(witness) {
                    return Err(SAVE_TRANSPORT_CHANGED.into());
                }
                let drained =
                    self.poll_idle_save_editor_input(control, witness, entry.source_index)?;
                if entry.idle_editor == Some(IdleEditorRead::Previous) {
                    if self.sources[entry.source_index].edit_recovery.is_none() {
                        // The earlier poll might have sampled before Save or
                        // used a non-flushing mode. Ask for a fresh idle read.
                        entry.idle_editor = Some(IdleEditorRead::Save);
                    }
                } else if drained {
                    entry.idle_editor = None;
                }
            }
            if wanted.iter().any(|entry| entry.idle_editor.is_some()) {
                if Instant::now() >= deadline {
                    return Err("vst3 editor input did not settle · retry save".into());
                }
                edits::poll_control(control);
            }
        }
        Ok(Some(witness))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_changing_retries_a_bounded_number_of_times_then_fails_specifically() {
        let mut attempts = 0;
        for _ in 1..STILL_CHANGING_ATTEMPTS {
            assert_eq!(
                refused_capture(&mut attempts, StateCaptureRefusal::StillChanging),
                Ok(())
            );
        }
        let failure = refused_capture(&mut attempts, StateCaptureRefusal::StillChanging)
            .expect_err("bounded retries");
        assert_eq!(failure, STILL_CHANGING);
        assert_eq!(failure, failure.to_lowercase());
        assert_eq!(attempts, STILL_CHANGING_ATTEMPTS);
    }

    #[test]
    fn oversized_state_fails_without_retry() {
        let mut attempts = 0;
        let failure = refused_capture(
            &mut attempts,
            StateCaptureRefusal::ExceedsLimit {
                observed_bytes: 6000,
            },
        )
        .expect_err("no retry for oversized state");
        assert!(failure.starts_with("vst3 state capture refused: plugin state exceeds"));
        assert_eq!(attempts, 0);
    }
}
