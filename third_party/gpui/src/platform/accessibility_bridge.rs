//! Data-only revocable native accessibility transport. No platform/application references.
use std::{
    collections::{HashMap, VecDeque},
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub(super) const MAILBOX_CAPACITY: usize = 64;
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct WindowKey {
    pub window: u64,
    pub epoch: u64,
}

struct State<S, R> {
    snapshot: Option<Arc<S>>,
    requests: VecDeque<R>,
    scheduled: bool,
    revoked: bool,
}

/// The only shared native object graph: immutable owned data and a bounded mailbox.
pub(super) struct Bridge<S, R> {
    pub key: WindowKey,
    state: Mutex<State<S, R>>,
}
impl<S, R> Bridge<S, R> {
    fn new(key: WindowKey) -> Self {
        Self {
            key,
            state: Mutex::new(State {
                snapshot: None,
                requests: VecDeque::with_capacity(MAILBOX_CAPACITY),
                scheduled: false,
                revoked: false,
            }),
        }
    }
    pub fn snapshot(&self) -> Option<Arc<S>> {
        let state = self.state.lock().ok()?;
        if state.revoked {
            None
        } else {
            state.snapshot.clone()
        }
    }
    /// Returns the prior snapshot for destruction outside the lock.
    pub fn publish(&self, snapshot: S) -> Option<Arc<S>> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        if state.revoked {
            return None;
        }
        state.snapshot.replace(Arc::new(snapshot))
    }
    /// Some(true): accepted and needs one ID-only wake. Queue acceptance is not edit acceptance.
    pub fn enqueue(&self, request: R, permits: impl FnOnce(&S, &R) -> bool) -> Option<bool> {
        let mut state = self.state.lock().ok()?;
        if state.revoked
            || state.requests.len() >= MAILBOX_CAPACITY
            || !state
                .snapshot
                .as_ref()
                .is_some_and(|s| permits(s, &request))
        {
            return None;
        }
        state.requests.push_back(request);
        let wake = !state.scheduled;
        state.scheduled = true;
        Some(wake)
    }
    /// One bounded dispatch; concurrent arrivals schedule a separate wake.
    pub fn drain(&self) -> VecDeque<R> {
        let Ok(mut state) = self.state.lock() else {
            return VecDeque::new();
        };
        state.scheduled = false;
        std::mem::replace(
            &mut state.requests,
            VecDeque::with_capacity(MAILBOX_CAPACITY),
        )
    }
    pub fn is_revoked(&self) -> bool {
        self.state.lock().map_or(true, |state| state.revoked)
    }
    pub fn revoke(&self) -> bool {
        let retired = {
            let Ok(mut state) = self.state.lock() else {
                return false;
            };
            if state.revoked {
                return false;
            }
            state.revoked = true;
            state.scheduled = false;
            (state.snapshot.take(), std::mem::take(&mut state.requests))
        };
        drop(retired); // Never destroy data while holding the snapshot lock.
        true
    }
}

/// Explicitly UI-local registry. No closed-window tombstones, and no reusable epoch.
pub(super) struct Registry<T> {
    entries: HashMap<WindowKey, T>,
    _ui_only: PhantomData<Rc<()>>,
}
impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            _ui_only: PhantomData,
        }
    }
}
impl<T> Registry<T> {
    pub fn register<S, R>(
        &mut self,
        window: u64,
        make: impl FnOnce(Arc<Bridge<S, R>>) -> T,
    ) -> Option<Arc<Bridge<S, R>>> {
        let epoch = NEXT_EPOCH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
                epoch.checked_add(1)
            })
            .ok()?;
        let bridge = Arc::new(Bridge::new(WindowKey { window, epoch }));
        self.entries.insert(bridge.key, make(bridge.clone()));
        Some(bridge)
    }
    /// Restore an entry temporarily extracted by UI code; caller must recheck revocation first.
    pub fn restore(&mut self, key: WindowKey, value: T) -> Option<T> {
        self.entries.insert(key, value)
    }
    pub fn get(&self, key: WindowKey) -> Option<&T> {
        self.entries.get(&key)
    }
    pub fn get_mut(&mut self, key: WindowKey) -> Option<&mut T> {
        self.entries.get_mut(&key)
    }
    pub fn remove(&mut self, key: WindowKey) -> Option<T> {
        self.entries.remove(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registered() -> (Registry<Arc<Bridge<u64, u64>>>, Arc<Bridge<u64, u64>>) {
        let mut registry = Registry::default();
        let bridge = registry.register(7, |bridge| bridge).unwrap();
        (registry, bridge)
    }
    #[test]
    fn immutable_snapshot_survives_background_query_but_revocation_refuses_future_queries() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Bridge<u64, u64>>();
        let (mut registry, bridge) = registered();
        bridge.publish(3);
        let reader = bridge.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other = barrier.clone();
        let thread = std::thread::spawn(move || {
            let snapshot = reader.snapshot().unwrap();
            other.wait();
            other.wait();
            assert_eq!(*snapshot, 3);
            assert!(reader.snapshot().is_none());
            assert_eq!(reader.enqueue(3, |_, _| true), None);
        });
        barrier.wait();
        bridge.revoke();
        drop(registry.remove(bridge.key));
        barrier.wait();
        thread.join().unwrap();
        assert!(registry.get(bridge.key).is_none());
    }
    #[test]
    fn mailbox_bounds_wakes_and_revalidates_current_revision_without_truncation() {
        let (_, bridge) = registered();
        bridge.publish(9);
        assert_eq!(bridge.enqueue(8, |s, r| s == r), None);
        for index in 0..MAILBOX_CAPACITY {
            assert_eq!(bridge.enqueue(9, |s, r| s == r), Some(index == 0));
        }
        assert_eq!(bridge.enqueue(9, |_, _| true), None);
        assert_eq!(bridge.drain().len(), MAILBOX_CAPACITY);
        bridge.publish(10);
        assert_eq!(bridge.enqueue(9, |s, r| s == r), None);
        assert_eq!(bridge.enqueue(10, |s, r| s == r), Some(true));
        bridge.revoke();
        assert!(bridge.drain().is_empty());
        bridge.publish(11);
        assert!(bridge.snapshot().is_none());
    }
    #[test]
    fn reused_window_slot_has_new_epoch_and_nested_close_cannot_restore_callback() {
        let (mut registry, bridge) = registered();
        // Simulate temporarily extracting a UI callback before calling application code.
        let callback = registry.get_mut(bridge.key).unwrap().clone();
        bridge.revoke();
        drop(registry.remove(bridge.key));
        let reopened = registry
            .register(7, |bridge: Arc<Bridge<u64, u64>>| bridge)
            .unwrap();
        assert_ne!(bridge.key, reopened.key);
        assert!(registry.get_mut(callback.key).is_none());
        assert!(registry.get(reopened.key).is_some());
        let entry = registry.remove(reopened.key).unwrap();
        assert!(registry.restore(reopened.key, entry).is_none());
    }
    #[test]
    fn ui_registry_owner_drops_on_ui_while_last_data_reader_drops_on_worker() {
        struct UiOwner {
            thread: std::thread::ThreadId,
            dropped: Rc<std::cell::Cell<bool>>,
        }
        impl Drop for UiOwner {
            fn drop(&mut self) {
                assert_eq!(self.thread, std::thread::current().id());
                self.dropped.set(true);
            }
        }
        let dropped = Rc::new(std::cell::Cell::new(false));
        let mut registry = Registry::default();
        let bridge = registry
            .register(19, |bridge: Arc<Bridge<u64, u64>>| {
                (
                    bridge,
                    UiOwner {
                        thread: std::thread::current().id(),
                        dropped: dropped.clone(),
                    },
                )
            })
            .unwrap();
        bridge.publish(5);
        let reader = bridge.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            let snapshot = reader.snapshot().unwrap();
            worker_barrier.wait();
            worker_barrier.wait();
            assert_eq!(*snapshot, 5);
            assert!(reader.is_revoked());
            // These are the last data-only owners; no UI owner is reachable from them.
            drop(snapshot);
            drop(reader);
        });
        barrier.wait();
        bridge.revoke();
        drop(registry.remove(bridge.key));
        drop(bridge);
        assert!(dropped.get());
        barrier.wait();
        worker.join().unwrap();
        assert!(registry.entries.is_empty());
    }
    #[test]
    fn retired_snapshot_drop_can_reenter_transport_outside_lock() {
        struct Reenter(std::sync::Weak<Bridge<Reenter, ()>>);
        impl Drop for Reenter {
            fn drop(&mut self) {
                if let Some(bridge) = self.0.upgrade() {
                    assert!(bridge.state.try_lock().is_ok());
                }
            }
        }
        let mut registry = Registry::default();
        let bridge = registry
            .register(8, |_: Arc<Bridge<Reenter, ()>>| ())
            .unwrap();
        bridge.publish(Reenter(Arc::downgrade(&bridge)));
        drop(bridge.publish(Reenter(Arc::downgrade(&bridge))));
        assert!(bridge.revoke());
        assert!(bridge.is_revoked());
        assert!(!bridge.revoke());
    }
}
