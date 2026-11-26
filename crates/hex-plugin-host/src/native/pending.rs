//! Pre-admission native helper startup, handshake, and configuration phase.
use super::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ChildCustodyProbeKey {
    helper: PathBuf,
    len: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
}

static CHILD_CUSTODY_PROBED: OnceLock<Mutex<HashSet<ChildCustodyProbeKey>>> = OnceLock::new();

fn child_custody_probe_key(helper: &Path) -> Option<ChildCustodyProbeKey> {
    let metadata = std::fs::metadata(helper).ok()?;
    let modified = metadata.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(ChildCustodyProbeKey {
        helper: helper.to_path_buf(),
        len: metadata.len(),
        mtime_secs: since_epoch.as_secs(),
        mtime_nanos: since_epoch.subsec_nanos(),
    })
}

fn is_child_custody_probe_cached(key: &ChildCustodyProbeKey) -> bool {
    CHILD_CUSTODY_PROBED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|set| set.contains(key))
        .unwrap_or(false)
}

fn record_child_custody_probe_cached(key: ChildCustodyProbeKey) {
    if let Ok(mut set) = CHILD_CUSTODY_PROBED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        set.insert(key);
    }
}

/// Source-bound configured component awaiting actual graph presentation facts.
/// No processing/state capture or canonical/audio acceptance has occurred. Keep
/// on control; dropping it cancels and joins this exact child through its lease.
/// Derive the graph plan from `facts`, then consume it once through completion.
pub struct Pending {
    pub(super) lease: Lease,
    pub(super) control_channel: Channel,
    pub(super) control_packet: Packet,
    pub(super) config: Configuration,
    pub(super) facts: Facts,
    output_buses: Option<Box<[hex_plugin_state::buses::BusDescriptor]>>,
    pub(super) limits: Limits,
    pub(super) packet: Packet,
    pub(super) source: Source,
    pub(super) build: InstanceBuild,
    pub(super) offline: bool,
}
impl Pending {
    /// Preserve actual failed-construction custody for the host's already
    /// reserved Root cell. Failure grants no source/graph admission or disposal.
    pub fn prepare_retained_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
        timings: Option<&mut StartupTimings>,
    ) -> std::result::Result<Self, PreparationFailure> {
        Self::prepare_retained(
            build,
            source,
            preparation,
            None,
            cancellation,
            |_| {},
            timings,
        )
    }
    /// The retained form of canonical startup restore; original state is not
    /// replaced by capture or by any later preparation failure.
    pub fn prepare_restored_retained_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> std::result::Result<Self, PreparationFailure> {
        Self::prepare_retained(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |_| {},
            None,
        )
    }
    pub fn prepare_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare(build, source, preparation, cancellation, |_| {})
    }
    /// Recreate captured state on a new source-bound instance. This never
    /// edits the caller's canonical envelope, even if later capture differs.
    pub fn prepare_restored_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare_with_restore(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |_| {},
        )
    }
    /// Configure a new source in the SDK's offline mode from confirmed state.
    /// The caller owns it independently of any live processor or output.
    pub fn prepare_offline_restored_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare_with_restore(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |command| {
                command.arg("--offline");
            },
        )
        .map(|mut pending| {
            pending.offline = true;
            pending
        })
    }
    pub(super) fn prepare(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> Result<Self> {
        Self::prepare_with_restore(build, source, preparation, None, cancellation, setup)
    }
    pub(super) fn prepare_with_restore(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> Result<Self> {
        Self::prepare_retained(
            build,
            source,
            preparation,
            restore,
            cancellation,
            setup,
            None,
        )
        .map_err(PreparationFailure::finish_on_control)
    }
    pub(super) fn prepare_retained(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
        mut timings: Option<&mut StartupTimings>,
    ) -> std::result::Result<Self, PreparationFailure> {
        let options = preparation;
        cancellation
            .token
            .check()
            .map_err(PreparationFailure::before_native)?;
        options
            .limits
            .validate()
            .map_err(PreparationFailure::before_native)?;
        let config = Configuration {
            binding: options.binding,
            identity: source.identity,
            restore,
            sample_rate: options.sample_rate,
            quantum: options.quantum,
            input_channels: options.input_channels,
            output_channels: options.output_channels,
            bundle: storage::copy_configuration_path(
                source
                    .bundle
                    .canonical_path
                    .to_str()
                    .ok_or(Error::Protocol("bundle path encoding"))
                    .map_err(PreparationFailure::before_native)?,
            )
            .map_err(PreparationFailure::before_native)?,
            executable: storage::copy_configuration_path(&source.executable.relative_path)
                .map_err(PreparationFailure::before_native)?,
        };
        let request = config.encode().map_err(PreparationFailure::before_native)?;
        // Establish the eventual Pending's bounded provenance copy before any
        // probe or instance child starts, never through a late derived Clone.
        let owned_build = build
            .try_copy_on_control()
            .map_err(Error::Validation)
            .map_err(PreparationFailure::before_native)?;
        // Verify helper build and selected source once per spawn instead of
        // re-hashing multi-MB Mach-O binaries five times synchronously on the
        // control worker. The supervised child also verifies exact SHA-256 of
        // both Info.plist and the executable right before loading the bundle.
        measure_revalidation(
            timings
                .as_deref_mut()
                .map(|timings| &mut timings.revalidation),
            || {
                source.unchanged_fast()?;
                build.unchanged().map_err(Error::Validation)
            },
        )
        .map_err(PreparationFailure::before_native)?;
        // Verify process-wide SIGCHLD waitable-child custody once per verified
        // helper executable identity instead of spawning an extra `.app` process
        // on every plugin instantiation. A missing/auto-reaped child still refuses
        // before any SDK instance starts on the first spawn.
        let probe_started = timings.as_ref().map(|_| Instant::now());
        let probe_key = child_custody_probe_key(build.helper());
        let already_verified = probe_key
            .as_ref()
            .is_some_and(is_child_custody_probe_cached);
        if !already_verified {
            let probe_until =
                end_after(options.limits.startup).map_err(PreparationFailure::before_native)?;
            let probe = Command::new(build.helper())
                .arg("--child-custody-probe")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(Error::Io)
                .map_err(PreparationFailure::before_native)?;
            let mut probe = ChildCustody::Owned(probe, None);
            let status = match wait(&mut probe, probe_until, &mut || cancellation.token.check()) {
                Ok(status) => status,
                Err(cause) => {
                    return Err(PreparationFailure::probe(
                        cause,
                        probe,
                        options.limits.retirement,
                    ));
                }
            };
            if !status.success() {
                return Err(PreparationFailure::before_native(Error::Custody(
                    "waitable-child preflight did not exit normally".into(),
                )));
            }
            if let Some(key) = probe_key {
                record_child_custody_probe_cached(key);
            }
        }
        if let Some((started, timings)) = probe_started.zip(timings.as_deref_mut()) {
            timings.waitable_probe = Some(started.elapsed());
        }
        let (data, child_data) = UnixStream::pair()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        let (control_data, child_control) = UnixStream::pair()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        let PreparationCancellation {
            token: cancellation,
            receiver: child_cancel,
        } = cancellation;
        let channel = Channel::new(data).map_err(PreparationFailure::before_native)?;
        let control_channel =
            Channel::new(control_data).map_err(PreparationFailure::before_native)?;
        let mut packet = Packet::new().map_err(PreparationFailure::before_native)?;
        let control_packet = Packet::new().map_err(PreparationFailure::before_native)?;
        let mut command = Command::new(build.helper());
        command
            .arg("--supervised")
            .env("HEX_VST3_SHM_RING", "1")
            .env(
                "HEX_VST3_TRANSPORT_SPACE",
                if source.transport_space { "1" } else { "0" },
            )
            .stdin(Stdio::from(OwnedFd::from(child_data)))
            .stdout(Stdio::from(OwnedFd::from(child_cancel)))
            .stderr(Stdio::from(OwnedFd::from(child_control)));
        if source.grouped_editor_callbacks {
            command.arg("--test-grouped-editor");
        }
        setup(&mut command);
        let deadline = if source.live_deadline && !command.get_args().any(|arg| arg == "--offline")
        {
            deadline::Permit::reserve(&config)
        } else {
            None
        };
        command.env(
            "HEX_VST3_WORKER_DEADLINE_ADMITTED",
            if deadline.is_some() { "1" } else { "0" },
        );
        let instance_started = timings.as_ref().map(|_| Instant::now());
        let child = command
            .spawn()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        // Command retains its Stdio descriptors for reuse. Close the parent
        // copies now, otherwise a crashed/cancelled child cannot produce EOF.
        drop(command);
        // The process exists before Ready. The local lease below owns every
        // early error path, including malformed data/source changes/timeouts.
        let mut pending = Lease {
            child: ChildCustody::Owned(child, deadline),
            channel,
            cancellation,
            shm: None,
            retirement_ms: duration_to_ms_u32(options.limits.retirement),
            #[cfg(test)]
            reap_probe: None,
        };
        let token = pending.cancellation.clone();
        let prepared = (|| {
            let until = end_after(options.limits.startup)?;
            pending
                .channel
                .send(&request, until, &mut || token.check())?;
            pending
                .channel
                .send(&config.startup_with_output_buses()?, until, &mut || {
                    token.check()
                })?;
            let shm = pending
                .channel
                .create_and_send_shm_fd(until, &mut || token.check())?;
            pending.shm = Some(Box::new(shm));
            pending
                .channel
                .read(&mut packet, until, &mut || token.check())?;
            if packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    packet.body()?,
                    config.binding,
                )?));
            }
            if packet.magic() != b"HXVIFA05" {
                return Err(Error::Protocol("worker did not report preparation facts"));
            }
            let facts = Facts::decode(packet.body()?, &config)?;
            pending
                .channel
                .read(&mut packet, until, &mut || token.check())?;
            if packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    packet.body()?,
                    config.binding,
                )?));
            }
            if packet.magic() != b"HXVIOB01" {
                return Err(Error::Protocol(
                    "worker did not report its negotiated output buses",
                ));
            }
            let output_buses = protocol::output_buses::decode(packet.body()?, &config)?;
            packet.shrink_to_pcm();
            if let Some((started, timings)) = instance_started.zip(timings.as_deref_mut()) {
                timings.instance_to_facts = Some(started.elapsed());
            }
            token.check()?;
            match pending.child.observe()? {
                None => {}
                Some(_) => return Err(Error::Custody("prepared worker already exited".into())),
            }
            Ok((facts, output_buses))
        })();
        let (facts, output_buses) = match prepared {
            Ok(facts) => facts,
            Err(cause) => {
                return Err(PreparationFailure::native(
                    cause,
                    pending,
                    options.limits.retirement,
                ));
            }
        };
        Ok(Self {
            lease: pending,
            control_channel,
            control_packet,
            config,
            facts,
            output_buses,
            limits: options.limits,
            packet,
            source,
            build: owned_build,
            offline: false,
        })
    }
    /// Original negotiated SDK slots, including inactive outputs, on the control lane.
    /// None means the helper could not identify every descriptor. These facts do not route audio.
    pub fn negotiated_audio_outputs(&self) -> Option<&[hex_plugin_state::buses::BusDescriptor]> {
        self.output_buses.as_deref()
    }
    pub fn facts(&self) -> Facts {
        self.facts
    }
    /// Immutable facts of this actual pending lease, not admission authority.
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub fn cancellation_on_control(&self) -> Cancellation {
        self.lease.cancellation.clone()
    }
    /// Cancel this actual pending source and retain its physical cleanup owner
    /// for Root polling. No source/graph can be accepted from the returned error.
    pub fn cancel_retained_on_control(self) -> PreparationFailure {
        PreparationFailure::native(Error::Cancelled, self.lease, self.limits.retirement)
    }
    /// Explicit pending cancellation with actual direct reap. This is OS
    /// retirement, not a successful SDK close or prepared-instance terminal.
    pub fn cancel_and_reap_on_control(mut self) -> Result<Retirement> {
        self.lease.cancel_native();
        let status = wait(
            &mut self.lease.child,
            end_after(self.limits.retirement)?,
            &mut || Ok(()),
        )?;
        Ok(Retirement {
            status,
            terminal: None,
        })
    }
    /// Supply actual per-direction graph/device contributions, or explicit None
    /// when unavailable. A present optional interface requires known facts;
    /// absence may complete without inventing zero. Source and final Ready are
    /// checked again; a changed reported latency invalidates the caller's plan.
    pub fn complete_on_control(
        self,
        presentation: Option<PresentationLatency>,
    ) -> Result<Prepared> {
        self.complete_with_plan_on_control(presentation.into())
    }
    pub fn complete_with_plan_on_control(
        self,
        presentation: protocol::PresentationPlan,
    ) -> Result<Prepared> {
        self.complete_retained_on_control(presentation)
            .map_err(PreparationFailure::finish_on_control)
    }
    /// Completion errors retain the same actual child for Root's independent
    /// cleanup observation. Callers can explicitly allow unknown presentation.
    pub fn complete_retained_on_control(
        mut self,
        presentation: impl Into<protocol::PresentationPlan>,
    ) -> std::result::Result<Prepared, PreparationFailure> {
        let presentation = presentation.into();
        let token = self.lease.cancellation.clone();
        let result = (|| {
            token.check()?;
            // Fast stat-fingerprint check detects mid-handshake bundle or
            // Info.plist modifications without re-reading multi-MB Mach-O binaries.
            self.source.unchanged_fast()?;
            let expected = presentation.expected(self.facts.presentation_available)?;
            let until = end_after(self.limits.startup)?;
            let request = protocol::presentation_request(self.config.binding, presentation)?;
            self.lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.lease
                .channel
                .read(&mut self.packet, until, &mut || token.check())?;
            if self.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.packet.body()?,
                    self.config.binding,
                )?));
            }
            if self.packet.magic() != b"HXVIRD06" {
                return Err(Error::Protocol(
                    "missing final prepared instance acknowledgement",
                ));
            }
            let ready = Ready::decode(self.packet.body()?, &self.config, presentation)?;
            self.packet.shrink_to_pcm();
            if ready.restored != self.facts.restored
                || ready.algorithmic_latency_frames != self.facts.algorithmic_latency_frames
                || ready.tail_frames != self.facts.tail_frames
                || ready.presentation != expected
            {
                return Err(Error::Protocol(
                    "prepared facts changed before final acknowledgement",
                ));
            }
            if self.lease.child.observe()?.is_some() {
                return Err(Error::Custody("prepared worker already exited".into()));
            }
            token.check()?;
            Ok(ready)
        })();
        let ready = match result {
            Ok(ready) => ready,
            Err(cause) => {
                return Err(PreparationFailure::native(
                    cause,
                    self.lease,
                    self.limits.retirement,
                ));
            }
        };
        let control = ControlClient::new(
            self.control_channel,
            self.control_packet,
            &self.config,
            self.limits,
            self.lease.cancellation.clone(),
        );
        Ok(Prepared {
            lease: self.lease,
            config: self.config,
            ready,
            output_buses: self.output_buses,
            limits: self.limits,
            sequence: 0,
            packet: self.packet,
            retired: None,
            faulted: false,
            live_capture: None,
            live_capture_sequence: 0,
            edit_query_sequence: 0,
            idle_flush_sequence: 0,
            last_edit_sequence: 0,
            last_pcm_edit_sequence: 0,
            last_pcm_restoration_sequence: 0,
            last_edit_frame: 0,
            parameter_page: None,
            parameter_page_sequence: 0,
            authored_parameter_metadata: None,
            control: Some(Box::new(control)),
            grouped_editor_callbacks: self.source.grouped_editor_callbacks,
            offline: self.offline,
        })
    }
}

#[cfg(test)]
mod probe_cache_tests {
    use super::*;

    #[test]
    fn child_custody_probe_cache_keys_on_helper_identity() {
        let dir = std::env::temp_dir().join(format!("hex-probe-cache-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("helper-a");
        let b = dir.join("helper-b");
        std::fs::write(&a, b"v1").unwrap();
        std::fs::write(&b, b"v1").unwrap();

        let key_a = child_custody_probe_key(&a).unwrap();
        let key_b = child_custody_probe_key(&b).unwrap();
        assert!(!is_child_custody_probe_cached(&key_a));
        assert!(!is_child_custody_probe_cached(&key_b));

        record_child_custody_probe_cached(key_a.clone());
        assert!(is_child_custody_probe_cached(&key_a));
        assert!(!is_child_custody_probe_cached(&key_b));

        // Changing helper binary content invalidates the cache key.
        std::fs::write(&a, b"v2-longer").unwrap();
        let key_a_changed = child_custody_probe_key(&a).unwrap();
        assert_ne!(key_a, key_a_changed);
        assert!(!is_child_custody_probe_cached(&key_a_changed));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
