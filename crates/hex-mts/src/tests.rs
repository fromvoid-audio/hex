use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Default)]
struct Probe {
    blocked: AtomicBool,
    bad_clients: AtomicBool,
    acquisitions: AtomicUsize,
    releases: AtomicUsize,
    publications: Mutex<Vec<TuningPublication>>,
}
struct Mock {
    probe: Arc<Probe>,
    owned: bool,
}
impl Transport for Mock {
    fn acquire(&mut self) -> Result<(), NativeError> {
        self.probe.acquisitions.fetch_add(1, Ordering::SeqCst);
        if self.probe.blocked.load(Ordering::SeqCst) {
            return Err(NativeError::ExistingMaster);
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
    wait(&handle, |s| s.state == State::Blocked);
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
