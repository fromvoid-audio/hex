//! Existing canonical worker's bounded native capture batch. No native owners
//! or extra queue/reader move here. Every admitted token is drained, including
//! failed/cancelled saves, before its caller-owned metadata is released.
use super::*;
use hex_audio::plugin_worker::{
    NativeWorkerCapture, NativeWorkerCaptureProgress, StateCaptureRefusal,
};
use hex_session::service::{PluginStateCapture, PluginStateSaveInput};

/// Whole-capture attempts per device while the helper reports that the plugin
/// is still changing. Each refused attempt already spent the helper's settle
/// bound (about one second), so Save gives up after a few seconds.
const STILL_CHANGING_ATTEMPTS: u8 = 3;
const STILL_CHANGING: &str =
    "vst3 state kept changing during save · stop editing the plugin, then save again";

/// Save's disposition of one helper capture refusal: `Ok` retries the whole
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
        StateCaptureRefusal::ExceedsLimit { .. } => Err(format!("vst3 save refused: {refusal}")),
    }
}

struct Entry {
    binding: Binding,
    owner: ProcessorChainOwner,
    device: u64,
    expected: NativeDeviceState,
    authored: Option<hex_audio::plugin_effects::AuthoredSelection>,
    request: Option<NativeWorkerCapture>,
    captured: Option<NativeDeviceState>,
    /// Refused-as-still-changing attempts so far.
    refused: u8,
    done: bool,
}

impl NativeAudioSink {
    pub(in crate::native_audio) fn capture_plugin_save(
        &mut self,
        input: PluginStateSaveInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        // Save captures exact accepted native bindings, not the graph's loop
        // tag. A pending gain/loop publication may retain those same instances;
        // refusing it would break Save queued behind an accepted state edit.
        self.plugins.required_save_sources(&input)?;
        self.plugins.capture_save(self.output.as_mut(), input)
    }
}

impl Plugins {
    fn required_save_sources(&self, input: &PluginStateSaveInput<'_>) -> Result<(), String> {
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
                return Err("vst3 save source preparation is pending".into());
            }
            Ok(())
        })?;
        if required && self.hold_reopen(input.context()) {
            return Err("vst3 state restoration owns save preparation".into());
        }
        Ok(())
    }

    fn capture_save(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        input: PluginStateSaveInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        let cancellation = input.cancellation();
        let mut wanted = Vec::new();
        input.visit_plugin_devices(|owner, device, state| {
            if let Some(source) = self.sources.iter().rev().find(|source| {
                source.accepted
                    && source.project_generation == input.context().project_generation
                    && source.owner == owner
                    && source.device == device
            }) {
                if source.retiring {
                    return Err("vst3 save source is retiring".into());
                }
                wanted
                    .try_reserve(1)
                    .map_err(|_| "vst3 save inventory allocation failed")?;
                wanted.push(Entry {
                    binding: source.binding,
                    owner,
                    device,
                    expected: state.clone(),
                    authored: hex_audio::plugin_effects::AuthoredSelection::from_targets(
                        input.authored_plugin_targets(device),
                    )?,
                    request: None,
                    captured: None,
                    refused: 0,
                    done: false,
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
            .ok_or("vst3 save output is unavailable")?;
        for entry in &wanted {
            if !edits::original_state(control, entry.binding, Some(cancellation))?
                .processing_eq(&entry.expected)
            {
                return Err("vst3 save source differs from canonical device".into());
            }
        }
        let mut failure = None;
        while wanted.iter().any(|entry| !entry.done) {
            if cancellation.is_cancelled() && failure.is_none() {
                failure = Some("vst3 save cancelled by document request or shutdown".to_owned());
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
            failure = Some("vst3 save cancelled by document request or shutdown".to_owned());
        }
        if let Some(reason) = failure {
            return Err(reason);
        }
        let mut captures = Vec::new();
        captures
            .try_reserve_exact(wanted.len())
            .map_err(|_| "vst3 saved state allocation failed")?;
        for mut entry in wanted {
            // All tokens have reached their actual terminals. A late source
            // close/replacement still refuses data before snapshot publication.
            control.native_source_metadata(entry.binding)?;
            if let Some(selection) = entry.authored.as_ref() {
                let host = self
                    .host
                    .as_ref()
                    .ok_or("qualified vst3 helper unavailable")?;
                let state = entry
                    .captured
                    .as_ref()
                    .ok_or("vst3 capture terminal is missing")?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                let mut scratch = loop {
                    if cancellation.is_cancelled() {
                        return Err("vst3 save cancelled".into());
                    }
                    match host.begin_saved_base_capture_on_control(
                        control,
                        entry.binding,
                        &entry.expected,
                        state,
                        selection,
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
                        entry.captured = Some(terminal?);
                        break;
                    }
                    edits::poll_control(control);
                }
            }
            captures.push(PluginStateCapture {
                owner: entry.owner,
                device_id: entry.device,
                expected: entry.expected,
                captured: entry.captured.ok_or("vst3 capture terminal is missing")?,
            });
        }
        Ok(captures)
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
        assert!(failure.starts_with("vst3 save refused: plugin state exceeds"));
        assert_eq!(attempts, 0);
    }
}
