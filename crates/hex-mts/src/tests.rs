use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Default)]
struct Probe {
    blocked: AtomicBool,
    ipc: AtomicBool,
    bad_clients: AtomicBool,
    acquisitions: AtomicUsize,
    recoveries: AtomicUsize,
    releases: AtomicUsize,
    publications: Mutex<Vec<TuningPublication>>,
}
struct Mock {
    probe: Arc<Probe>,
    owned: bool,
}
impl Transport for Mock {
    fn acquire(&mut self, recover: bool) -> Result<(), NativeError> {
        self.probe.acquisitions.fetch_add(1, Ordering::SeqCst);
        if recover {
            assert!(self.probe.ipc.load(Ordering::SeqCst));
            self.probe.recoveries.fetch_add(1, Ordering::SeqCst);
            self.probe.blocked.store(false, Ordering::SeqCst);
        }
        if self.probe.blocked.load(Ordering::SeqCst) {
            return Err(if self.probe.ipc.load(Ordering::SeqCst) {
                NativeError::ExistingMasterWithIpc
            } else {
                NativeError::ExistingMaster
            });
        }
        assert!(!self.owned);
        self.owned = true;
        Ok(())
    }
    fn release(&mut self) {
        if self.owned {
            self.probe.releases.fetch_add(1, Ordering::SeqCst);
            self.owned = false;
        }
    }
    fn publish(&mut self, publication: &TuningPublication) {
        assert!(self.owned);
        self.probe.publications.lock().unwrap().push(*publication);
    }
    fn clients(&self) -> Result<u32, NativeError> {
        if self.probe.bad_clients.load(Ordering::SeqCst) {
            Err(NativeError::InvalidClientCount(-1))
        } else {
            Ok(3)
        }
    }
}
fn fixture() -> (Worker, Handle, Arc<Probe>, Owner) {
    let probe = Arc::new(Probe::default());
    let factory_probe = Arc::clone(&probe);
    let worker = Worker::start_with(move || Mock {
        probe: factory_probe,
        owned: false,
    })
    .unwrap();
    let handle = worker.handle();
    let owner = Owner {
        client: handle.new_client().unwrap(),
        generation: 1,
    };
    (worker, handle, probe, owner)
}
fn publication(owner: Owner, serial: u64) -> Publication {
    Publication::new(
        Revision {
            owner,
            serial,
            project_revision: serial,
        },
        ProjectTuning::default(),
    )
    .unwrap()
}
fn wait(handle: &Handle, predicate: impl Fn(Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        let snapshot = handle.snapshot().unwrap();
        if predicate(snapshot) {
            return snapshot;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn explicit_owner_latest_revision_and_reserved_shutdown() {
    let (mut worker, handle, probe, owner) = fixture();
    assert_eq!(
        handle.publish(publication(owner, 1)),
        Err(ControlError::Stale)
    );
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
    handle.enable(publication(owner, 1)).unwrap();
    for serial in 2..100 {
        handle.publish(publication(owner, serial)).unwrap();
    }
    let snapshot = wait(&handle, |s| {
        s.state == State::Active && s.published == Some(publication(owner, 99).revision)
    });
    assert_eq!(snapshot.clients, 3);
    assert_eq!(snapshot.desired, snapshot.published);
    assert_eq!(
        handle.publish(publication(owner, 98)),
        Err(ControlError::Stale)
    );
    let foreign = Owner {
        client: handle.new_client().unwrap(),
        generation: 1,
    };
    assert_eq!(
        handle.enable(publication(foreign, 1)),
        Err(ControlError::Busy)
    );
    assert_eq!(handle.disable(foreign), Err(ControlError::Stale));
    worker.shutdown();
    assert_eq!(handle.snapshot().unwrap().state, State::Stopped);
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        handle.enable(publication(owner, 100)),
        Err(ControlError::Unavailable)
    );
    worker.shutdown();
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
}

#[test]
fn existing_master_is_never_taken_over_and_updates_do_not_retry() {
    let (mut worker, handle, probe, owner) = fixture();
    probe.blocked.store(true, Ordering::SeqCst);
    handle.enable(publication(owner, 1)).unwrap();
    wait(&handle, |s| {
        s.state
            == State::Blocked {
                recovery_available: false,
            }
    });
    assert_eq!(handle.recover(owner), Err(ControlError::Stale));
    handle.publish(publication(owner, 2)).unwrap();
    // Synchronize against the worker's poll without requesting reacquisition.
    thread::sleep(Duration::from_millis(150));
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
    assert!(probe.publications.lock().unwrap().is_empty());
    assert_eq!(probe.releases.load(Ordering::SeqCst), 0);
    probe.blocked.store(false, Ordering::SeqCst);
    handle.retry(owner).unwrap();
    wait(&handle, |s| {
        s.state == State::Active && s.published == Some(publication(owner, 2).revision)
    });
    worker.shutdown();
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
}

#[test]
fn ipc_recovery_requires_current_blocked_owner_and_publishes_latest_tuning() {
    let (mut worker, handle, probe, owner) = fixture();
    probe.blocked.store(true, Ordering::SeqCst);
    probe.ipc.store(true, Ordering::SeqCst);
    handle.enable(publication(owner, 1)).unwrap();
    let is_blocked = |s: Snapshot| {
        s.state
            == State::Blocked {
                recovery_available: true,
            }
    };
    wait(&handle, is_blocked);

    let foreign = Owner {
        client: handle.new_client().unwrap(),
        ..owner
    };
    assert_eq!(handle.recover(foreign), Err(ControlError::Stale));
    handle.retry(owner).unwrap();
    wait(&handle, is_blocked);
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 2);
    assert_eq!(probe.recoveries.load(Ordering::SeqCst), 0);
    assert!(probe.publications.lock().unwrap().is_empty());

    let latest = publication(owner, 2);
    handle.publish(latest).unwrap();
    handle.recover(owner).unwrap();
    assert_eq!(handle.recover(owner), Err(ControlError::Stale));
    wait(&handle, |s| {
        s.state == State::Active && s.published == Some(latest.revision)
    });
    worker.shutdown();
    assert_eq!(probe.recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.publications.lock().unwrap().as_slice(),
        &[latest.payload]
    );
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
}

#[test]
fn disable_reenable_same_identity_still_releases_original_registration() {
    let (mut worker, handle, probe, owner) = fixture();
    handle.enable(publication(owner, 1)).unwrap();
    wait(&handle, |s| s.state == State::Active);
    handle.disable(owner).unwrap();
    handle.enable(publication(owner, 2)).unwrap();
    wait(&handle, |s| {
        s.state == State::Active && s.published == Some(publication(owner, 2).revision)
    });
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 2);
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
    handle.disable(owner).unwrap();
    wait(&handle, |s| s.state == State::Disabled);
    assert_eq!(probe.releases.load(Ordering::SeqCst), 2);
    worker.shutdown();
    assert_eq!(probe.releases.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_client_count_retires_only_owned_master_and_requires_retry() {
    let (mut worker, handle, probe, owner) = fixture();
    probe.bad_clients.store(true, Ordering::SeqCst);
    handle.enable(publication(owner, 1)).unwrap();
    wait(&handle, |s| {
        s.state == State::Unavailable(NativeError::InvalidClientCount(-1))
    });
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
    probe.bad_clients.store(false, Ordering::SeqCst);
    handle.retry(owner).unwrap();
    wait(&handle, |s| s.state == State::Active);
    worker.shutdown();
    assert_eq!(probe.releases.load(Ordering::SeqCst), 2);
}

#[test]
fn project_switch_during_native_acquisition_cannot_publish_old_receipt() {
    use std::sync::mpsc;
    struct Gated {
        mock: Mock,
        entered: mpsc::SyncSender<()>,
        resume: Option<mpsc::Receiver<()>>,
    }
    impl Transport for Gated {
        fn acquire(&mut self, recover: bool) -> Result<(), NativeError> {
            if let Some(resume) = self.resume.take() {
                self.entered.send(()).unwrap();
                resume.recv().unwrap();
            }
            self.mock.acquire(recover)
        }
        fn release(&mut self) {
            self.mock.release();
        }
        fn publish(&mut self, publication: &TuningPublication) {
            self.mock.publish(publication);
        }
        fn clients(&self) -> Result<u32, NativeError> {
            self.mock.clients()
        }
    }
    let probe = Arc::new(Probe::default());
    let factory_probe = Arc::clone(&probe);
    let (entered, receiving) = mpsc::sync_channel(1);
    let (resume, gate) = mpsc::sync_channel(1);
    let mut worker = Worker::start_with(move || Gated {
        mock: Mock {
            probe: factory_probe,
            owned: false,
        },
        entered,
        resume: Some(gate),
    })
    .unwrap();
    let handle = worker.handle();
    let old = Owner {
        client: handle.new_client().unwrap(),
        generation: 1,
    };
    handle.enable(publication(old, 1)).unwrap();
    receiving.recv_timeout(Duration::from_secs(3)).unwrap();
    handle.disable(old).unwrap();
    let new = Owner {
        generation: 2,
        ..old
    };
    handle.enable(publication(new, 1)).unwrap();
    assert_eq!(
        handle.publish(publication(old, 2)),
        Err(ControlError::Stale)
    );
    resume.send(()).unwrap();
    let snapshot = wait(&handle, |snapshot| snapshot.state == State::Active);
    assert_eq!(snapshot.published.unwrap().owner, new);
    assert_eq!(
        probe.publications.lock().unwrap().len(),
        1,
        "old acquisition must never publish"
    );
    assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 2);
    assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
    worker.shutdown();
    assert_eq!(probe.releases.load(Ordering::SeqCst), 2);
}
