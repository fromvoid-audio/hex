//! Background export worker thread and export destination/extent guards.
use crate::service::*;

pub(crate) fn same_existing_file(left: &Path, right: &Path) -> bool {
    hex_storage::same_existing_file(left, right)
}

pub(crate) fn export_destination_is_original_media(
    project: &Project,
    destination: &Path,
    base: Option<&Path>,
) -> bool {
    if std::fs::metadata(destination).is_err() {
        return false;
    }
    project.assets().any(|asset| {
        let source = asset.path();
        let source = if source.is_absolute() {
            source.to_path_buf()
        } else {
            base.unwrap_or(Path::new(".")).join(source)
        };
        hex_storage::same_existing_file(&source, destination)
    })
}

pub(crate) fn latest_timeline_content_end(project: &Project) -> u64 {
    let audio_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(|clip| clip.start().checked_add(clip.duration()))
        .max()
        .unwrap_or(0);
    let frozen_end = project
        .tracks()
        .iter()
        .filter_map(|track| track.frozen())
        .filter_map(|frozen| frozen.start.checked_add(frozen.duration))
        .max()
        .unwrap_or(0);
    let midi_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .filter_map(|clip| clip.start().raw().checked_add(clip.length().raw()))
        .filter_map(|raw| {
            project
                .tempo_map()
                .beat_to_frame(hex_project::BeatTime::from_raw(raw))
                .ok()
        })
        .max()
        .unwrap_or(0);
    let section_end = project
        .arrangement_sections()
        .iter()
        .filter_map(|section| project.tempo_map().beat_to_frame(section.end()).ok())
        .max()
        .unwrap_or(0);
    let locator_end = project
        .arrangement_locators()
        .last()
        .map_or(0, |locator| locator.frame());
    audio_end
        .max(frozen_end)
        .max(midi_end)
        .max(section_end)
        .max(locator_end)
}

#[cfg(test)]
pub(crate) fn run_export_worker(
    work: Receiver<ExportWork>,
    events: SyncSender<ExportEvent>,
    exporter: Exporter,
    finished: SyncSender<u64>,
    shutdown: Arc<AtomicBool>,
    wake_hook: client::WakeHook,
    #[cfg(test)] test_hook: ExportTestHook,
) {
    run_export_worker_with_recovery(
        work,
        events,
        exporter,
        finished,
        shutdown,
        wake_hook,
        Arc::new(WakeSignal::default()),
        None,
        #[cfg(test)]
        test_hook,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_export_worker_with_recovery(
    work: Receiver<ExportWork>,
    events: SyncSender<ExportEvent>,
    exporter: Exporter,
    finished: SyncSender<u64>,
    shutdown: Arc<AtomicBool>,
    wake_hook: client::WakeHook,
    drain_signal: Arc<WakeSignal>,
    recovery: Option<Arc<crate::ExportRecoveryJournal>>,
    #[cfg(test)] test_hook: ExportTestHook,
) {
    while let Ok(work) = work.recv() {
        #[cfg(test)]
        if let Some(hook) = test_hook.lock().unwrap().take() {
            hook();
        }
        let (id, mut event) = {
            let ExportWork::Render(mut job) = work;
            let result = if let Some(streamed) = job.streamed.take() {
                if job.sources.is_some() {
                    Err("export cannot combine streamed pages with private constructor source scopes".into())
                } else {
                    let controlled = job
                        .controlled
                        .take()
                        .unwrap_or_else(ControlledExport::whole_main);
                    (streamed.render)(&StreamedExportRequest {
                        foreign: std::cell::RefCell::new(
                            job.foreign
                                .as_deref_mut()
                                .map(|source| source as &mut dyn hex_export::ForeignBounceRenderer),
                        ),
                        source: &streamed.source,
                        prepared: &job.prepared,
                        resources: job.resources.as_ref(),
                        path: &job.path,
                        start: controlled.start,
                        frames: job.frames,
                        signal: &controlled.signal,
                        format: controlled.format,
                        control: &controlled.control,
                        publication: &controlled.publication,
                    })
                }
            } else if let Some(foreign) = job.foreign.as_deref_mut() {
                let controlled = job
                    .controlled
                    .take()
                    .unwrap_or_else(ControlledExport::whole_main);
                hex_export::render_prepared_with_foreign(
                    hex_export::ExportJobSpec {
                        path: &job.path,
                        revision: Arc::clone(&job.prepared),
                        resources: job.resources.as_ref(),
                        start: controlled.start,
                        frames: job.frames,
                        signal: controlled.signal,
                        format: controlled.format,
                        control: &controlled.control,
                        publication: &controlled.publication,
                    },
                    None,
                    foreign,
                )
                .map_err(ExportFailure::from)
            } else if job.sources.is_some() && job.resources.is_some() {
                Err("export cannot combine private and ordinary source scopes".into())
            } else if let Some(resources) = job.resources.take() {
                if let Some(controlled) = job.controlled.take() {
                    hex_export::render_prepared_with_device_resources_and_policy(
                        hex_export::ExportJobSpec {
                            path: &job.path,
                            revision: job.prepared,
                            resources: None,
                            start: controlled.start,
                            frames: job.frames,
                            signal: controlled.signal,
                            format: controlled.format,
                            control: &controlled.control,
                            publication: &controlled.publication,
                        },
                        &resources,
                    )
                    .map_err(ExportFailure::from)
                } else {
                    hex_export::render_prepared_new_with_device_resources(
                        &job.path,
                        job.prepared,
                        &resources,
                        0,
                        job.frames,
                    )
                    .map_err(ExportFailure::from)
                }
            } else if let Some(sources) = job.sources.take() {
                let controlled = job
                    .controlled
                    .take()
                    .unwrap_or_else(ControlledExport::whole_main);
                hex_export::render_control_sampler_with_policy(
                    hex_export::ExportJobSpec {
                        path: &job.path,
                        revision: job.prepared,
                        resources: None,
                        start: controlled.start,
                        frames: job.frames,
                        signal: controlled.signal,
                        format: controlled.format,
                        control: &controlled.control,
                        publication: &controlled.publication,
                    },
                    sources,
                )
                .map_err(ExportFailure::from)
            } else {
                // Every service request carries controlled parts; a bare job
                // renders as the automatic whole-project Main export.
                let controlled = job.controlled.unwrap_or_else(ControlledExport::whole_main);
                exporter(
                    &job.path,
                    &job.prepared,
                    controlled.start,
                    job.frames,
                    controlled.signal,
                    controlled.format,
                    &controlled.control,
                    &controlled.publication,
                )
            };
            (
                job.job_id,
                ExportEvent {
                    revision: job.revision,
                    path: job.path,
                    result,
                },
            )
        }; // All renderer/runtime/job source handles have dropped before terminal.
        let terminal_delivered = finished.try_send(id).is_ok();
        if !terminal_delivered {
            let notification =
                "export finished but control custody notification failed; export worker stopped";
            event.result = Err(match event.result {
                Ok(report) => ExportFailure::notification_after_publication(report, notification),
                Err(mut error) => {
                    error.append_context(notification);
                    error
                }
            });
        }
        loop {
            let seq = drain_signal.snapshot();
            match events.try_send(event) {
                Ok(()) => {
                    wake_hook.wake();
                    break;
                }
                Err(TrySendError::Disconnected(undelivered)) => {
                    if let Some(journal) = &recovery {
                        journal.record_event(&undelivered);
                    }
                    return;
                }
                Err(TrySendError::Full(returned)) => {
                    if shutdown.load(Ordering::Acquire) {
                        if let Some(journal) = &recovery {
                            journal.record_event(&returned);
                        }
                        return;
                    }
                    event = returned;
                }
            }
            drain_signal.wait_since(seq, std::time::Duration::from_millis(5));
        }
        if !terminal_delivered {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_timeline_content_end_is_zero_for_empty_project() {
        let project = Project::new(48_000).expect("project");
        assert_eq!(latest_timeline_content_end(&project), 0);
    }

    #[test]
    fn latest_timeline_content_end_includes_frozen_track_tail() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddTrack {
                name: "frozen".into(),
            })
            .unwrap()
        else {
            panic!("track fixture");
        };
        editor
            .execute(hex_project::Command::FreezeTrack {
                track,
                path: PathBuf::from("frozen.wav"),
                start: 1_000,
                duration: 4_500,
                frames: 4_500,
            })
            .unwrap();
        assert_eq!(latest_timeline_content_end(editor.project()), 5_500);
    }
}
