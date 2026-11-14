//! Native seeks use the existing bounded 8 MiB seek worker. Canonical control retains real
//! same-scope configuration/revision anchors and finite capture/runtime allowances until actual
//! result transfer or post-drop completion. Cancellation and worker idle are not release signals.
use super::*;
use crate::arrangement_seek::{ArrangementSeekMode, ArrangementSeekSpec, MAX_RESIDENT_SEEK_JOBS};
use hex_engine::native_device_resources::DeviceResourceAdmission;

/// Explicit native control intent. Ordinary UI requests keep Fresh; Preserve
/// reconstructs bounded linear history and refuses sections/transport loops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResidentSeekHistory {
    #[default]
    Fresh,
    Preserve,
}

fn preparation_mode(
    history: ResidentSeekHistory,
    section: Option<crate::arrangement_seek::SectionLaunch>,
) -> Result<ArrangementSeekMode, String> {
    match (history, section) {
        (ResidentSeekHistory::Preserve, Some(_)) => {
            Err("history-preserving sections are not supported".into())
        }
        (ResidentSeekHistory::Preserve, None) => {
            Ok(ArrangementSeekMode::HistoryPreservingInteractive)
        }
        (ResidentSeekHistory::Fresh, Some(section)) => Ok(section.mode()),
        (ResidentSeekHistory::Fresh, None) => Ok(ArrangementSeekMode::Interactive),
    }
}

/// Scalar control intent; prepared graphs, original PCM and runtime owners stay worker-private.
#[derive(Clone, Debug)]
pub struct ResidentArrangementSeekIntent {
    pub request_id: u64,
    pub project_generation: u64,
    pub project_revision: u64,
    pub destination_frame: u64,
    pub start_playing: bool,
    pub history: ResidentSeekHistory,
    pub section: Option<crate::arrangement_seek::SectionLaunch>,
    /// An ordinary playing seek with no section loop edit. Section launches
    /// carry their quantization in the section intent.
    pub locator_quantization: Option<hex_engine::LaunchQuantization>,
    pub fence: ResidentFreshRuntimeFence,
    pub device_preview: Option<DevicePreviewIdentity>,
    pub tuning_preview: Option<TuningPreviewIdentity>,
    pub cancelled: Arc<AtomicBool>,
}
/// Submission is distinct from the later callback's exact output/context/revision ACK.
#[derive(Debug)]
pub struct ResidentArrangementSeekResult {
    pub request_id: u64,
    pub project_generation: u64,
    pub project_revision: u64,
    pub result: Result<ResidentPublicationAcceptance, String>,
}
fn selected_source<'a>(
    intent: &ResidentArrangementSeekIntent,
    canonical: &'a resident_audio::Snapshot,
    preview: Option<&'a resident_preview::Preview>,
) -> Result<&'a resident_audio::Snapshot, String> {
    match (intent.device_preview, intent.tuning_preview, preview) {
        (None, None, None) => Ok(canonical),
        (Some(device), None, Some(preview))
            if preview.device_identity() == Some(device)
                && device.project_generation == intent.project_generation
                && device.canonical_revision == intent.project_revision =>
        {
            Ok(&preview.snapshot)
        }
        (None, Some(tuning), Some(preview))
            if preview.tuning_identity() == Some(tuning)
                && tuning.project_generation == intent.project_generation
                && tuning.canonical_revision == intent.project_revision =>
        {
            Ok(&preview.snapshot)
        }
        _ => Err("seek preview changed".into()),
    }
}
fn result_matches_intent(
    result: &crate::arrangement_seek::ArrangementSeekResult,
    intent: &ResidentArrangementSeekIntent,
    prepared: &Arc<hex_engine::PreparedRevision>,
) -> bool {
    preparation_mode(intent.history, intent.section).is_ok_and(|mode| result.mode == mode)
        && result.generation == intent.project_generation
        && result.revision == intent.project_revision
        && Arc::ptr_eq(&result.prepared_revision, prepared)
        && result.destination_frame == intent.destination_frame
        && result.work_budget_frames == crate::arrangement_seek::MAX_SEEK_WORK_FRAMES
        && result.start_playing == intent.start_playing
}

struct Active {
    job: u64,
    intent: ResidentArrangementSeekIntent,
    snapshot: resident_audio::Snapshot,
    allowance: usize,
    completion: Option<ResidentArrangementSeekResult>,
}
pub(crate) struct Lane {
    worker: Option<ArrangementSeekWorker>,
    active: [Option<Active>; MAX_RESIDENT_SEEK_JOBS],
    results: SyncSender<ResidentArrangementSeekResult>,
    next_job: u64,
    rejected: Option<ResidentArrangementSeekResult>,
}
impl Lane {
    /// An admitted job or an undelivered rejection still needs worker polls.
    pub(super) fn pending(&self) -> bool {
        self.rejected.is_some() || self.active.iter().any(Option::is_some)
    }
    pub(super) fn views(&self) -> impl Iterator<Item = DeviceResourceAdmission<'_>> {
        self.active.iter().flatten().map(|job| {
            DeviceResourceAdmission::prepared_on_control(
                &job.snapshot.resources,
                &job.snapshot.prepared,
            )
        })
    }
    pub(super) fn new(
        worker: ArrangementSeekWorker,
        results: SyncSender<ResidentArrangementSeekResult>,
    ) -> Self {
        Self {
            worker: Some(worker),
            active: std::array::from_fn(|_| None),
            results,
            next_job: 1,
            rejected: None,
        }
    }
    pub(super) fn reject(&mut self, intent: &ResidentArrangementSeekIntent, error: String) {
        // Service reserves one completion slot before admitting each request. Full is therefore
        // unreachable through that endpoint; retain a defensive result instead of losing it.
        if let Err(TrySendError::Full(result)) =
            self.results.try_send(completion(intent, Err(error)))
        {
            self.rejected = Some(result);
        }
    }
    pub(super) fn allowance(&self) -> Result<usize, String> {
        self.active
            .iter()
            .flatten()
            .try_fold(std::mem::size_of::<Self>(), |n, job| {
                n.checked_add(job.allowance)
                    .ok_or_else(|| "seek storage allowance overflow".into())
            })
    }
    #[allow(clippy::too_many_arguments)] // One canonical borrowed admission boundary, no owner projection.
    pub(super) fn submit(
        &mut self,
        intent: ResidentArrangementSeekIntent,
        session: &Session,
        current: &resident_audio::Snapshot,
        sink: &dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        if self.rejected.is_some() {
            return Err("seek completion delivery is pending".into());
        }
        if intent.request_id == 0
            || intent.project_generation != context.project_generation
            || intent.project_revision != context.editor_revision
            || intent.cancelled.load(Ordering::Acquire)
        {
            return Err("seek target changed or was cancelled".into());
        }
        if intent.section.is_some_and(|section| {
            !section.matches_loop(session.project().loop_region())
                || section.region.start() != intent.destination_frame
                || !intent.start_playing
        }) {
            return Err("section launch requires its exact start and loop state".into());
        }
        if intent.locator_quantization.is_some()
            && (intent.history != ResidentSeekHistory::Fresh
                || intent.section.is_some()
                || !intent.start_playing)
        {
            return Err("locator quantization requires an ordinary playing seek".into());
        }
        let mode = preparation_mode(intent.history, intent.section)?;
        let selected = selected_source(&intent, current, admission.preview)?;
        let files = selected
            .file_sources
            .as_ref()
            .map(|sources| {
                sink.resident_file_seek_preparer()
                    .map(|prepare| (sources.clone(), prepare))
                    .ok_or_else(|| "file-backed seek preparation is unavailable".to_string())
            })
            .transpose()?;
        let file_storage = selected.file_sources.as_ref().map_or(Ok(0), |sources| {
            sink.resident_file_seek_storage_bytes(&selected.prepared, sources)
        })?;
        let index = self
            .active
            .iter()
            .position(Option::is_none)
            .ok_or("seek retirement is pending")?;
        let limits = session
            .native_reservation
            .ok_or("resident resource reservation unavailable")?;
        let runtime = intent
            .section
            .filter(|section| section.region.enabled())
            .map_or_else(
                || hex_engine::SessionPlayback::interactive_runtime_allowance(&selected.prepared),
                |section| {
                    hex_engine::SessionPlayback::section_runtime_allowance(
                        &selected.prepared,
                        section.region,
                    )
                },
            )
            .map_err(|e| e.to_string())?;
        let input_capture = selected
            .inputs
            .capture_bytes()
            .map_err(|e| e.to_string())?
            .checked_add(std::mem::size_of_val(&files))
            .and_then(|n| n.checked_add(file_storage))
            .ok_or("seek storage allowance overflow")?;
        let capture_quote = selected
            .resources
            .metadata_bytes()
            .map_err(|e| format!("seek metadata: {e:?}"))?
            .checked_mul(2)
            .and_then(|n| n.checked_add(runtime))
            .and_then(|n| n.checked_add(input_capture))
            .ok_or("seek storage allowance overflow")?;
        let old_views = self.views().collect::<Vec<_>>();
        let registered = admission
            .registered
            .iter()
            .copied()
            .chain(old_views)
            .collect::<Vec<_>>();
        session
            .admit_resident_seek(
                current,
                sink,
                sampler_export::Admission {
                    registered: &registered,
                    ..admission
                },
                self.allowance()?
                    .checked_add(capture_quote)
                    .ok_or("seek storage allowance overflow")?,
            )
            .map_err(|e| e.to_string())?;
        drop(registered);
        let anchor = selected
            .resources
            .capture_on_control(limits.storage.max_retained_bytes)
            .map_err(|e| format!("seek preparation: {e:?}"))?;
        let capture = anchor
            .capture_on_control(limits.storage.max_retained_bytes)
            .map_err(|e| format!("seek preparation: {e:?}"))?;
        // This is a conservative metadata allowance for an actual captured wrapper, not another
        // original-bank frame charge. Runtime/scratch uses the existing checked engine quote.
        let allowance = capture
            .metadata_bytes()
            .map_err(|e| format!("seek preparation: {e:?}"))?
            .checked_add(
                intent
                    .section
                    .filter(|section| section.region.enabled())
                    .map_or_else(
                        || {
                            hex_engine::SessionPlayback::interactive_runtime_allowance(
                                &selected.prepared,
                            )
                        },
                        |section| {
                            hex_engine::SessionPlayback::section_runtime_allowance(
                                &selected.prepared,
                                section.region,
                            )
                        },
                    )
                    .map_err(|e| format!("seek preparation: {e:?}"))?,
            )
            .and_then(|bytes| bytes.checked_add(input_capture))
            .ok_or("seek storage allowance overflow")?;
        let views = self.views().chain(std::iter::once(
            DeviceResourceAdmission::prepared_on_control(&anchor, &selected.prepared),
        ));
        let registered = admission
            .registered
            .iter()
            .copied()
            .chain(views)
            .collect::<Vec<_>>();
        let admission = sampler_export::Admission {
            registered: &registered,
            ..admission
        };
        session
            .admit_resident_seek(
                current,
                sink,
                admission,
                self.allowance()?
                    .checked_add(allowance)
                    .ok_or("seek storage allowance overflow")?,
            )
            .map_err(|e| format!("seek preparation: {e:?}"))?;
        let job = self.next_job;
        let next = job.checked_add(1).ok_or("seek job identity exhausted")?;
        self.worker
            .as_ref()
            .ok_or("seek worker unavailable")?
            .submit_resident(
                ArrangementSeekSpec {
                    mode,
                    generation: intent.project_generation,
                    revision: intent.project_revision,
                    prepared_revision: Arc::clone(&selected.prepared),
                    destination_frame: intent.destination_frame,
                    work_budget_frames: crate::arrangement_seek::MAX_SEEK_WORK_FRAMES,
                    start_playing: intent.start_playing,
                },
                job,
                capture,
                Arc::clone(&intent.cancelled),
                files,
            )
            .map_err(|e| format!("seek worker refused: {e:?}"))?;
        drop(registered);
        self.next_job = next;
        self.active[index] = Some(Active {
            job,
            intent,
            snapshot: resident_audio::Snapshot {
                source_origin: selected.source_origin,
                prepared_context: selected.prepared_context,
                file_sources: selected.file_sources.clone(),
                prepared: Arc::clone(&selected.prepared),
                resources: anchor,
                inputs: selected.inputs.clone(),
            },
            allowance,
            completion: None,
        });
        Ok(())
    }
    pub(super) fn drive<'audio>(
        &mut self,
        session: &Session,
        current: Option<&mut resident_audio::Snapshot>,
        sink: Option<&mut (dyn WorkerAudioSink + 'audio)>,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
    ) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        if let Some(result) = self.rejected.take()
            && let Err(TrySendError::Full(result)) = self.results.try_send(result)
        {
            self.rejected = Some(result);
        }
        while let Some(id) = worker.try_resident_completion() {
            if let Some(job) = self.active.iter_mut().flatten().find(|job| job.job == id) {
                job.completion = Some(completion(
                    &job.intent,
                    Err("seek cancelled or superseded".into()),
                ));
            }
        }
        if let Some((id, result)) = worker.try_resident_result()
            && let Some(index) = self
                .active
                .iter()
                .position(|job| job.as_ref().is_some_and(|job| job.job == id))
            && let Some(mut job) = self.active[index].take()
        {
            let accepted = (|| {
                let identity_matches =
                    result_matches_intent(&result, &job.intent, &job.snapshot.prepared);
                let playback = result
                    .result
                    .map_err(|e| format!("seek preparation failed: {e:?}"))?;
                if !identity_matches
                    || job.intent.cancelled.load(Ordering::Acquire)
                    || context.project_generation != job.intent.project_generation
                    || context.editor_revision != job.intent.project_revision
                    || !playback
                        .prepared_revision()
                        .is_some_and(|r| Arc::ptr_eq(r, &job.snapshot.prepared))
                    || current.as_ref().is_none_or(|current| {
                        selected_source(&job.intent, current, admission.preview)
                            .map_or(true, |selected| {
                                !Arc::ptr_eq(&selected.prepared, &job.snapshot.prepared)
                            })
                            || current.inputs.version != job.snapshot.inputs.version
                    })
                {
                    return Err("seek target changed or was cancelled".into());
                }
                let sink = sink.ok_or("native seek output unavailable")?;
                let registered = admission
                    .registered
                    .iter()
                    .copied()
                    .chain(self.views())
                    .collect::<Vec<_>>();
                let admission = sampler_export::Admission {
                    registered: &registered,
                    runtime_bytes: admission
                        .runtime_bytes
                        .checked_add(self.allowance()?)
                        .ok_or("seek storage allowance overflow")?,
                    ..admission
                };
                session
                    .publish_resident_seek(
                        &job.snapshot,
                        admission
                            .preview
                            .map_or(session.project(), |preview| preview.project.as_ref()),
                        current.ok_or("seek source unavailable")?,
                        playback,
                        sink,
                        admission,
                        ResidentPublicationMetadata {
                            source_origin: crate::service::ResidentSourceOrigin::Unknown,
                            clip_preview: None,
                            context,
                            device_preview: job.intent.device_preview,
                            tuning_preview: job.intent.tuning_preview,
                            seek_request: Some(job.intent.request_id),
                            stopped_restore: false,
                            computer_keyboard_request: None,
                            gain_recording_track: job.snapshot.prepared.gain_recording_track(),
                            requires_preroll: job
                                .snapshot
                                .prepared
                                .arrangement_seek_requires_preroll(),
                        },
                        job.intent
                            .section
                            .map(|section| section.quantization)
                            .or(job.intent.locator_quantization)
                            .map_or(
                                ResidentPublicationPolicy::FreshIfUnchanged(job.intent.fence),
                                |quantization| ResidentPublicationPolicy::QuantizedSeekWhenReady {
                                    fence: job.intent.fence,
                                    quantization,
                                },
                            ),
                        &job.intent.cancelled,
                    )
                    .map_err(|e| e.to_string())
            })();
            // Result owners have either moved into the actual sink or physically dropped above.
            job.completion = Some(completion(&job.intent, accepted));
            self.active[index] = Some(job);
        }
        for slot in &mut self.active {
            if let Some(result) = slot.as_mut().and_then(|job| job.completion.take()) {
                match self.results.try_send(result) {
                    Ok(()) => {
                        *slot = None;
                    }
                    Err(TrySendError::Full(result)) => {
                        if let Some(job) = slot.as_mut() {
                            job.completion = Some(result);
                        }
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        *slot = None;
                    }
                }
            }
        }
    }
}
fn completion(
    intent: &ResidentArrangementSeekIntent,
    result: Result<ResidentPublicationAcceptance, String>,
) -> ResidentArrangementSeekResult {
    ResidentArrangementSeekResult {
        request_id: intent.request_id,
        project_generation: intent.project_generation,
        project_revision: intent.project_revision,
        result,
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown_and_join_on_control();
        }
        // Anchors outlive the joined worker's final resource/result destruction.
    }
}

#[cfg(test)]
mod tests;
