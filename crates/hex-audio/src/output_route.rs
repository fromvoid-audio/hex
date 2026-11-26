//! Control-side observation of the default output route (#4131, #13524).
//!
//! On macOS, Hex subscribes on control to CoreAudio HAL property notifications
//! (`kAudioHardwarePropertyDefaultOutputDevice` on the system object plus
//! `kAudioDevicePropertyNominalSampleRate` and `kAudioDevicePropertyDeviceIsAlive`
//! on the active default device) through the pinned CPAL dependency's safe
//! `CoreAudioOutputRouteListener`; see `third_party/cpal/HEX-PATCHES.md`. The
//! dedicated control thread blocks at zero polling cost until a HAL
//! notification arrives (with a conservative long fallback interval), probes
//! the current route, and confirms changes across a brief settle delay so a
//! transient default while macOS switches devices is not followed. Hex adds no
//! workspace unsafe code or competing audio-device owner.
//!
//! A sample without a default device is ignored. The thread never touches an
//! output stream, callback or lifecycle lease. A consumer compares
//! [`OutputRouteWatch::latest_if_newer`] with the route an output opened on
//! ([`crate::WorkerPreparedOutput::route`]) and requests the existing bounded
//! recovery open.
//!
//! The watch thread shares only this state. Dropping the watch requests stop,
//! wakes the HAL waiter and thread, and joins it within a short bounded wait so
//! normal teardown leaves no detached thread while a blocked OS device query
//! can never stall the owner indefinitely.
use cpal::traits::{DeviceTrait, HostTrait};
use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{Acquire, Release},
        },
    },
    thread::JoinHandle,
    time::Duration,
};

/// Short confirmation delay after a route change is observed so transient
/// intermediate defaults during OS device switches are not published.
pub const ROUTE_SETTLE_DELAY: Duration = Duration::from_millis(25);

/// Idle safety fallback interval between CoreAudio HAL notifications. Normal
/// default-output and nominal-rate changes wake immediately via HAL listeners.
pub const ROUTE_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// The default output device and its nominal rate, as CPAL reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    /// Stable device identifier (the CoreAudio device UID on macOS).
    pub device: String,
    /// The device's current nominal sample rate in hertz.
    pub nominal_hz: u32,
}

impl OutputRoute {
    /// Observe the current default output without opening a stream or changing
    /// the device. The backend query may block and belongs on control.
    pub fn probe_default_on_control() -> Option<Self> {
        let device = cpal::default_host().default_output_device()?;
        route_of(&device)
    }
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

    fn has_pending_candidate(&self) -> bool {
        self.candidate.is_some()
    }
}

const DROP_JOIN_BOUND: Duration = Duration::from_millis(50);

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<OutputRoute>>,
    /// Incremented after each publication; 0 means nothing published yet.
    generation: AtomicU64,
    stop: AtomicBool,
    exited: Mutex<bool>,
    exited_signal: Condvar,
}

/// Owner of the default-route observation thread.
pub struct OutputRouteWatch {
    shared: Arc<Shared>,
    wake: Option<Box<dyn Fn() + Send + Sync>>,
    handle: Option<JoinHandle<()>>,
}
impl OutputRouteWatch {
    /// Starts observing the actual default output route. On macOS, reacts
    /// immediately to CoreAudio HAL default-device and sample-rate property
    /// notifications instead of 500 ms polling.
    pub fn spawn() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            if let Ok(mut listener) = cpal::platform::CoreAudioOutputRouteListener::new() {
                let mut seen_gen = listener.generation();
                return Self::spawn_event_driven(
                    ROUTE_SETTLE_DELAY,
                    ROUTE_SAMPLE_INTERVAL,
                    OutputRoute::probe_default_on_control,
                    move |timeout| {
                        listener.sync_default_device();
                        let _ = listener.wait_for_change(&mut seen_gen, timeout);
                        listener.sync_default_device();
                    },
                    Some(Box::new(
                        cpal::platform::CoreAudioOutputRouteListener::wake_waiters,
                    )),
                );
            }
        }
        Self::spawn_with(
            Duration::from_millis(500),
            OutputRoute::probe_default_on_control,
        )
    }

    pub(crate) fn spawn_with(
        interval: Duration,
        probe: impl FnMut() -> Option<OutputRoute> + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn_event_driven(interval, interval, probe, std::thread::park_timeout, None)
    }

    fn spawn_event_driven(
        settle_delay: Duration,
        fallback_interval: Duration,
        mut probe: impl FnMut() -> Option<OutputRoute> + Send + 'static,
        mut wait_for_event: impl FnMut(Duration) + Send + 'static,
        wake: Option<Box<dyn Fn() + Send + Sync>>,
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
                    if watched.stop.load(Acquire) {
                        break;
                    }
                    if debounce.has_pending_candidate() {
                        std::thread::park_timeout(settle_delay);
                    } else {
                        wait_for_event(fallback_interval);
                    }
                }
                let mut exited = watched
                    .exited
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                *exited = true;
                watched.exited_signal.notify_all();
            })
            .map_err(|error| format!("could not start output route watch: {error}"))?;
        Ok(Self {
            shared,
            wake,
            handle: Some(handle),
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
        if let Some(wake) = &self.wake {
            wake();
        }
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let guard = self
                .shared
                .exited
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (guard, _) = self
                .shared
                .exited_signal
                .wait_timeout_while(guard, DROP_JOIN_BOUND, |exited| !*exited)
                .unwrap_or_else(PoisonError::into_inner);
            let exited = *guard;
            drop(guard);
            if exited {
                let _ = handle.join();
            }
        }
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
        // Drop joins the worker before returning, so the probe sender is already dropped.
        loop {
            match probe_calls.try_recv() {
                Ok(()) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    panic!("watch thread was still alive after drop(watch)")
                }
            }
        }
    }

    #[test]
    fn drop_does_not_hang_if_probe_is_blocked() {
        let (entered, probe_entered) = mpsc::sync_channel(1);
        let (release, wait_release) = mpsc::sync_channel(1);
        let watch = OutputRouteWatch::spawn_with(Duration::from_secs(60), move || {
            let _ = entered.try_send(());
            let _ = wait_release.recv();
            None
        })
        .unwrap();
        probe_entered
            .recv_timeout(Duration::from_secs(5))
            .expect("probe started");
        let start = Instant::now();
        drop(watch);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "drop stalled on blocked probe"
        );
        let _ = release.send(());
    }

    #[test]
    fn event_driven_signal_publishes_route_without_polling_delay() {
        let current = Arc::new(Mutex::new(route("speakers", 48_000)));
        let probes = Arc::new(AtomicU64::new(0));
        let signal = Arc::new((Mutex::new(0u64), Condvar::new()));

        let probe_current = Arc::clone(&current);
        let probe_count = Arc::clone(&probes);
        let wait_signal = Arc::clone(&signal);
        let wake_signal = Arc::clone(&signal);
        let mut seen_event = 0u64;

        let watch = OutputRouteWatch::spawn_event_driven(
            Duration::from_millis(2),
            Duration::from_secs(60),
            move || {
                probe_count.fetch_add(1, Release);
                probe_current
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            },
            move |timeout| {
                let (lock, cv) = &*wait_signal;
                let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let (guard, _) = cv
                    .wait_timeout_while(guard, timeout, |generation| *generation == seen_event)
                    .unwrap_or_else(PoisonError::into_inner);
                seen_event = *guard;
            },
            Some(Box::new(move || {
                let (lock, cv) = &*wake_signal;
                let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                *guard += 1;
                cv.notify_all();
            })),
        )
        .unwrap();

        let mut seen = 0;
        let deadline = Instant::now() + Duration::from_millis(250);
        let initial = loop {
            if let Some(r) = watch.latest_if_newer(&mut seen) {
                break r;
            }
            assert!(Instant::now() < deadline, "initial route never published");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(Some(initial), route("speakers", 48_000));

        // While idle, the watch stays blocked on the event signal rather than polling.
        std::thread::sleep(Duration::from_millis(20));
        let idle_probes = probes.load(Acquire);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            probes.load(Acquire),
            idle_probes,
            "idle event-driven watch must not poll"
        );

        // Switch sample rate and trigger the HAL-style signal; publication must happen
        // promptly (~2 ms settle) despite the 60 s fallback interval.
        *current.lock().unwrap_or_else(PoisonError::into_inner) = route("speakers", 96_000);
        let signaled_at = Instant::now();
        {
            let (lock, cv) = &*signal;
            let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            *guard += 1;
            cv.notify_all();
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        let updated = loop {
            if let Some(r) = watch.latest_if_newer(&mut seen) {
                break r;
            }
            assert!(Instant::now() < deadline, "rate change never published");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(Some(updated), route("speakers", 96_000));
        assert!(
            signaled_at.elapsed() < Duration::from_millis(200),
            "event-driven update exceeded reaction bound"
        );
        drop(watch);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn repeated_route_listener_retire_and_reregister_exceeds_sixteen_cycles() {
        for _ in 0..24 {
            let mut listener = cpal::platform::CoreAudioOutputRouteListener::new()
                .expect("CoreAudio HAL route listener must not exhaust slots");
            listener.sync_default_device();
            drop(listener);
        }
        let watch = OutputRouteWatch::spawn().expect("spawn event-driven OutputRouteWatch");
        drop(watch);
    }
}
