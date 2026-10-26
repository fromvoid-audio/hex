//! Existing canonical worker's bounded native capture batch. No native owners
//! or extra queue/reader move here. Every admitted token is drained, including
//! failed/cancelled saves, before its caller-owned metadata is released.
use super::*;
use hex_audio::plugin_worker::{NativeWorkerCapture, NativeWorkerCaptureProgress};
use hex_session::service::{PluginStateCapture, PluginStateSaveInput};

struct Entry {
    binding: Binding,
    owner: ProcessorChainOwner,
    device: u64,
    expected: NativeDeviceState,
    request: Option<NativeWorkerCapture>,
    captured: Option<NativeDeviceState>,
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
                    request: None,
                    captured: None,
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
        for entry in wanted {
            // All tokens have reached their actual terminals. A late source
            // close/replacement still refuses data before snapshot publication.
            control.native_source_metadata(entry.binding)?;
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
