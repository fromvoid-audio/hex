//! Stopped renewal borrows Root's existing source fence and original return
//! carriers. These scalar stages never stand in for graph/endpoint return or ACK.
use super::*;
use hex_audio::prepared::{
    PublicationControl, PublicationPolicy, StoppedEditFence, StoppedEditFenceState,
};

pub(super) struct Restore {
    context: AudioSourceContext,
    source: hex_audio::prepared::AppliedAudioSource,
    fence: StoppedEditFence,
    stage: Stage,
    result: Option<Result<(), String>>,
    submitted: Option<u64>,
}
enum Stage {
    Fence,
    Return,
    Renew(usize),
    RefreshLatency(Binding),
    Prepare(usize),
    Ready,
}
impl Restore {
    pub(super) fn submitted(&self) -> bool {
        self.submitted.is_some()
    }
    pub(super) fn output_closed(&mut self) {
        self.result = Some(Err("stopped vst3 restore output changed".into()));
    }
}
impl Plugins {
    pub fn prepare_restore(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        if let Some(restore) = self.restore.as_ref() {
            if restore.context != context {
                return Err("stopped vst3 restore source changed".into());
            }
            return restore.result.as_ref().map_or(Ok(Some(false)), |result| {
                result.as_ref().map(|()| Some(true)).map_err(Clone::clone)
            });
        }
        if !self
            .sources
            .iter()
            .any(|source| source.accepted && !source.retiring)
        {
            return Ok(None);
        }
        let control = output.publication.control().ok_or("native output closed")?;
        // Constructor/sampler restoration has its own existing owner. This
        // adapter renews only the ordinary graphs it compiled from its sources.
        if control.status.applied_audio_source().is_some() {
            return Ok(None);
        }
        let source = control
            .status
            .recording_source()
            .ok_or("ordinary vst3 restore source unavailable")?;
        if self.pending.is_some() || source.context != context {
            return Err("stopped vst3 source is not current".into());
        }
        let fence = control
            .request_ordinary_native_epoch_fence(source)
            .map_err(str::to_owned)?;
        self.restore = Some(Restore {
            context,
            source,
            fence,
            stage: Stage::Fence,
            result: None,
            submitted: None,
        });
        Ok(Some(false))
    }
    pub fn restore_ready(&self) -> bool {
        self.restore
            .as_ref()
            .is_some_and(|restore| restore.result.is_some())
    }
    pub fn cancel_restore(&mut self, control: Option<&mut PublicationControl>) {
        if self
            .restore
            .as_ref()
            .is_some_and(|restore| restore.submitted.is_some())
        {
            return;
        }
        if let Some(restore) = self.restore.take()
            && let Some(control) = control
            && control.output_identity() == Some(restore.source.output_identity)
        {
            control.release_stopped_edit_fence(restore.fence);
        }
    }
    pub fn restore_policy(
        &self,
        context: AudioSourceContext,
    ) -> Result<Option<PublicationPolicy>, String> {
        let Some(restore) = self.restore.as_ref() else {
            return Ok(None);
        };
        if restore.context != context
            || !matches!(restore.result, Some(Ok(())))
            || restore.submitted.is_some()
        {
            return Err("stopped vst3 restoration is not ready for publication".into());
        }
        Ok(Some(PublicationPolicy::InstallUnderNativeEpoch(
            restore.fence,
        )))
    }
    pub fn restore_submitted(&mut self, revision: u64) {
        if let Some(restore) = self.restore.as_mut() {
            restore.submitted = Some(revision);
        }
    }
    pub fn poll_restore(
        &mut self,
        control: &mut PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        let Some(restore) = self.restore.as_mut() else {
            return;
        };
        if let Some(revision) = restore.submitted {
            if control.output_identity() == Some(restore.source.output_identity)
                && (control.status.applied_revision.load(Acquire) == revision
                    || control
                        .status
                        .rejected_revision()
                        .is_some_and(|(rejected, _)| rejected == revision))
            {
                control.release_stopped_edit_fence(restore.fence);
                self.restore = None;
            }
            return;
        }
        if restore.result.is_some() {
            return;
        }
        let result = (|| {
            if context != Some(restore.context)
                || control.output_identity() != Some(restore.source.output_identity)
            {
                return Err("stopped vst3 restore source changed".into());
            }
            match restore.stage {
                Stage::Fence => match control.stopped_edit_fence_state(restore.fence) {
                    StoppedEditFenceState::Pending => {}
                    StoppedEditFenceState::Refused => {
                        // The editor holds Play until this restore settles;
                        // another transport source started playback first.
                        return Err("playback started before vst3 effects were restored".into());
                    }
                    StoppedEditFenceState::Ready => restore.stage = Stage::Return,
                },
                Stage::Return => match control
                    .return_stopped_ordinary_native_epoch_runtime(restore.source, restore.fence)
                {
                    Ok(()) => restore.stage = Stage::Renew(0),
                    Err("native epoch publication is busy") => {}
                    Err(reason) => return Err(reason.into()),
                },
                Stage::Renew(index) => {
                    if let Some(source) = self.sources.get_mut(index) {
                        if source.accepted && !source.retiring {
                            match control.quiesce_native_source_preparation(
                                source.binding,
                                source.binding.graph,
                            ) {
                                Ok(binding) => {
                                    source.binding = binding;
                                    source.queued = false;
                                    if source.observes_latency {
                                        restore.stage = Stage::RefreshLatency(binding);
                                        return Ok(false);
                                    }
                                }
                                Err(
                                    "native preparation is busy"
                                    | "native parameter mailbox is busy"
                                    | "native epoch requires parameter input terminal"
                                    | "native epoch requires original graph and endpoint return",
                                ) => return Ok(false),
                                Err(reason) => return Err(reason.into()),
                            }
                        }
                        restore.stage = Stage::Renew(index + 1);
                    } else {
                        restore.stage = Stage::Prepare(0);
                    }
                }
                Stage::RefreshLatency(binding) => {
                    let index = self
                        .sources
                        .iter()
                        .position(|source| source.binding == binding && !source.retiring)
                        .ok_or("vst3 latency source changed")?;
                    match control.settle_native_latency_on_control(binding) {
                        Ok(Some(_)) => restore.stage = Stage::Renew(index + 1),
                        Err("native preparation is busy") => {}
                        Ok(None) => {
                            return Err(control
                                .native_source_preparation_error(binding)
                                .unwrap_or_else(|| "vst3 latency reactivation refused".into()));
                        }
                        Err(reason) => return Err(reason.into()),
                    }
                }
                Stage::Prepare(index) => {
                    let Some(source) = self.sources.get_mut(index) else {
                        restore.stage = Stage::Ready;
                        return Ok(true);
                    };
                    if !source.accepted || source.retiring {
                        restore.stage = Stage::Prepare(index + 1);
                        return Ok(false);
                    }
                    #[cfg(not(all(test, feature = "allocation-probe")))]
                    let presentation = None;
                    #[cfg(all(test, feature = "allocation-probe"))]
                    let presentation = self.presentation;
                    let host = self
                        .host
                        .as_ref()
                        .ok_or("qualified vst3 helper unavailable")?;
                    if advance_source(host, presentation, control, source)?.is_some() {
                        restore.stage = Stage::Prepare(index + 1);
                    }
                }
                Stage::Ready => return Ok(true),
            }
            Ok(false)
        })();
        match result {
            Ok(true) => restore.result = Some(Ok(())),
            Ok(false) => {}
            Err(error) => restore.result = Some(Err(bounded_error(error))),
        }
    }
}
