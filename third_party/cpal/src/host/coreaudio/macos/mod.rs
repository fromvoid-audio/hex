#![allow(deprecated)]
use super::{asbd_from_config, check_os_status, frames_to_duration, HostTimebase};

use super::OSStatus;
use crate::host::coreaudio::macos::loopback::LoopbackDevice;
use crate::traits::{HostTrait, StreamTrait};
use crate::{BackendSpecificError, DevicesError, PauseStreamError, PlayStreamError};
use coreaudio::audio_unit::AudioUnit;
use objc2_core_audio::AudioDeviceID;
use std::sync::{mpsc, Arc, Mutex, Weak};

pub use self::enumerate::{default_input_device, default_output_device, Devices};

use objc2_core_audio::{
    kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, AudioObjectPropertyAddress,
};
use property_listener::AudioObjectPropertyListener;

mod device;
pub mod enumerate;
mod loopback;
mod property_listener;
use device::OutputTimingWatch;
pub use device::{Device, OutputTiming, OutputTimingReceipt};

/// Coreaudio host, the default host on macOS.
#[derive(Debug)]
pub struct Host;

impl Host {
    pub fn new() -> Result<Self, crate::HostUnavailable> {
        Ok(Host)
    }
}

impl HostTrait for Host {
    type Devices = Devices;
    type Device = Device;

    fn is_available() -> bool {
        // Assume coreaudio is always available
        true
    }

    fn devices(&self) -> Result<Self::Devices, DevicesError> {
        Devices::new()
    }

    fn default_input_device(&self) -> Option<Self::Device> {
        default_input_device()
    }

    fn default_output_device(&self) -> Option<Self::Device> {
        default_output_device()
    }
}

/// Type alias for the error callback to reduce complexity
type ErrorCallback = Box<dyn FnMut(crate::StreamError) + Send + 'static>;

/// Invoke error callback, recovering from poisoned mutex if needed.
/// Returns true if callback was invoked, false if skipped due to WouldBlock.
#[inline]
fn invoke_error_callback<E>(error_callback: &Arc<Mutex<E>>, err: crate::StreamError) -> bool
where
    E: FnMut(crate::StreamError) + Send,
{
    match error_callback.try_lock() {
        Ok(mut cb) => {
            cb(err);
            true
        }
        Err(std::sync::TryLockError::Poisoned(guard)) => {
            // Recover from poisoned lock to still report this error
            guard.into_inner()(err);
            true
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            // Skip if callback is busy
            false
        }
    }
}

/// Manages device disconnection listener on a dedicated thread to ensure the
/// AudioObjectPropertyListener is always created and dropped on the same thread.
/// This avoids potential threading issues with CoreAudio APIs.
///
/// When a device disconnects, this manager:
/// 1. Attempts to pause the stream to stop audio I/O
/// 2. Calls the error callback with `StreamError::DeviceNotAvailable`
///
/// The dedicated thread architecture ensures `Stream` can implement `Send`.
struct DisconnectManager {
    _shutdown_tx: mpsc::Sender<()>,
}

impl DisconnectManager {
    /// Create a new DisconnectManager that monitors device disconnection on a dedicated thread
    fn new(
        device_id: AudioDeviceID,
        stream_weak: Weak<Mutex<StreamInner>>,
        error_callback: Arc<Mutex<ErrorCallback>>,
    ) -> Result<Self, crate::BuildStreamError> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let (disconnect_tx, disconnect_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        // Spawn dedicated thread to own the AudioObjectPropertyListener
        let disconnect_tx_clone = disconnect_tx.clone();
        std::thread::spawn(move || {
            let property_address = AudioObjectPropertyAddress {
                mSelector: kAudioDevicePropertyDeviceIsAlive,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            };

            // Create the listener on this dedicated thread
            let disconnect_fn = move || {
                let _ = disconnect_tx_clone.send(());
            };
            match AudioObjectPropertyListener::new(device_id, property_address, disconnect_fn) {
                Ok(_listener) => {
                    let _ = ready_tx.send(Ok(()));
                    // Drop the listener on this thread after receiving a shutdown signal
                    let _ = shutdown_rx.recv();
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });

        // Wait for listener creation to complete or fail
        ready_rx
            .recv()
            .map_err(|_| crate::BuildStreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "Disconnect listener thread terminated unexpectedly".to_string(),
                },
            })??;

        // Handle disconnect events on the main thread pool
        let stream_weak_clone = stream_weak.clone();
        let error_callback_clone = error_callback.clone();
        std::thread::spawn(move || {
            while disconnect_rx.recv().is_ok() {
                // Check if stream still exists
                if let Some(stream_arc) = stream_weak_clone.upgrade() {
                    // First, try to pause the stream to stop playback
                    if let Ok(mut stream_inner) = stream_arc.try_lock() {
                        let _ = stream_inner.pause();
                    }

                    // Always try to notify about device disconnection
                    invoke_error_callback(
                        &error_callback_clone,
                        crate::StreamError::DeviceNotAvailable,
                    );
                } else {
                    // Stream is gone, exit the handler thread
                    break;
                }
            }
        });

        Ok(DisconnectManager {
            _shutdown_tx: shutdown_tx,
        })
    }
}

struct StreamInner {
    playing: bool,
    timing_watch: Option<OutputTimingWatch>,
    audio_unit: AudioUnit,
    // Track the device with which the audio unit was spawned.
    //
    // We must do this so that we can avoid changing the device sample rate if there is already
    // a stream associated with the device.
    #[allow(dead_code)]
    device_id: AudioDeviceID,
    /// Manage the lifetime of the aggregate device used
    /// for loopback recording
    _loopback_device: Option<LoopbackDevice>,
}

impl Drop for StreamInner {
    fn drop(&mut self) {
        self.clear_timing_watch();
    }
}

impl StreamInner {
    fn clear_timing_watch(&mut self) {
        if let Some(watch) = self.timing_watch.take() {
            watch.retire(&self.audio_unit);
        }
    }

    fn play(&mut self) -> Result<(), PlayStreamError> {
        if !self.playing {
            self.clear_timing_watch();
            if let Err(e) = self.audio_unit.start() {
                let description = format!("{e}");
                let err = BackendSpecificError { description };
                return Err(err.into());
            }
            self.playing = true;
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), PauseStreamError> {
        if self.playing {
            self.clear_timing_watch();
            if let Err(e) = self.audio_unit.stop() {
                let description = format!("{e}");
                let err = BackendSpecificError { description };
                return Err(err.into());
            }
            self.playing = false;
        }
        Ok(())
    }
}

pub struct Stream {
    inner: Arc<Mutex<StreamInner>>,
    // Manages the device disconnection listener separately to allow Stream to be Send.
    // The DisconnectManager contains the non-Send AudioObjectPropertyListener.
    _disconnect_manager: DisconnectManager,
}

impl Stream {
    fn new(
        inner: StreamInner,
        error_callback: ErrorCallback,
    ) -> Result<Self, crate::BuildStreamError> {
        let device_id = inner.device_id;
        let inner_arc = Arc::new(Mutex::new(inner));
        let weak_inner = Arc::downgrade(&inner_arc);

        let error_callback = Arc::new(Mutex::new(error_callback));
        let disconnect_manager = DisconnectManager::new(device_id, weak_inner, error_callback)?;

        Ok(Self {
            inner: inner_arc,
            _disconnect_manager: disconnect_manager,
        })
    }
}

impl StreamTrait for Stream {
    fn play(&self) -> Result<(), PlayStreamError> {
        let mut stream = self
            .inner
            .lock()
            .map_err(|_| PlayStreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "A cpal stream operation panicked while holding the lock - this is a bug, please report it".to_string(),
                },
            })?;

        stream.play()
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        let mut stream = self
            .inner
            .lock()
            .map_err(|_| PauseStreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "A cpal stream operation panicked while holding the lock - this is a bug, please report it".to_string(),
                },
            })?;

        stream.pause()
    }
}

#[cfg(test)]
mod test {
    use crate::{
        default_host,
        traits::{DeviceTrait, HostTrait, StreamTrait},
        Sample,
    };

    #[test]
    fn test_play() {
        let host = default_host();
        let device = host.default_output_device().unwrap();

        let mut supported_configs_range = device.supported_output_configs().unwrap();
        let supported_config = supported_configs_range
            .next()
            .unwrap()
            .with_max_sample_rate();
        let config = supported_config.config();

        let stream = device
            .build_output_stream(
                &config,
                write_silence::<f32>,
                move |err| println!("Error: {err}"),
                None, // None=blocking, Some(Duration)=timeout
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_record() {
        let host = default_host();
        let device = host.default_input_device().unwrap();
        println!("Device: {:?}", device.name());

        let mut supported_configs_range = device.supported_input_configs().unwrap();
        println!("Supported configs:");
        for config in supported_configs_range.clone() {
            println!("{:?}", config)
        }
        let supported_config = supported_configs_range
            .next()
            .unwrap()
            .with_max_sample_rate();
        let config = supported_config.config();

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &crate::InputCallbackInfo| {
                    // react to stream events and read or write stream data here.
                    println!("Got data: {:?}", &data[..25]);
                },
                move |err| println!("Error: {err}"),
                None, // None=blocking, Some(Duration)=timeout
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_record_output() {
        if std::env::var("CI").is_ok() {
            println!("Skipping test_record_output in CI environment due to permissions");
            return;
        }

        let host = default_host();
        let device = host.default_output_device().unwrap();

        let mut supported_configs_range = device.supported_output_configs().unwrap();
        let supported_config = supported_configs_range
            .next()
            .unwrap()
            .with_max_sample_rate();
        let config = supported_config.config();

        println!("Building input stream");
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &crate::InputCallbackInfo| {
                    // react to stream events and read or write stream data here.
                    println!("Got data: {:?}", &data[..25]);
                },
                move |err| println!("Error: {err}"),
                None, // None=blocking, Some(Duration)=timeout
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    fn write_silence<T: Sample>(data: &mut [T], _: &crate::OutputCallbackInfo) {
        for sample in data.iter_mut() {
            *sample = Sample::EQUILIBRIUM;
        }
    }
}

/// Current-thread time constraint for an auxiliary audio worker.
///
/// Construct on a dedicated, ordinarily scheduled worker. No call belongs on
/// the device callback. The caller owns admission, deadlines and retirement.
pub struct WorkerTimeConstraint {
    thread: mach2::mach_types::thread_t,
    policy: mach2::thread_policy::thread_time_constraint_policy_data_t,
    active: bool,
    _current_thread_only: std::marker::PhantomData<*mut ()>,
}

impl WorkerTimeConstraint {
    /// Acquire the current thread and validate the time constraint without
    /// changing its scheduling class. Existing real-time threads refuse.
    pub fn for_current_thread(
        period: std::time::Duration,
        computation: std::time::Duration,
        constraint: std::time::Duration,
    ) -> Result<Self, BackendSpecificError> {
        use mach2::{mach_init::mach_thread_self, port::MACH_PORT_NULL, thread_policy::*};

        let refused = || BackendSpecificError {
            description: "invalid auxiliary audio worker time constraint".into(),
        };
        if computation.is_zero() || computation > constraint / 2 || constraint > period {
            return Err(refused());
        }
        let ratio = super::read_host_timebase()?;
        let ticks = |duration: std::time::Duration| {
            let numerator = duration.as_nanos().checked_mul(u128::from(ratio.denom))?;
            u32::try_from(numerator.checked_div(u128::from(ratio.numer))?)
                .ok()
                .filter(|value| *value != 0)
        };
        let policy = thread_time_constraint_policy_data_t {
            period: ticks(period).ok_or_else(refused)?,
            computation: ticks(computation).ok_or_else(refused)?,
            constraint: ticks(constraint).ok_or_else(refused)?,
            preemptible: 1,
        };
        // SAFETY: this returns a send right for the calling thread. The
        // non-Send owner deallocates it even when the policy check fails.
        let thread = unsafe { mach_thread_self() };
        if thread == MACH_PORT_NULL {
            return Err(refused());
        }
        let owner = Self {
            thread,
            policy,
            active: false,
            _current_thread_only: std::marker::PhantomData,
        };
        if !owner.is_ordinary()? {
            return Err(refused());
        }
        Ok(owner)
    }

    /// Apply the configured time constraint to the current worker.
    pub fn enter(&mut self) -> Result<(), BackendSpecificError> {
        use mach2::thread_policy::*;
        if !self.active {
            // SAFETY: the native record matches the flavor and count and the
            // current worker still owns the acquired thread right.
            let status = unsafe {
                thread_policy_set(
                    self.thread,
                    THREAD_TIME_CONSTRAINT_POLICY,
                    (&mut self.policy as *mut thread_time_constraint_policy_data_t).cast(),
                    THREAD_TIME_CONSTRAINT_POLICY_COUNT,
                )
            };
            super::check_os_status(status)?;
            self.active = true;
        }
        Ok(())
    }

    /// Return to ordinary scheduling. The caller may reapply its own QoS.
    pub fn leave(&mut self) -> Result<(), BackendSpecificError> {
        use mach2::thread_policy::*;
        if self.active {
            // SAFETY: the standard-policy flavor takes a zero-word argument.
            let status = unsafe {
                thread_policy_set(
                    self.thread,
                    THREAD_STANDARD_POLICY,
                    std::ptr::null_mut(),
                    THREAD_STANDARD_POLICY_COUNT,
                )
            };
            super::check_os_status(status)?;
            self.active = false;
        }
        Ok(())
    }

    fn is_ordinary(&self) -> Result<bool, BackendSpecificError> {
        use mach2::thread_policy::*;
        let mut policy = self.policy;
        let mut count = THREAD_TIME_CONSTRAINT_POLICY_COUNT;
        let mut default = 0;
        // SAFETY: the native getter writes into initialized storage for this
        // flavor, count and status flag.
        let status = unsafe {
            thread_policy_get(
                self.thread,
                THREAD_TIME_CONSTRAINT_POLICY,
                (&mut policy as *mut thread_time_constraint_policy_data_t).cast(),
                &mut count,
                &mut default,
            )
        };
        super::check_os_status(status)?;
        Ok(default != 0)
    }
}

impl Drop for WorkerTimeConstraint {
    fn drop(&mut self) {
        let _ = self.leave();
        // SAFETY: release exactly the owned current-thread send right.
        unsafe {
            mach2::mach_port::mach_port_deallocate(mach2::traps::mach_task_self(), self.thread);
        }
    }
}

#[cfg(test)]
mod worker_time_constraint_regression {
    use super::WorkerTimeConstraint;
    use std::time::Duration;

    #[test]
    fn current_thread_repeatedly_enters_and_releases_time_constraint() {
        let period = Duration::from_nanos(5_804_988);
        let computation = Duration::from_micros(300);
        let constraint = Duration::from_millis(4);
        let mut worker =
            WorkerTimeConstraint::for_current_thread(period, computation, constraint).unwrap();
        assert!(worker.is_ordinary().unwrap());
        for _ in 0..2 {
            worker.enter().unwrap();
            assert!(!worker.is_ordinary().unwrap());
            assert!(
                WorkerTimeConstraint::for_current_thread(period, computation, constraint).is_err()
            );
            worker.leave().unwrap();
            assert!(worker.is_ordinary().unwrap());
        }
    }
}
