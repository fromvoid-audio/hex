//! Measures exact immutable configuration comparisons for bounded runtime chains.
//! Synthetic fixtures do not represent deployable native effects or callback deadlines.

use hex_lattice::{
    NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::{DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceState};
use std::{error::Error, fmt, hint::black_box, sync::Arc, time::Instant};

const SYNTHETIC_DEVICE_ID: &str = "hex.synthetic.configuration-profile";
const REGULAR_TRACKS: usize = 256;
const RETURN_TRACKS: usize = 16;
const DEVICES_PER_CHAIN: usize = 64;
const DEFAULT_REPEATS: usize = 101;
const DEFAULT_WARMUPS: usize = 20;
const MAX_REPEATS: usize = 10_000;
const MAX_WARMUPS: usize = 10_000;

#[derive(Clone, Copy, Debug)]
struct Options {
    repeats: usize,
    warmups: usize,
    check_only: bool,
}

#[derive(Debug)]
struct ProfileError(String);

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ProfileError {}

#[derive(Clone, Copy, Debug)]
enum Candidate {
    SharedArc,
    SeparateEqual,
    LastDeviceChanged,
}

impl Candidate {
    const ALL: [Self; 3] = [
        Self::SharedArc,
        Self::SeparateEqual,
        Self::LastDeviceChanged,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::SharedArc => "shared_arc",
            Self::SeparateEqual => "separate_equal",
            Self::LastDeviceChanged => "last_device_changed",
        }
    }
}

struct OwnerGroup {
    name: &'static str,
    instances: Vec<Vec<PreparedProcessorInstance>>,
    separate_equal_instances: Vec<Vec<PreparedProcessorInstance>>,
    last_device_changed_instances: Vec<Vec<PreparedProcessorInstance>>,
    old: Vec<NativeRuntimeChain>,
    shared_arc: Vec<NativeRuntimeChain>,
    separate_equal: Vec<NativeRuntimeChain>,
    last_device_changed: Vec<NativeRuntimeChain>,
}

impl OwnerGroup {
    fn candidate(&self, candidate: Candidate) -> &[NativeRuntimeChain] {
        match candidate {
            Candidate::SharedArc => &self.shared_arc,
            Candidate::SeparateEqual => &self.separate_equal,
            Candidate::LastDeviceChanged => &self.last_device_changed,
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let Some(options) = parse_options(std::env::args().skip(1))? else {
        println!("usage: configuration_profile [--repeats 1..10000] [--warmup 0..10000] [--check]");
        return Ok(());
    };
    let device_type = DeviceTypeId::new(SYNTHETIC_DEVICE_ID)?;
    let regular_tracks = build_group("track", REGULAR_TRACKS, device_type)?;
    let return_tracks = build_group("return", RETURN_TRACKS, device_type)?;
    validate_cases(&regular_tracks)?;
    validate_cases(&return_tracks)?;

    println!(
        "fixture device_id={SYNTHETIC_DEVICE_ID} tracks={} returns={} devices_per_chain={} total_slots={} bytes_per_configuration={} configuration_bank_bytes={} configuration_bank_mib={:.0} fixture_payload_bytes={} fixture_payload_mib={:.0} mode=synthetic",
        REGULAR_TRACKS,
        RETURN_TRACKS,
        DEVICES_PER_CHAIN,
        (REGULAR_TRACKS + RETURN_TRACKS) * DEVICES_PER_CHAIN,
        MAX_NATIVE_DEVICE_STATE_BYTES,
        (REGULAR_TRACKS + RETURN_TRACKS) * DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES,
        ((REGULAR_TRACKS + RETURN_TRACKS) * DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES)
            as f64
            / (1024.0 * 1024.0),
        3 * (REGULAR_TRACKS + RETURN_TRACKS) * DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES,
        (3 * (REGULAR_TRACKS + RETURN_TRACKS) * DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES)
            as f64
            / (1024.0 * 1024.0),
    );
    println!("fixture_validation shared_arc=true separate_equal=true last_device_changed=false");

    if options.check_only {
        println!("functional_check=passed timings=not_run");
        return Ok(());
    }

    profile_group(&regular_tracks, options)?;
    profile_group(&return_tracks, options)?;
    profile_groups("all", &[&regular_tracks, &return_tracks], options)?;
    Ok(())
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Option<Options>, ProfileError> {
    let mut options = Options {
        repeats: DEFAULT_REPEATS,
        warmups: DEFAULT_WARMUPS,
        check_only: false,
    };
    let mut args = args;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--repeats" => {
                options.repeats = parse_bounded("--repeats", args.next(), 1, MAX_REPEATS)?;
            }
            "--warmup" => {
                options.warmups = parse_bounded("--warmup", args.next(), 0, MAX_WARMUPS)?;
            }
            "--check" => options.check_only = true,
            "--help" | "-h" => return Ok(None),
            _ => return Err(ProfileError(format!("unknown argument: {argument}"))),
        }
    }
    Ok(Some(options))
}

fn parse_bounded(
    name: &str,
    value: Option<String>,
    minimum: usize,
    maximum: usize,
) -> Result<usize, ProfileError> {
    let value = value.ok_or_else(|| ProfileError(format!("{name} needs a value")))?;
    let parsed = value.parse::<usize>().map_err(|_| {
        ProfileError(format!(
            "{name} must be an integer in {minimum}..={maximum}"
        ))
    })?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(ProfileError(format!(
            "{name} must be in {minimum}..={maximum}"
        )));
    }
    Ok(parsed)
}

fn build_group(
    name: &'static str,
    owner_count: usize,
    device_type: DeviceTypeId,
) -> Result<OwnerGroup, Box<dyn Error>> {
    let mut instances = Vec::new();
    instances.try_reserve_exact(owner_count)?;
    let mut old = Vec::new();
    old.try_reserve_exact(owner_count)?;

    for owner_index in 0..owner_count {
        let owner_id = u64::try_from(owner_index + 1)?;
        let owner = if name == "track" {
            ProcessorChainOwner::Track(owner_id)
        } else {
            ProcessorChainOwner::Return(owner_id)
        };
        let mut chain_instances = Vec::new();
        chain_instances.try_reserve_exact(DEVICES_PER_CHAIN)?;
        for device_index in 0..DEVICES_PER_CHAIN {
            let slot = owner_index
                .checked_mul(DEVICES_PER_CHAIN)
                .and_then(|base| base.checked_add(device_index))
                .ok_or_else(|| ProfileError("fixture slot count overflowed".into()))?;
            let configuration = Arc::new(NativeDeviceState::new(
                device_type,
                1,
                fixture_payload(slot)?,
            )?);
            chain_instances.push(instance(
                owner,
                u64::try_from(device_index + 1)?,
                device_type,
                configuration,
            ));
        }
        old.push(NativeRuntimeChain::new(clone_instances(&chain_instances)?)?);
        instances.push(chain_instances);
    }

    let mut shared_arc = Vec::new();
    shared_arc.try_reserve_exact(owner_count)?;
    for chain_instances in &instances {
        shared_arc.push(NativeRuntimeChain::new(clone_instances(chain_instances)?)?);
    }

    let mut separate_equal = Vec::new();
    separate_equal.try_reserve_exact(owner_count)?;
    let mut separate_equal_instances = Vec::new();
    separate_equal_instances.try_reserve_exact(owner_count)?;
    for chain_instances in &instances {
        let mut equal_instances = Vec::new();
        equal_instances.try_reserve_exact(DEVICES_PER_CHAIN)?;
        for original in chain_instances {
            let original_state = original
                .configuration
                .as_ref()
                .ok_or_else(|| ProfileError("fixture configuration is missing".into()))?;
            let independent_state = Arc::new(NativeDeviceState::new(
                device_type,
                original_state.state_version(),
                duplicate_payload(original_state.payload())?,
            )?);
            equal_instances.push(instance(
                original.owner,
                original.device_id,
                device_type,
                independent_state,
            ));
        }
        separate_equal.push(NativeRuntimeChain::new(clone_instances(&equal_instances)?)?);
        separate_equal_instances.push(equal_instances);
    }

    let mut last_device_changed = Vec::new();
    last_device_changed.try_reserve_exact(owner_count)?;
    let mut last_device_changed_instances = Vec::new();
    last_device_changed_instances.try_reserve_exact(owner_count)?;
    for chain_instances in &instances {
        let mut changed_instances = Vec::new();
        changed_instances.try_reserve_exact(DEVICES_PER_CHAIN)?;
        for (device_index, original) in chain_instances.iter().enumerate() {
            let original_state = original
                .configuration
                .as_ref()
                .ok_or_else(|| ProfileError("fixture configuration is missing".into()))?;
            let mut payload = duplicate_payload(original_state.payload())?;
            if device_index + 1 == DEVICES_PER_CHAIN {
                let last = payload
                    .last_mut()
                    .ok_or_else(|| ProfileError("fixture state payload is empty".into()))?;
                *last ^= 0xff;
            }
            let configuration = Arc::new(NativeDeviceState::new(
                device_type,
                original_state.state_version(),
                payload,
            )?);
            changed_instances.push(instance(
                original.owner,
                original.device_id,
                device_type,
                configuration,
            ));
        }
        last_device_changed.push(NativeRuntimeChain::new(clone_instances(
            &changed_instances,
        )?)?);
        last_device_changed_instances.push(changed_instances);
    }

    Ok(OwnerGroup {
        name,
        instances,
        separate_equal_instances,
        last_device_changed_instances,
        old,
        shared_arc,
        separate_equal,
        last_device_changed,
    })
}

fn clone_instances(
    instances: &[PreparedProcessorInstance],
) -> Result<Vec<PreparedProcessorInstance>, std::collections::TryReserveError> {
    let mut copies = Vec::new();
    copies.try_reserve_exact(instances.len())?;
    copies.extend(instances.iter().cloned());
    Ok(copies)
}

fn duplicate_payload(payload: &[u8]) -> Result<Vec<u8>, std::collections::TryReserveError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(payload.len())?;
    copy.extend_from_slice(payload);
    Ok(copy)
}

fn instance(
    owner: ProcessorChainOwner,
    device_id: u64,
    device_type: DeviceTypeId,
    configuration: Arc<NativeDeviceState>,
) -> PreparedProcessorInstance {
    PreparedProcessorInstance {
        owner,
        device_id,
        device_type,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(configuration),
        definition: None,
    }
}

fn fixture_payload(slot: usize) -> Result<Vec<u8>, std::collections::TryReserveError> {
    let mut payload = Vec::new();
    payload.try_reserve_exact(MAX_NATIVE_DEVICE_STATE_BYTES)?;
    payload.resize(MAX_NATIVE_DEVICE_STATE_BYTES, 0);
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = ((slot.wrapping_mul(31).wrapping_add(index.wrapping_mul(17))) % 251) as u8;
    }
    Ok(payload)
}

fn validate_cases(group: &OwnerGroup) -> Result<(), ProfileError> {
    for (owner_index, (old, shared, separate, changed)) in group
        .old
        .iter()
        .zip(&group.shared_arc)
        .zip(&group.separate_equal)
        .zip(&group.last_device_changed)
        .map(|(((old, shared), separate), changed)| (old, shared, separate, changed))
        .enumerate()
    {
        let original_instances = group
            .instances
            .get(owner_index)
            .ok_or_else(|| ProfileError(format!("{} fixture instances are missing", group.name)))?;
        let equal_instances = group
            .separate_equal_instances
            .get(owner_index)
            .ok_or_else(|| {
                ProfileError(format!("{} equal-state instances are missing", group.name))
            })?;
        let changed_instances = group
            .last_device_changed_instances
            .get(owner_index)
            .ok_or_else(|| {
                ProfileError(format!(
                    "{} changed-state instances are missing",
                    group.name
                ))
            })?;
        if original_instances.len() != DEVICES_PER_CHAIN
            || equal_instances.len() != DEVICES_PER_CHAIN
            || changed_instances.len() != DEVICES_PER_CHAIN
        {
            return Err(ProfileError(format!(
                "{} fixture chain length is invalid",
                group.name
            )));
        }
        for device_index in 0..DEVICES_PER_CHAIN {
            let original = original_instances
                .get(device_index)
                .and_then(|instance| instance.configuration.as_ref())
                .ok_or_else(|| ProfileError(format!("{} original state is missing", group.name)))?;
            let equal = equal_instances
                .get(device_index)
                .and_then(|instance| instance.configuration.as_ref())
                .ok_or_else(|| ProfileError(format!("{} equal state is missing", group.name)))?;
            let changed = changed_instances
                .get(device_index)
                .and_then(|instance| instance.configuration.as_ref())
                .ok_or_else(|| ProfileError(format!("{} changed state is missing", group.name)))?;
            if Arc::ptr_eq(original, equal) || original.payload() != equal.payload() {
                return Err(ProfileError(format!(
                    "{} separate-equal state is not independently allocated and byte-identical",
                    group.name
                )));
            }
            if Arc::ptr_eq(original, changed)
                || original.state_version() != changed.state_version()
                || original.type_id() != changed.type_id()
                || original.payload().len() != changed.payload().len()
            {
                return Err(ProfileError(format!(
                    "{} changed state identity or metadata is invalid",
                    group.name
                )));
            }
            let original_payload = original.payload();
            let changed_payload = changed.payload();
            let same_prefix = if device_index + 1 == DEVICES_PER_CHAIN {
                original_payload.get(..MAX_NATIVE_DEVICE_STATE_BYTES - 1)
                    == changed_payload.get(..MAX_NATIVE_DEVICE_STATE_BYTES - 1)
            } else {
                original_payload == changed_payload
            };
            let final_byte_changed = device_index + 1 != DEVICES_PER_CHAIN
                || original_payload.last() != changed_payload.last();
            if !same_prefix || !final_byte_changed {
                return Err(ProfileError(format!(
                    "{} changed fixture must differ only in the final byte of the final device",
                    group.name
                )));
            }
        }
        if !old.configuration_matches_range(shared, 0..DEVICES_PER_CHAIN) {
            return Err(ProfileError(format!(
                "{} shared-Arc fixture did not match",
                group.name
            )));
        }
        if !old.configuration_matches_range(separate, 0..DEVICES_PER_CHAIN) {
            return Err(ProfileError(format!(
                "{} separate-equal fixture did not match",
                group.name
            )));
        }
        if old.configuration_matches_range(changed, 0..DEVICES_PER_CHAIN) {
            return Err(ProfileError(format!(
                "{} final-device change unexpectedly matched",
                group.name
            )));
        }
    }
    Ok(())
}

fn profile_group(group: &OwnerGroup, options: Options) -> Result<(), ProfileError> {
    profile_groups(group.name, &[group], options)
}

fn profile_groups(
    name: &'static str,
    groups: &[&OwnerGroup],
    options: Options,
) -> Result<(), ProfileError> {
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    for timings in &mut samples {
        timings
            .try_reserve_exact(options.repeats)
            .map_err(|error| ProfileError(format!("could not reserve timing samples: {error}")))?;
    }

    let total_iterations = options
        .warmups
        .checked_add(options.repeats)
        .ok_or_else(|| ProfileError("iteration count overflowed".into()))?;
    for iteration in 0..total_iterations {
        for offset in 0..Candidate::ALL.len() {
            let candidate_index = (iteration + offset) % Candidate::ALL.len();
            let candidate = Candidate::ALL[candidate_index];
            let started = Instant::now();
            let comparisons = compare_groups(groups, candidate);
            let elapsed = started.elapsed().as_nanos();
            black_box(comparisons);
            if iteration >= options.warmups {
                samples[candidate_index].push(elapsed);
            }
        }
    }

    for (index, candidate) in Candidate::ALL.into_iter().enumerate() {
        let timings = &mut samples[index];
        timings.sort_unstable();
        let median = percentile(timings, 50)
            .ok_or_else(|| ProfileError("no median timing sample was collected".into()))?;
        let p95 = percentile(timings, 95)
            .ok_or_else(|| ProfileError("no p95 timing sample was collected".into()))?;
        let comparisons = groups.iter().try_fold(0_usize, |total, group| {
            total
                .checked_add(group.old.len())
                .ok_or_else(|| ProfileError("comparison count overflowed".into()))
        })?;
        let per_chain_median = median as f64 / comparisons as f64;
        let payload_bytes_per_chain = match candidate {
            Candidate::SharedArc => 0,
            Candidate::SeparateEqual => DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES,
            Candidate::LastDeviceChanged => DEVICES_PER_CHAIN * MAX_NATIVE_DEVICE_STATE_BYTES,
        };
        let payload_bytes_total = payload_bytes_per_chain * comparisons;
        println!(
            "group={} case={} repeats={} warmup={} chains={} devices_per_chain={} comparisons_per_sample={} payload_bytes_per_chain={} payload_bytes_per_sample={} median_ns={} p95_ns={} median_ns_per_chain={:.1}",
            name,
            candidate.label(),
            options.repeats,
            options.warmups,
            comparisons,
            DEVICES_PER_CHAIN,
            comparisons,
            payload_bytes_per_chain,
            payload_bytes_total,
            median,
            p95,
            per_chain_median,
        );
    }
    Ok(())
}

fn compare_groups(groups: &[&OwnerGroup], candidate: Candidate) -> usize {
    let groups = black_box(groups);
    let range = black_box(0..DEVICES_PER_CHAIN);
    let mut comparisons = 0;
    for group in groups {
        let old = black_box(group.old.as_slice());
        let next = black_box(group.candidate(candidate));
        for (previous, candidate) in old.iter().zip(next) {
            black_box(previous.configuration_matches_range(candidate, range.clone()));
            comparisons += 1;
        }
    }
    black_box(comparisons)
}

fn percentile(sorted_samples: &[u128], percentile: usize) -> Option<u128> {
    if sorted_samples.is_empty() || percentile == 0 || percentile > 100 {
        return None;
    }
    let rank = sorted_samples
        .len()
        .checked_mul(percentile)?
        .checked_add(99)?
        / 100;
    sorted_samples.get(rank.saturating_sub(1)).copied()
}
