//! Control-side observation of the default output route (#4131).
//!
//! CPAL 0.17.3 exposes no public CoreAudio listener for the default output
//! device or its nominal rate. Its property-listener type is private and only
//! watches `DeviceIsAlive`, reporting a disconnected stream to the error
//! callback as `StreamError::DeviceNotAvailable`. Hex therefore samples the
//! default route on one dedicated control thread at a bounded cadence: each
//! sample is one default-device lookup, one device UID read and one default
//! config read through CPAL's safe API. The pinned CPAL dependency reads the
//! macOS buffer range directly from the device, without constructing an Audio
//! Unit for each probe; see `third_party/cpal/HEX-PATCHES.md`. Hex adds no
//! workspace unsafe code or competing audio-device owner.
//!
//! A different route is published only after two consecutive equal samples,
//! so a transient default while macOS switches devices is not followed. A
//! sample without a default device is ignored. The thread never touches an
//! output stream, callback or lifecycle lease. A consumer compares
//! [`OutputRouteWatch::latest_if_newer`] with the route an output opened on
//! ([`crate::WorkerPreparedOutput::route`]) and requests the existing bounded
//! recovery open.
//!
//! The watch thread shares only this state. Dropping the watch requests stop
//! and wakes the thread, which exits after at most one in-flight probe. The
//! thread is not joined, so a blocked device query can never stall the owner.
use cpal::traits::{DeviceTrait, HostTrait};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{Acquire, Release},
        },
    },
    thread::Thread,
    time::Duration,
};

/// Interval between default-route samples. A change is followed after two
/// equal samples, so between one and two intervals after it settles.
pub const ROUTE_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// The default output device and its nominal rate, as CPAL reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    /// Stable device identifier (the CoreAudio device UID on macOS).
    pub device: String,
    /// The device's current nominal sample rate in hertz.
    pub nominal_hz: u32,
}

/// Current default output route, or `None` when no default device answers.
/// Blocking device queries: call only on a control thread.
pub(crate) fn probe_default_route() -> Option<OutputRoute> {
    let device = cpal::default_host().default_output_device()?;
    route_of(&device)
}

/// Route of an already selected device, read on the open lane.
pub(crate) fn route_of(device: &cpal::Device) -> Option<OutputRoute> {
    let id = device.id().ok()?;
    let config = device.default_output_config().ok()?;
    Some(OutputRoute {
        device: id.to_string(),
        nominal_hz: config.sample_rate(),
    })
}

/// Two-sample confirmation of a route change.
#[derive(Debug, Default)]
struct Debounce {
    published: Option<OutputRoute>,
    candidate: Option<OutputRoute>,
}
impl Debounce {
    /// Returns the route to publish when `sample` confirms a new route.
    fn observe(&mut self, sample: Option<OutputRoute>) -> Option<OutputRoute> {
        let Some(sample) = sample else {
            self.candidate = None;
            return None;
        };
        if self.published.as_ref() == Some(&sample) {
            self.candidate = None;
            return None;
        }
        if self.candidate.as_ref() != Some(&sample) {
            self.candidate = Some(sample);
            return None;
        }
        self.candidate = None;
        self.published = Some(sample.clone());
        Some(sample)
    }
}

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<OutputRoute>>,
    /// Incremented after each publication; 0 means nothing published yet.
    generation: AtomicU64,
    stop: AtomicBool,
}

/// Owner of the default-route sampling thread.
pub struct OutputRouteWatch {
    shared: Arc<Shared>,
    thread: Thread,
}
impl OutputRouteWatch {
    /// Starts sampling the actual default output route.
    pub fn spawn() -> Result<Self, String> {
        Self::spawn_with(ROUTE_SAMPLE_INTERVAL, probe_default_route)
    }

    pub(crate) fn spawn_with(
        interval: Duration,
        mut probe: impl FnMut() -> Option<OutputRoute> + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let watched = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("hex-output-route".into())
            .spawn(move || {
                let mut debounce = Debounce::default();
                while !watched.stop.load(Acquire) {
                    if let Some(route) = debounce.observe(probe()) {
                        *watched
                            .latest
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(route);
                        watched.generation.fetch_add(1, Release);
                    }
                    std::thread::park_timeout(interval);
                }
            })
            .map_err(|error| format!("could not start output route watch: {error}"))?;
        Ok(Self {
            shared,
            thread: handle.thread().clone(),
        })
    }

    /// The confirmed route when it was published after `seen`, which is
    /// advanced. Reads nothing (and allocates nothing) when unchanged.
    pub fn latest_if_newer(&self, seen: &mut u64) -> Option<OutputRoute> {
        let generation = self.shared.generation.load(Acquire);
        if generation == *seen {
            return None;
        }
        *seen = generation;
        self.shared
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
impl Drop for OutputRouteWatch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Release);
        self.thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn route(device: &str, nominal_hz: u32) -> Option<OutputRoute> {
        Some(OutputRoute {
            device: device.into(),
            nominal_hz,
        })
    }

    #[test]
    fn a_route_is_published_only_after_two_equal_samples() {
        let mut debounce = Debounce::default();
        assert_eq!(debounce.observe(route("speakers", 48_000)), None);
        assert_eq!(
            debounce.observe(route("speakers", 48_000)),
            route("speakers", 48_000)
        );
        assert_eq!(debounce.observe(route("speakers", 48_000)), None);
        // A transient default during a switch is never published.
        assert_eq!(debounce.observe(route("transient", 48_000)), None);
        assert_eq!(debounce.observe(route("headphones", 48_000)), None);
        assert_eq!(
            debounce.observe(route("headphones", 48_000)),
            route("headphones", 48_000)
        );
        // A missing default device neither publishes nor confirms a candidate.
        assert_eq!(debounce.observe(route("speakers", 48_000)), None);
        assert_eq!(debounce.observe(None), None);
        assert_eq!(debounce.observe(route("speakers", 48_000)), None);
        // A nominal-rate change on the same device is a new route.
        assert_eq!(debounce.observe(route("headphones", 44_100)), None);
        assert_eq!(
            debounce.observe(route("headphones", 44_100)),
            route("headphones", 44_100)
        );
    }

    #[test]
    fn the_watch_publishes_confirmed_routes_and_its_thread_exits_on_drop() {
        let (samples, probe_samples) = mpsc::channel();
        let (probed, probe_calls) = mpsc::channel::<()>();
        let mut current = None;
        let watch = OutputRouteWatch::spawn_with(Duration::from_millis(1), move || {
            let _ = probed.send(());
            if let Ok(next) = probe_samples.try_recv() {
                current = next;
            }
            current.clone()
        })
        .unwrap();
        let mut seen = 0;
        assert_eq!(watch.latest_if_newer(&mut seen), None);
        samples.send(route("speakers", 48_000)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let published = loop {
            if let Some(route) = watch.latest_if_newer(&mut seen) {
                break route;
            }
            assert!(Instant::now() < deadline, "route was never published");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(Some(published), route("speakers", 48_000));
        assert_eq!(watch.latest_if_newer(&mut seen), None);
        drop(watch);
        // The probe (and its sender) is dropped only when the thread exits.
        loop {
            match probe_calls.recv_timeout(Duration::from_secs(5)) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("watch thread did not exit"),
            }
        }
    }
}
