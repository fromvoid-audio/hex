//! One bounded export's real control custody. UI result polling never retires it.
use super::sampler_export::ServiceExportSnapshot;
use super::*;
use hex_engine::native_device_resources::{ControlSamplerSelection, DeviceResourcePhase};

enum DispatchState {
    Pending(Option<hex_engine::native_device_resources::DispatchedSourceObligation>),
    Dispatched(Option<hex_engine::native_device_resources::DispatchedSourceObligation>),
}
pub(super) struct ActiveExport {
    pub(super) job_id: u64,
    pub(super) snapshot: ServiceExportSnapshot,
    pub(super) runtime_bytes: usize,
    dispatch: DispatchState,
}
pub(super) struct ExportLane {
    work: Option<SyncSender<ExportWork>>,
    finished: Receiver<u64>,
    worker: Option<JoinHandle<()>>,
    pub(super) active: Option<ActiveExport>,
    next_id: u64,
    pub(super) native_hold: NativeSamplerReservationHold,
}
impl ExportLane {
    pub(super) fn new(
        work: SyncSender<ExportWork>,
        finished: Receiver<u64>,
        worker: JoinHandle<()>,
    ) -> Self {
        Self {
            work: Some(work),
            finished,
            worker: Some(worker),
            active: None,
            next_id: 1,
            native_hold: NativeSamplerReservationHold::default(),
        }
    }
    pub(super) fn poll(&mut self) {
        while let Ok(id) = self.finished.try_recv() {
            if self.active.as_ref().is_some_and(|a| {
                a.job_id == id && matches!(&a.dispatch, DispatchState::Dispatched(_))
            }) {
                self.active = None;
            }
        }
        // A missing message is never teardown proof. A finished thread can be
        // joined on this control worker; only that confirmation permits release.
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.work.take();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            if self
                .active
                .as_ref()
                .is_some_and(|a| matches!(&a.dispatch, DispatchState::Dispatched(_)))
            {
                self.active = None;
            }
        }
    }
    pub(super) fn admission<'a>(
        &'a self,
        current: Option<ControlSamplerSelection<'a>>,
    ) -> sampler_export::Admission<'a> {
        sampler_export::Admission {
            registered: &[],
            seeks: None,
            current,
            native: self.native_hold,
            export: self.active.as_ref().map(|a| a.snapshot.selection()),
            runtime_bytes: self.active.as_ref().map_or(0, |a| a.runtime_bytes),
            outbound: self.active.as_ref().and_then(|a| {
                match (&a.dispatch, a.snapshot.sources.as_ref()) {
                    (DispatchState::Dispatched(Some(obligation)), Some(anchor)) => Some(
                        hex_engine::native_device_resources::ControlSamplerOutbound {
                            anchor,
                            obligation,
                            job: a.job_id,
                        },
                    ),
                    _ => None,
                }
            }),
        }
    }
    pub(super) fn admission_with_seeks<'a>(
        &'a self,
        current: Option<ControlSamplerSelection<'a>>,
        seeks: Option<&'a super::resident_seek::Lane>,
    ) -> sampler_export::Admission<'a> {
        let admission = self.admission(current);
        sampler_export::Admission {
            seeks,
            runtime_bytes: admission.runtime_bytes.saturating_add(
                seeks
                    .map_or(Ok(0), super::resident_seek::Lane::allowance)
                    .unwrap_or(usize::MAX),
            ),
            ..admission
        }
    }
    pub(super) fn prepare_job(
        &mut self,
        job: &mut ExportJob,
        session: &Session,
        current_sources: Option<&hex_engine::native_device_resources::ControlSamplerSources>,
        resident: Option<(&resident_audio::Snapshot, &dyn WorkerAudioSink)>,
        seeks: Option<&super::resident_seek::Lane>,
    ) -> Result<(), String> {
        self.poll();
        if self.worker.is_none() {
            return Err("export worker is unavailable".into());
        }
        if self.active.is_some() {
            return Err("previous export resource custody is still active".into());
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("export job identity exhausted")?;
        job.job_id = id;
        if session.native_reservation.is_none() {
            if job.prepared.requires_control_sampler_sources() {
                return Err("sampler requires an explicit resident reservation".into());
            }
            return Ok(());
        }
        let native_views = resident.map(|(_, sink)| sink.resident_admission());
        let sets = resident.map(|(snapshot, _)| [&snapshot.resources]);
        let revisions = resident.map(|(snapshot, _)| [snapshot.prepared.as_ref()]);
        let mut registered = Vec::new();
        if let (Some(sets), Some(revisions), Some(views)) = (&sets, &revisions, &native_views) {
            registered.push(
                hex_engine::native_device_resources::DeviceResourceAdmission::new(sets, &[])
                    .with_prepared_revisions(revisions),
            );
            registered.extend_from_slice(views);
        }
        if let Some(seeks) = seeks {
            registered.extend(seeks.views());
        }
        let seek_allowance = seeks.map_or(Ok(0), super::resident_seek::Lane::allowance)?;
        let native_hold = resident.map_or(self.native_hold, |(_, sink)| sink.native_sampler_hold());
        // Offline export retains its existing private source-borrowing renderer. It compiles a
        // separate matching snapshot; an ordinary device-resource revision is never handed to
        // the old private constructor. Current original banks and all actual native owners share
        // this same admission ledger throughout compilation and dispatch.
        let current = if resident.is_some() {
            session
                .prepare_service_snapshot(
                    session.project(),
                    sampler_export::Admission {
                        registered: &registered,
                        current: Some(ControlSamplerSelection {
                            sources: current_sources,
                            prepared: Some(&job.prepared),
                        }),
                        native: native_hold,
                        runtime_bytes: seek_allowance,
                        ..sampler_export::Admission::default()
                    },
                )
                .map_err(|error| error.to_string())?
        } else {
            ServiceExportSnapshot {
                prepared: Arc::clone(&job.prepared),
                sources: current_sources
                    .map(|s| s.capture_on_control())
                    .transpose()
                    .map_err(|e| format!("sampler export capture refused: {e:?}").to_lowercase())?,
            }
        };
        job.prepared = Arc::clone(&current.prepared);
        let runtime_bytes = match current.sources.as_ref() {
            Some(sources) => hex_engine::SessionPlayback::control_sampler_export_runtime_allowance(
                &job.prepared,
                sources,
            ),
            None => hex_engine::SessionPlayback::linear_export_runtime_allowance(&job.prepared),
        }
        .map_err(|e| e.to_string())?;
        let (job_sources, obligation) = match current.sources.as_ref() {
            Some(source) => {
                let (owner, obligation) = source
                    .capture_for_dispatch_on_control(id)
                    .map_err(|e| format!("sampler export capture refused: {e:?}").to_lowercase())?;
                (Some(owner), Some(obligation))
            }
            None => (None, None),
        };
        let runtime_bytes = runtime_bytes
            .checked_add(std::mem::size_of::<DispatchState>())
            .ok_or("export runtime and owner metadata overflow")?;
        let selections = [
            ControlSamplerSelection {
                sources: current_sources,
                prepared: Some(resident.map_or(job.prepared.as_ref(), |(snapshot, _)| {
                    snapshot.prepared.as_ref()
                })),
            },
            current.selection(),
            ControlSamplerSelection {
                sources: job_sources.as_ref(),
                prepared: Some(&job.prepared),
            },
        ];
        let stores = [&session.native_assets];
        let legacy = session
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .collect::<Vec<_>>();
        let phase = DeviceResourcePhase {
            registered: &registered,
            stores: &stores,
            selections: &selections,
            outbound: &[],
            legacy: &legacy,
            checked_runtime_bytes: runtime_bytes
                .checked_add(seek_allowance)
                .ok_or("export seek allowance overflow")?
                .checked_add(native_hold.construction_bytes)
                .ok_or("native export reservation overflow")?,
            construction_bytes: 0,
        };
        let reservation = session
            .native_reservation
            .ok_or("native reservation unavailable")?;
        let inventory = hex_engine::native_device_resources::inspect_device_resource_union(
            &phase,
            &phase,
            reservation.storage,
        )
        .map_err(|e| format!("export storage refused: {e:?}").to_lowercase())?;
        if inventory
            .retained_frames
            .checked_add(native_hold.frames)
            .is_none_or(|n| n > session.max_resident_frames)
        {
            return Err("resident audio frame limit reached".into());
        }
        job.sources = job_sources;
        self.active = Some(ActiveExport {
            job_id: id,
            snapshot: current,
            runtime_bytes,
            dispatch: DispatchState::Pending(obligation),
        });
        Ok(())
    }
    pub(super) fn submit(&mut self, job: ExportJob) -> Result<(), ExportEvent> {
        let job_id = job.job_id;
        let result = match self.work.as_ref() {
            Some(work) => work.try_send(ExportWork::Render(job)),
            None => Err(TrySendError::Disconnected(ExportWork::Render(job))),
        };
        match result {
            Ok(()) => {
                if let Some(active) = self.active.as_mut().filter(|a| a.job_id == job_id) {
                    let previous =
                        std::mem::replace(&mut active.dispatch, DispatchState::Pending(None));
                    active.dispatch = match previous {
                        DispatchState::Pending(token) => DispatchState::Dispatched(token),
                        other => other,
                    };
                }
                Ok(())
            }
            Err(
                TrySendError::Full(ExportWork::Render(job))
                | TrySendError::Disconnected(ExportWork::Render(job)),
            ) => {
                let id = job.job_id;
                let event = ExportEvent {
                    revision: job.revision,
                    path: job.path.clone(),
                    result: Err("export worker is unavailable".into()),
                };
                drop(job); // This job never transferred. Retire its source handle first.
                if self.active.as_ref().is_some_and(|a| a.job_id == id) {
                    self.active = None;
                }
                Err(event)
            }
        }
    }
}
impl Drop for ExportLane {
    fn drop(&mut self) {
        // Runs only on the Session control worker. Closing input must precede
        // joining; the anchor survives every missing/disconnected completion.
        self.work.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::sampler_export::tests::{Files, direct_sampler, job};
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stale_and_pending_terminals_cannot_release_job_and_missing_terminal_requires_join() {
        let files = Files::new(false);
        let (session, snapshot) = direct_sampler(&files);
        let (work_tx, work_rx) = mpsc::sync_channel(1);
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let injected_terminal = finished_tx.clone();
        let (events_tx, events_rx) = mpsc::sync_channel(1);
        let hook: ExportTestHook = Arc::new(std::sync::Mutex::new(None));
        let worker_hook = Arc::clone(&hook);
        let worker = thread::spawn(move || {
            run_export_worker(
                work_rx,
                events_tx,
                Arc::new(|_, _, _, _| Err("unexpected asset-free exporter".into())),
                finished_tx,
                Arc::new(AtomicBool::new(false)),
                worker_hook,
            )
        });
        let mut lane = ExportLane::new(work_tx, finished_rx, worker);
        let mut previous_id = 0;
        for generation in 0..3 {
            let (entered_tx, entered) = mpsc::sync_channel(1);
            let (release, wait) = mpsc::sync_channel(1);
            *hook.lock().unwrap() = Some(Box::new(move || {
                entered_tx.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }));
            let path = std::env::temp_dir().join(format!(
                "hex-token-job-{}-{generation}.wav",
                std::process::id()
            ));
            let mut next = job(&snapshot, path.clone());
            lane.prepare_job(&mut next, &session, snapshot.sources.as_ref(), None, None)
                .unwrap();
            let id = next.job_id;
            injected_terminal.send(id).unwrap();
            lane.poll();
            assert!(
                lane.active.is_some(),
                "a pending actual job has not transferred or dropped"
            );
            assert!(lane.submit(next).is_ok());
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            injected_terminal.send(previous_id).unwrap();
            if generation < 2 {
                lane.poll();
                assert_eq!(
                    lane.active.as_ref().unwrap().job_id,
                    id,
                    "stale terminal cannot release newer custody"
                );
            }
            release.send(()).unwrap();
            let result = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if generation < 2 {
                assert!(result.result.is_ok());
                // The terminal may be delayed after the actual source job has
                // dropped. No count observation discharges the outbound field.
                assert!(lane.active.is_some());
                lane.admission(Some(snapshot.selection()))
                    .check(&session, &session, snapshot.selection(), 0, None)
                    .unwrap();
                lane.poll();
                assert!(lane.active.is_none());
            } else {
                assert!(result.result.unwrap_err().contains("notification failed"));
                assert!(lane.active.is_some());
                let end = Instant::now() + Duration::from_secs(5);
                while !lane.worker.as_ref().unwrap().is_finished() && Instant::now() < end {
                    thread::sleep(Duration::from_millis(1));
                }
                lane.poll();
                assert!(
                    lane.worker.is_none(),
                    "only actual joined teardown can discharge a missing terminal"
                );
                assert!(lane.active.is_none());
            }
            previous_id = id;
            std::fs::remove_file(path).unwrap();
        }
    }
}
