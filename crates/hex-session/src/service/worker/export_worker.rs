//! Background export worker thread and export destination/extent guards.
use crate::service::*;

pub(crate) fn export_destination_is_original_media(
    project: &Project,
    destination: &Path,
    base: Option<&Path>,
) -> bool {
    let Ok(destination_metadata) = std::fs::metadata(destination) else {
        return false;
    };
    let destination_path = std::fs::canonicalize(destination).ok();
    project.assets().any(|asset| {
        let source = asset.path();
        let source = if source.is_absolute() {
            source.to_path_buf()
        } else {
            base.unwrap_or(Path::new(".")).join(source)
        };
        let Ok(source_metadata) = std::fs::metadata(&source) else {
            return false;
        };
        same_media_file(&source_metadata, &destination_metadata)
            || destination_path.as_ref().is_some_and(|destination| {
                std::fs::canonicalize(&source).is_ok_and(|source| &source == destination)
            })
    })
}

#[cfg(unix)]
fn same_media_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_media_file(_left: &std::fs::Metadata, _right: &std::fs::Metadata) -> bool {
    // The native VST hosting boundary currently supports macOS only.
    false
}

pub(crate) fn latest_timeline_content_end(project: &Project) -> u64 {
    let audio_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(|clip| clip.start().checked_add(clip.duration()))
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
    audio_end.max(midi_end).max(section_end).max(locator_end)
}

pub(crate) fn run_export_worker(
    work: Receiver<ExportWork>,
    events: SyncSender<ExportEvent>,
    exporter: Exporter,
    finished: SyncSender<u64>,
    shutdown: Arc<AtomicBool>,
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
                    match controlled.publication(&job.path) {
                        Ok(publication) => (streamed.render)(&StreamedExportRequest {
                            foreign: std::cell::RefCell::new(job.foreign.as_deref_mut().map(
                                |source| source as &mut dyn hex_export::ForeignBounceRenderer,
                            )),
                            source: &streamed.source,
                            prepared: &job.prepared,
                            resources: job.resources.as_ref(),
                            path: &job.path,
                            start: controlled.start,
                            frames: job.frames,
                            signal: &controlled.signal,
                            format: controlled.format,
                            control: &controlled.control,
                            publication: &publication,
                        }),
                        Err(error) => Err(error.into()),
                    }
                }
            } else if let Some(foreign) = job.foreign.as_deref_mut() {
                let controlled = job
                    .controlled
                    .take()
                    .unwrap_or_else(ControlledExport::whole_main);
                match controlled.publication(&job.path) {
                    Ok(publication)
                        if matches!(controlled.signal, hex_export::ExportSignal::Main) =>
                    {
                        hex_export::render_linear_main_with_foreign(
                            &job.path,
                            Arc::clone(&job.prepared),
                            job.resources.as_ref(),
                            controlled.start,
                            job.frames,
                            controlled.format,
                            &controlled.control,
                            &publication,
                            None,
                            foreign,
                        )
                        .map_err(ExportFailure::from)
                    }
                    Ok(_) => Err("private vst3 export requires main output".into()),
                    Err(error) => Err(error.into()),
                }
            } else if job.sources.is_some() && job.resources.is_some() {
                Err("export cannot combine private and ordinary source scopes".into())
            } else if let Some(resources) = job.resources.take() {
                if let Some(controlled) = job.controlled.take() {
                    controlled
                        .publication(&job.path)
                        .and_then(|policy| {
                            hex_export::render_prepared_with_device_resources_and_policy(
                                &job.path,
                                job.prepared,
                                &resources,
                                controlled.start,
                                job.frames,
                                controlled.signal,
                                controlled.format,
                                &controlled.control,
                                &policy,
                            )
                        })
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
                controlled
                    .publication(&job.path)
                    .and_then(|policy| {
                        hex_export::render_control_sampler_with_policy(
                            &job.path,
                            job.prepared,
                            sources,
                            controlled.start,
                            job.frames,
                            controlled.signal,
                            controlled.format,
                            &controlled.control,
                            &policy,
                        )
                    })
                    .map_err(ExportFailure::from)
            } else {
                // Every service request carries controlled parts; a bare job
                // renders as the automatic whole-project Main export.
                let controlled = job.controlled.unwrap_or_else(ControlledExport::whole_main);
                controlled
                    .publication(&job.path)
                    .map_err(ExportFailure::from)
                    .and_then(|policy| {
                        exporter(
                            &job.path,
                            &job.prepared,
                            controlled.start,
                            job.frames,
                            controlled.signal,
                            controlled.format,
                            &controlled.control,
                            &policy,
                        )
                    })
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
            match events.try_send(event) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(returned)) => {
                    if shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    event = returned;
                }
            }
            thread::sleep(std::time::Duration::from_millis(5));
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
}
