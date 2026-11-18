use crate::{
    endpoint::{PageBuffer, PageRequest},
    types::{
        CatalogFootprint, PageKey, ScalarSourceDefinition, SourceDefinition, StreamConfig,
        StreamFailure, StreamFailureKind, VerifiedCatalog,
    },
};
use hex_engine::{StreamResourceKey, StreamedSourceMetadata};
use std::collections::VecDeque;
pub(crate) mod granular;
pub(crate) mod scalar;
use hex_media::{ResampleKernel, ResamplePlan, ResampleVersion, VerifiedWavReader};
use hex_warp::{PitchWindow, RepitchWindow, Resampler};
use rtrb::{Consumer, Producer, PushError};
use scalar::ScalarJob;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread,
    time::Duration,
};

/// A child worker failure is a parent shutdown failure even if no PCM loan was
/// active; successful pool counts alone must not erase a failed join.
pub(crate) type WorkerExit = Result<Vec<Box<PageBuffer>>, crate::StreamRuntimeError>;

pub(crate) struct WorkerQueues {
    pub(crate) requests: Consumer<PageRequest>,
    pub(crate) ready: Producer<Box<PageBuffer>>,
    pub(crate) returns: Consumer<Box<PageBuffer>>,
    pub(crate) failures: Producer<StreamFailure>,
}

struct Source {
    definition: SourceDefinition,
    reader: VerifiedWavReader,
    plan: ResamplePlan,
    pitch: Option<PitchWindow>,
    repitch: Option<RepitchWindow>,
}

// 4096 output frames at the maximum 16x pitch plus the 512-tap halo.
const PITCH_SCRATCH_FRAMES: usize = 66_048;

fn pitch_law(law: hex_project::WarpResampler) -> Resampler {
    match law {
        hex_project::WarpResampler::V1 => Resampler::V1,
        hex_project::WarpResampler::V2 => Resampler::V2,
    }
}

pub(crate) fn preparation_data_bound(
    config: StreamConfig,
    definitions: &[SourceDefinition],
    definition_capacity: usize,
) -> Option<(usize, usize)> {
    let count = definitions.len();
    let paths = definitions
        .iter()
        .try_fold(0usize, |n, d| n.checked_add(d.path.capacity()))?;
    if paths > config.max_catalog_path_bytes {
        return None;
    }
    let mut manifests = 0usize;
    let mut kernel_count = 0usize;
    let mut table_reservation = 0usize;
    for (index, definition) in definitions.iter().enumerate() {
        if let Some((_, law)) = definition.key.pitch() {
            table_reservation = table_reservation
                .checked_add(PitchWindow::filter_reservation_bytes(pitch_law(law)))?;
        }
        if let Some((_, map, law)) = &definition.repitch {
            table_reservation =
                table_reservation.checked_add(RepitchWindow::filter_reservation_bytes(*law))?;
            table_reservation = table_reservation.checked_add(map.backing_bytes()?)?;
        }
        let pages = definition
            .reader_limits
            .reader
            .max_file_bytes
            .checked_add(hex_media::VERIFIED_WAV_PAGE_BYTES as u64 - 1)?
            / hex_media::VERIFIED_WAV_PAGE_BYTES as u64;
        let manifest = usize::try_from(pages)
            .ok()?
            .checked_mul(32)?
            .min(definition.reader_limits.max_manifest_bytes);
        manifests = manifests.checked_add(manifest)?;
        let pair = (
            definition.metadata.source_rate(),
            definition.metadata.project_rate(),
            definition.metadata.conversion_version(),
        );
        if !definitions[..index].iter().any(|d| {
            (
                d.metadata.source_rate(),
                d.metadata.project_rate(),
                d.metadata.conversion_version(),
            ) == pair
        }) {
            kernel_count = kernel_count.checked_add(1)?;
            if pair.0 != pair.1 {
                table_reservation =
                    table_reservation.checked_add(ResampleKernel::max_table_payload_bytes())?;
            }
        }
    }
    if table_reservation > config.max_kernel_table_bytes {
        return None;
    }
    let data = crate::footprint::sum([
        count.checked_mul(std::mem::size_of::<Source>())?,
        paths,
        paths,
        manifests.min(config.max_manifest_bytes),
        count.checked_mul(hex_media::VerifiedWavManifest::owner_payload_bytes())?,
        kernel_count.checked_mul(ResampleKernel::owner_payload_bytes())?,
        table_reservation,
        config
            .source_scratch_frames
            .checked_mul(std::mem::size_of::<hex_engine::Frame>())?,
        if definitions
            .iter()
            .any(|d| d.key.pitch().is_some() || d.repitch.is_some())
        {
            PITCH_SCRATCH_FRAMES.checked_mul(std::mem::size_of::<hex_engine::Frame>())?
        } else {
            0
        },
        config
            .page_buffers
            .checked_mul(2)?
            .checked_mul(std::mem::size_of::<Box<PageBuffer>>())?,
        config
            .request_capacity
            .checked_mul(2)?
            .checked_mul(std::mem::size_of::<StreamFailure>())?,
        count.checked_mul(std::mem::size_of::<StreamedSourceMetadata>())?,
        std::mem::size_of::<VerifiedCatalog>(),
    ])?;
    let extra = definition_capacity
        .checked_mul(std::mem::size_of::<SourceDefinition>())?
        .checked_add(count.checked_mul(std::mem::size_of::<(u32, u32, ResampleKernel)>())?)?;
    Some((data, data.checked_add(extra)?))
}

/// The prepared readers/maps already exist before this quote. Their original
/// full decode/checkpoint peak belongs to control preparation, not this worker.
pub(crate) fn preparation_data_bound_with_scalar_sources(
    config: StreamConfig,
    definitions: &[SourceDefinition],
    definition_capacity: usize,
    scalar_definitions: &[ScalarSourceDefinition],
    scalar_capacity: usize,
) -> Option<(usize, usize)> {
    let (base, peak) = preparation_data_bound(config, definitions, definition_capacity)?;
    if scalar_capacity < scalar_definitions.len() {
        return None;
    }
    let mut extra = scalar_capacity
        .checked_mul(std::mem::size_of::<ScalarSourceDefinition>())?
        .checked_add(scalar_definitions.len().checked_mul(
            std::mem::size_of::<hex_engine::ScalarSourceMetadata>()
                + std::mem::size_of::<crate::ScalarSharedData>(),
        )?)?;
    if !scalar_definitions.is_empty() {
        extra = extra
            .checked_add(std::mem::size_of::<Option<ScalarJob>>())?
            .checked_add(
                config
                    .request_capacity
                    .checked_mul(std::mem::size_of::<crate::ResourcePageKey>())?,
            )?
            .checked_add(std::mem::size_of::<VecDeque<crate::ResourcePageKey>>())?;
    }
    let mut paths = definitions
        .iter()
        .try_fold(0usize, |n, d| n.checked_add(d.path.capacity()))?;
    for (index, definition) in scalar_definitions.iter().enumerate() {
        if scalar_definitions[..index]
            .iter()
            .any(|old| old.metadata().key() == definition.metadata().key())
        {
            return None;
        }
        let layout = definition.source.data_layout();
        paths = paths.checked_add(layout.path_capacity_bytes())?;
        extra = extra
            .checked_add(layout.path_capacity_bytes())?
            .checked_add(layout.captured_tempo().checked_backing_bytes()?)?;
        if !scalar_definitions[..index]
            .iter()
            .any(|old| Arc::ptr_eq(&old.projection, &definition.projection))
        {
            extra = extra
                .checked_add(std::mem::size_of::<hex_project::TempoMap>())?
                .checked_add(
                    definition
                        .projection
                        .owned_allocation_layout()
                        .checked_backing_bytes()?,
                )?;
        }
    }
    if paths > config.max_catalog_path_bytes {
        return None;
    }
    Some((base.checked_add(extra)?, peak.checked_add(extra)?))
}

#[cfg(test)]
// Observe actual worker publication/holding without exposing a production API.
// The gate also honors Stop, so a failed test cannot strand a paused worker.
pub(crate) struct TestWorkerHooks {
    pub(crate) read_gate: Option<TestReadGate>,
    pub(crate) entered: SyncSender<()>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
    pub(crate) publications: SyncSender<usize>,
    pub(crate) scalar_steps: Option<(
        SyncSender<hex_engine::ScalarSourceKey>,
        std::sync::mpsc::Receiver<()>,
    )>,
}

#[cfg(test)]
pub(crate) struct TestReadGate {
    pub(crate) key: PageKey,
    pub(crate) after_checks: usize,
    failure_after_page_reads: Option<u32>,
    used: std::cell::Cell<bool>,
    pub(crate) entered: SyncSender<()>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
impl TestReadGate {
    pub(crate) fn new(
        key: PageKey,
        after_checks: usize,
        entered: SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
        failure_after_page_reads: Option<u32>,
    ) -> Self {
        Self {
            key,
            after_checks,
            entered,
            release,
            failure_after_page_reads,
            used: std::cell::Cell::new(false),
        }
    }

    fn wait(&self, stop: &AtomicBool) {
        self.used.set(true);
        if self.entered.try_send(()).is_err() {
            return;
        }
        while !stop.load(Ordering::Acquire) {
            match self.release.recv_timeout(Duration::from_millis(2)) {
                Ok(()) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
impl TestWorkerHooks {
    fn before_scalar_step(&self, key: hex_engine::ScalarSourceKey, stop: &AtomicBool) -> bool {
        let Some((steps, release)) = &self.scalar_steps else {
            return true;
        };
        if steps.try_send(key).is_err() {
            return false;
        }
        while !stop.load(Ordering::Acquire) {
            match release.recv_timeout(Duration::from_millis(2)) {
                Ok(()) => return true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
        false
    }
    fn wait_for_release(&self, stop: &AtomicBool) -> bool {
        if self.entered.send(()).is_err() {
            return false;
        }
        while !stop.load(Ordering::Acquire) {
            match self.release.recv_timeout(Duration::from_millis(2)) {
                Ok(()) => return true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
        false
    }
}

#[allow(clippy::too_many_arguments)] // Fixed source/pool owners plus test-only gates.
pub(crate) fn run_worker(
    config: StreamConfig,
    definitions: Vec<SourceDefinition>,
    mut scalar_definitions: Vec<ScalarSourceDefinition>,
    granular: Option<granular::PreparedGranular>,
    mut free_pages: Vec<Box<PageBuffer>>,
    mut queues: WorkerQueues,
    stop: Arc<AtomicBool>,
    catalog_result: SyncSender<Result<VerifiedCatalog, StreamFailureKind>>,
    offline_consumer: Option<thread::Thread>,
    #[cfg(test)] test_hooks: Option<TestWorkerHooks>,
    #[cfg(feature = "test-support")] mut page_gate: Option<crate::test_support::PageGate>,
) -> WorkerExit {
    let mut source_scratch = Vec::new();
    let scratch_ok = source_scratch
        .try_reserve_exact(config.source_scratch_frames)
        .is_ok();
    if scratch_ok {
        source_scratch.resize(config.source_scratch_frames, [0.0; 2]);
    }
    let mut pitch_scratch = Vec::new();
    let needs_pitch = definitions
        .iter()
        .any(|d| d.key.pitch().is_some() || d.repitch.is_some());
    let pitch_scratch_ok = !needs_pitch
        || (pitch_scratch
            .try_reserve_exact(PITCH_SCRATCH_FRAMES)
            .is_ok()
            && pitch_scratch.capacity() <= PITCH_SCRATCH_FRAMES);
    if needs_pitch && pitch_scratch_ok {
        pitch_scratch.resize(PITCH_SCRATCH_FRAMES, [0.0; 2]);
    }
    let mut held_pages = Vec::new();
    let mut held_failures = Vec::new();
    let held_ok = held_pages.try_reserve_exact(config.page_buffers).is_ok()
        && held_failures
            .try_reserve_exact(config.request_capacity.saturating_mul(2))
            .is_ok();
    let mut deferred_scalar = VecDeque::new();
    let deferred_ok = scalar_definitions.is_empty()
        || deferred_scalar
            .try_reserve_exact(config.request_capacity)
            .is_ok();
    let catalog = if scratch_ok && pitch_scratch_ok && held_ok && deferred_ok {
        build_catalog(
            &config,
            definitions,
            &mut source_scratch,
            pitch_scratch.capacity(),
            &stop,
        )
    } else {
        Err(StreamFailureKind::DecodeFailed)
    };
    scalar_definitions.sort_unstable_by_key(|definition| definition.metadata().key());
    if let Err(kind) = &catalog {
        let _ = catalog_result.send(Err(*kind));
        return Ok(free_pages);
    }
    let mut granular = match granular.map(|prepared| prepared.start(&stop)).transpose() {
        Ok(routing) => routing,
        Err(kind) => {
            let _ = catalog_result.send(Err(kind));
            return Ok(free_pages);
        }
    };
    let catalog_message = catalog.as_ref().map_or_else(
        |error| Err(*error),
        |(sources, footprint, data)| {
            let mut data = *data;
            data.free_page_handles =
                crate::AllocationData::of::<Box<PageBuffer>>(free_pages.capacity());
            data.held_page_handles =
                crate::AllocationData::of::<Box<PageBuffer>>(held_pages.capacity());
            data.held_failures =
                crate::AllocationData::of::<StreamFailure>(held_failures.capacity());
            data.scalar_deferred_keys =
                crate::AllocationData::of::<crate::ResourcePageKey>(deferred_scalar.capacity());
            data.scalar_deferred_inline = if scalar_definitions.is_empty() {
                0
            } else {
                std::mem::size_of::<VecDeque<crate::ResourcePageKey>>()
            };
            data.scalar_source_slots =
                crate::AllocationData::of::<ScalarSourceDefinition>(scalar_definitions.capacity());
            let mut catalog = verified_membership(
                sources,
                &scalar_definitions,
                granular.as_ref().map_or(&[], |routing| routing.metadata()),
                *footprint,
                data,
            )?;
            if let Some(routing) = &granular {
                catalog.data.granular = Some(routing.data());
                catalog.footprint.sources += routing.metadata().len();
                catalog.footprint.granular_data_bytes = routing
                    .data()
                    .bytes()
                    .ok_or(StreamFailureKind::CapacityExhausted)?;
            }
            Ok(catalog)
        },
    );
    if catalog_result.send(catalog_message).is_err() {
        return Ok(free_pages);
    }
    let (mut sources, _footprint, _data) = match catalog {
        Ok(sources) => sources,
        Err(_) => return Ok(free_pages),
    };
    #[cfg(test)]
    if test_hooks
        .as_ref()
        .is_some_and(|hooks| !hooks.wait_for_release(&stop))
    {
        return Ok(free_pages);
    }
    let mut scalar_job: Option<ScalarJob> = None;
    loop {
        for _ in 0..config.return_capacity {
            let Ok(page) = queues.returns.pop() else {
                break;
            };
            free_pages.push(page);
        }
        if let Some(routing) = &mut granular {
            for _ in 0..config.page_buffers {
                let Some((mut page, status)) = routing.poll() else {
                    break;
                };
                match status {
                    Ok(()) => {
                        if let Err(PushError::Full(page)) = queues.ready.push(page) {
                            held_pages.push(page);
                        }
                    }
                    Err(kind) => {
                        if let Some(key) = page.key {
                            hold_failure(
                                StreamFailure {
                                    key: key.source,
                                    page_index: key.page_index,
                                    kind,
                                },
                                &mut held_failures,
                                &mut queues.failures,
                            );
                        }
                        page.key = None;
                        page.valid_frames = 0;
                        free_pages.push(page);
                    }
                }
            }
            if routing.faulted() {
                break;
            }
        }
        flush_held_pages(&mut held_pages, &mut queues.ready);
        flush_held_failures(&mut held_failures, &mut queues.failures);
        if let Some(consumer) = &offline_consumer
            && (queues.ready.slots() < config.ready_capacity
                || queues.failures.slots() < config.failure_capacity)
        {
            // A park token also covers a reply published just before the
            // control owner starts waiting. Live paths never register a waiter.
            consumer.unpark();
        }
        if stop.load(Ordering::Acquire) {
            while let Ok(page) = queues.returns.pop() {
                free_pages.push(page);
            }
            break;
        }
        if !held_pages.is_empty() || !held_failures.is_empty() {
            thread::park_timeout(Duration::from_millis(2));
            continue;
        }
        #[cfg(test)]
        if let Some(job) = &scalar_job
            && let (Some(hooks), StreamResourceKey::Scalar(key)) = (&test_hooks, job.key.source)
            && !hooks.before_scalar_step(key, &stop)
        {
            break;
        }
        if let Some(mut job) = scalar_job.take() {
            let outcome = job.step(&mut scalar_definitions[job.source]);
            match outcome {
                Ok(false) => {
                    scalar_job = Some(job);
                    thread::yield_now();
                }
                Ok(true) => {
                    if let Err(PushError::Full(page)) = queues.ready.push(job.page) {
                        held_pages.push(page);
                    }
                }
                Err(kind) => {
                    job.page.key = None;
                    job.page.valid_frames = 0;
                    free_pages.push(job.page);
                    hold_failure(
                        StreamFailure {
                            key: job.key.source,
                            page_index: job.key.page_index,
                            kind,
                        },
                        &mut held_failures,
                        &mut queues.failures,
                    );
                }
            }
        }
        // One scalar quantum, then at most one request intake. PCM can progress
        // while a dense scalar page remains pending. Other scalar keys retain
        // FIFO order without consuming another page/cursor. Stop wins before I/O.
        if stop.load(Ordering::Acquire) {
            break;
        }
        let request = if scalar_job.is_none() {
            deferred_scalar
                .pop_front()
                .map(PageRequest)
                .or_else(|| queues.requests.pop().ok())
        } else {
            queues.requests.pop().ok()
        };
        let Some(PageRequest(key)) = request else {
            if scalar_job.is_none() {
                thread::park_timeout(Duration::from_millis(2));
            }
            continue;
        };
        #[cfg(feature = "test-support")]
        let read_failure_after = if let Some(gate) = page_gate.take_if(|gate| {
            key.source == StreamResourceKey::Audio(gate.key.source)
                && key.page_index == gate.key.page_index
        }) {
            let failure = gate.read_failure_after;
            if !gate.wait(&stop) {
                break;
            }
            failure
        } else {
            None
        };
        if matches!(key.source, StreamResourceKey::Scalar(_)) && scalar_job.is_some() {
            if deferred_scalar.len() < deferred_scalar.capacity() {
                deferred_scalar.push_back(key);
            } else {
                hold_failure(
                    StreamFailure {
                        key: key.source,
                        page_index: key.page_index,
                        kind: StreamFailureKind::CapacityExhausted,
                    },
                    &mut held_failures,
                    &mut queues.failures,
                );
            }
            continue;
        }
        // A fast consumer can send a returned buffer and its next request
        // after the loop's first drain. Recheck after acquiring the request.
        let Some(mut page) = free_pages.pop().or_else(|| queues.returns.pop().ok()) else {
            let failure = StreamFailure {
                key: key.source,
                page_index: key.page_index,
                kind: StreamFailureKind::CapacityExhausted,
            };
            hold_failure(failure, &mut held_failures, &mut queues.failures);
            continue;
        };
        if let StreamResourceKey::Scalar(scalar_key) = key.source {
            match scalar_definitions
                .binary_search_by_key(&scalar_key, |definition| definition.metadata().key())
            {
                Ok(index) => {
                    match ScalarJob::begin(index, &mut scalar_definitions[index], key, page) {
                        Ok(job) => scalar_job = Some(job),
                        Err((page, kind)) => {
                            free_pages.push(page);
                            hold_failure(
                                StreamFailure {
                                    key: key.source,
                                    page_index: key.page_index,
                                    kind,
                                },
                                &mut held_failures,
                                &mut queues.failures,
                            );
                        }
                    }
                }
                Err(_) => {
                    free_pages.push(page);
                    hold_failure(
                        StreamFailure {
                            key: key.source,
                            page_index: key.page_index,
                            kind: StreamFailureKind::UnknownSource,
                        },
                        &mut held_failures,
                        &mut queues.failures,
                    );
                }
            }
            continue;
        }
        let StreamResourceKey::Audio(audio_key) = key.source else {
            continue;
        };
        if let Some(routing) = &mut granular
            && routing.contains(audio_key)
        {
            if let Err(page) = routing.submit(key, page) {
                free_pages.push(page);
                hold_failure(
                    StreamFailure {
                        key: key.source,
                        page_index: key.page_index,
                        kind: StreamFailureKind::CapacityExhausted,
                    },
                    &mut held_failures,
                    &mut queues.failures,
                );
            }
            continue;
        }
        let Some(source_index) = sources
            .binary_search_by_key(&audio_key, |source| source.definition.key)
            .ok()
        else {
            free_pages.push(page);
            hold_failure(
                StreamFailure {
                    key: key.source,
                    page_index: key.page_index,
                    kind: StreamFailureKind::UnknownSource,
                },
                &mut held_failures,
                &mut queues.failures,
            );
            continue;
        };
        let source = &mut sources[source_index];
        #[cfg(feature = "test-support")]
        if let Some(after) = read_failure_after {
            source.reader.fail_page_read_after(after);
        }
        let rendered = render_page(
            source,
            PageKey {
                source: audio_key,
                page_index: key.page_index,
            },
            &mut page,
            &mut source_scratch,
            &mut pitch_scratch,
            &stop,
            #[cfg(test)]
            test_hooks
                .as_ref()
                .and_then(|hooks| hooks.read_gate.as_ref()),
        );
        #[cfg(feature = "test-support")]
        if read_failure_after.is_some() {
            source.reader.clear_page_read_failure();
        }
        match rendered {
            Ok(()) => {
                match queues.ready.push(page) {
                    Ok(()) => {}
                    Err(PushError::Full(page)) => held_pages.push(page),
                }
                #[cfg(test)]
                if let Some(hooks) = &test_hooks {
                    let _ = hooks.publications.send(held_pages.len());
                }
            }
            Err(kind) => {
                page.frames.fill([0.0; 2]);
                page.key = None;
                page.valid_frames = 0;
                free_pages.push(page);
                hold_failure(
                    StreamFailure {
                        key: key.source,
                        page_index: key.page_index,
                        kind,
                    },
                    &mut held_failures,
                    &mut queues.failures,
                );
            }
        }
    }
    // Stop may arrive during a scalar quantum, after the loop's initial
    // return drain. Reclaim every returned buffer on every loop exit before
    // destroying the worker queue; otherwise the original pair loses custody.
    while let Ok(page) = queues.returns.pop() {
        free_pages.push(page);
    }
    // All final descriptor/kernel and scratch destruction occurs after the
    // worker loop, and the caller joins this thread outside rendering.
    if let Some(job) = scalar_job.take() {
        free_pages.push(job.page);
    }
    if let Some(routing) = granular {
        routing.stop_into(&mut free_pages)?;
    }
    free_pages.append(&mut held_pages);
    drop(sources);
    drop(source_scratch);
    Ok(free_pages)
}

fn verified_membership(
    sources: &[Source],
    scalar_sources: &[ScalarSourceDefinition],
    granular_sources: &[StreamedSourceMetadata],
    mut footprint: CatalogFootprint,
    mut data: crate::WorkerData,
) -> Result<VerifiedCatalog, StreamFailureKind> {
    let mut members = Vec::new();
    members
        .try_reserve_exact(
            sources
                .len()
                .checked_add(granular_sources.len())
                .ok_or(StreamFailureKind::CapacityExhausted)?,
        )
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
    members.extend(
        sources
            .iter()
            .map(|source| source.definition.metadata.clone()),
    );
    members.extend_from_slice(granular_sources);
    members.sort_unstable_by_key(StreamedSourceMetadata::key);
    data.membership = crate::AllocationData::of::<StreamedSourceMetadata>(members.capacity());
    data.catalog_object = std::mem::size_of::<VerifiedCatalog>();
    let mut scalar_members = Vec::new();
    scalar_members
        .try_reserve_exact(scalar_sources.len())
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
    scalar_members.extend(
        scalar_sources
            .iter()
            .map(|source| source.metadata().clone()),
    );
    data.scalar_membership =
        crate::AllocationData::of::<hex_engine::ScalarSourceMetadata>(scalar_members.capacity());
    data.scalar_job_data = if scalar_sources.is_empty() {
        0
    } else {
        std::mem::size_of::<Option<ScalarJob>>()
    };
    let mut shared_rows = Vec::new();
    shared_rows
        .try_reserve_exact(scalar_sources.len())
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
    for (index, source) in scalar_sources.iter().enumerate() {
        let layout = source.source.data_layout();
        shared_rows.push(crate::ScalarSharedData {
            source_identity: layout.source_owner().identity(),
            projection_identity: Arc::as_ptr(&source.projection) as usize,
            projection_data_bytes: std::mem::size_of::<hex_project::TempoMap>()
                .checked_add(
                    source
                        .projection
                        .owned_allocation_layout()
                        .checked_backing_bytes()
                        .ok_or(StreamFailureKind::CapacityExhausted)?,
                )
                .ok_or(StreamFailureKind::CapacityExhausted)?,
            source_handles: 1,
            projection_handles: 1,
        });
        data.scalar_source_backings = data
            .scalar_source_backings
            .checked_add(layout.path_capacity_bytes())
            .and_then(|n| n.checked_add(layout.captured_tempo().checked_backing_bytes()?))
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        footprint.catalog_path_bytes = footprint
            .catalog_path_bytes
            .checked_add(layout.path_capacity_bytes())
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        if !scalar_sources[..index]
            .iter()
            .any(|old| Arc::ptr_eq(&old.projection, &source.projection))
        {
            data.scalar_projection_data = data
                .scalar_projection_data
                .checked_add(std::mem::size_of::<hex_project::TempoMap>())
                .and_then(|n| {
                    n.checked_add(
                        source
                            .projection
                            .owned_allocation_layout()
                            .checked_backing_bytes()?,
                    )
                })
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            data.scalar_projection_groups += 1;
        }
    }
    data.scalar_shared_rows =
        crate::AllocationData::of::<crate::ScalarSharedData>(shared_rows.capacity());
    data.scalar_source_handles = scalar_sources.len();
    // One in-flight cursor holds one additional source and projection handle.
    data.scalar_cursor_handle_bound = if scalar_sources.is_empty() { 0 } else { 2 };
    footprint.sources += scalar_sources.len();
    Ok(VerifiedCatalog {
        scalar_sources: scalar_members,
        scalar_shared_data: shared_rows,
        data,
        footprint,
        sources: members,
    })
}

fn build_catalog(
    config: &StreamConfig,
    mut definitions: Vec<SourceDefinition>,
    source_scratch: &mut Vec<[f32; 2]>,
    pitch_scratch_capacity: usize,
    stop: &AtomicBool,
) -> Result<(Vec<Source>, CatalogFootprint, crate::WorkerData), StreamFailureKind> {
    if definitions
        .iter()
        .any(|d| d.key.pitch().is_some() || d.repitch.is_some())
        && pitch_scratch_capacity < PITCH_SCRATCH_FRAMES
    {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    if definitions.len() > config.max_sources {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    let verified_reader_scratch_bytes = definitions
        .len()
        .checked_mul(hex_media::VERIFIED_WAV_PAGE_BYTES)
        .ok_or(StreamFailureKind::CapacityExhausted)?;
    if verified_reader_scratch_bytes > config.max_reader_scratch_bytes {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    let mut catalog_path_bytes = 0_usize;
    for definition in &definitions {
        catalog_path_bytes = catalog_path_bytes
            .checked_add(definition.path.as_os_str().len())
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        if catalog_path_bytes > config.max_catalog_path_bytes {
            return Err(StreamFailureKind::CapacityExhausted);
        }
    }
    let mut data = crate::WorkerData {
        construction_definition_slots: crate::AllocationData::of::<SourceDefinition>(
            definitions.capacity(),
        ),
        source_scratch: crate::AllocationData::of::<[f32; 2]>(source_scratch.capacity()),
        pitch_scratch: crate::AllocationData::of::<[f32; 2]>(pitch_scratch_capacity),
        ..Default::default()
    };
    definitions.sort_unstable_by_key(|definition| definition.key);
    for pair in definitions.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(StreamFailureKind::UnknownSource);
        }
    }
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(definitions.len())
        .map_err(|_| StreamFailureKind::DecodeFailed)?;
    let mut kernels: Vec<(u32, u32, ResampleKernel)> = Vec::new();
    // Its finite maximum is known before any reader/kernel construction; avoid
    // repeated old backing overlap while a pending Root owns this constructor.
    kernels
        .try_reserve_exact(definitions.len())
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
    let mut manifest_bytes = 0_usize;
    let mut kernel_bytes = 0_usize;
    let mut kernel_budget_reserved = definitions
        .iter()
        .try_fold(0usize, |sum, d| {
            let reservation = d.key.pitch().map_or(0, |(_, law)| {
                PitchWindow::filter_reservation_bytes(pitch_law(law))
            });
            let repitch = if let Some((_, map, law)) = &d.repitch {
                RepitchWindow::filter_reservation_bytes(*law).checked_add(map.backing_bytes()?)?
            } else {
                0
            };
            sum.checked_add(reservation)?.checked_add(repitch)
        })
        .ok_or(StreamFailureKind::CapacityExhausted)?;
    if kernel_budget_reserved > config.max_kernel_table_bytes {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    for mut definition in definitions {
        if stop.load(Ordering::Acquire) {
            return Err(StreamFailureKind::Cancelled);
        }
        if definition.key != definition.metadata.key()
            || match &definition.repitch {
                Some((raw, map, law)) => {
                    raw.with_repitch(map, *law).ok().as_ref() != Some(&definition.metadata)
                }
                None => !definition.key.is_raw() && definition.key.pitch().is_none(),
            }
        {
            return Err(StreamFailureKind::UnknownSource);
        }
        let remaining_manifest = config.max_manifest_bytes.saturating_sub(manifest_bytes);
        let mut reader_limits = definition.reader_limits;
        reader_limits.max_manifest_bytes = reader_limits.max_manifest_bytes.min(remaining_manifest);
        let cancelled = || stop.load(Ordering::Acquire);
        // A caller-captured manifest avoids re-hashing a file whose length and
        // mtime are unchanged; every page served later is still digest-verified.
        // If that identity changed, re-hash in full: the fingerprint check below
        // then accepts a touched-but-identical file and refuses a changed one.
        // Move the supplied handle into the reader. A failed identity reuse
        // must not retain an unused old manifest beside the replacement reader.
        let reused = definition.verified_manifest.take().map(|manifest| {
            VerifiedWavReader::open_verified_with_cancel(
                &definition.path,
                reader_limits,
                manifest,
                cancelled,
            )
        });
        let mut reader = match reused {
            Some(Ok(reader)) => reader,
            None | Some(Err(hex_media::ImportError::SourceChanged)) => {
                VerifiedWavReader::open_with_cancel(&definition.path, reader_limits, cancelled)
                    .map_err(map_import_error)?
            }
            Some(Err(error)) => return Err(map_import_error(error)),
        };
        let reader_manifest = reader.page_manifest_bytes();
        manifest_bytes = manifest_bytes
            .checked_add(reader_manifest)
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        if manifest_bytes > config.max_manifest_bytes {
            return Err(StreamFailureKind::CapacityExhausted);
        }
        if reader.sample_rate() != definition.metadata.source_rate()
            || reader.source_frames() != definition.metadata.source_frames()
            || *reader.fingerprint() != definition.metadata.fingerprint()
        {
            return Err(StreamFailureKind::SourceChanged);
        }
        // The asset's saved conversion version selects the law; v1 media stays v1.
        let version = ResampleVersion::for_conversion(definition.metadata.conversion_version())
            .map_err(map_import_error)?;
        let kernel_index = kernels
            .iter()
            .position(|(source_rate, target_rate, kernel)| {
                *source_rate == reader.sample_rate()
                    && *target_rate == definition.metadata.project_rate()
                    && kernel.version() == version
            });
        let kernel = if let Some(index) = kernel_index {
            kernels[index].2.clone()
        } else {
            // The media kernel constructor caps one immutable table at 13 MiB.
            // Reserve that full possible payload before construction so the
            // aggregate peak, not only the retained actual capacities, is bounded.
            const MAX_KERNEL_BYTES: usize = 13 * 1024 * 1024;
            if reader.sample_rate() != definition.metadata.project_rate() {
                kernel_budget_reserved = kernel_budget_reserved
                    .checked_add(MAX_KERNEL_BYTES)
                    .ok_or(StreamFailureKind::CapacityExhausted)?;
            }
            if kernel_budget_reserved > config.max_kernel_table_bytes {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            let kernel = ResampleKernel::with_version(
                reader.sample_rate(),
                definition.metadata.project_rate(),
                version,
            )
            .map_err(map_import_error)?;
            kernel_bytes = kernel_bytes
                .checked_add(kernel.table_payload_bytes())
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            if kernel_bytes > config.max_kernel_table_bytes {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            let old_capacity = kernels.capacity();
            if kernels.len() == old_capacity {
                data.construction_cache_old_backings = data
                    .construction_cache_old_backings
                    .checked_add(
                        old_capacity
                            .checked_mul(std::mem::size_of::<(u32, u32, ResampleKernel)>())
                            .ok_or(StreamFailureKind::CapacityExhausted)?,
                    )
                    .ok_or(StreamFailureKind::CapacityExhausted)?;
            }
            kernels
                .try_reserve(1)
                .map_err(|_| StreamFailureKind::DecodeFailed)?;
            kernels.push((
                reader.sample_rate(),
                definition.metadata.project_rate(),
                kernel.clone(),
            ));
            kernel
        };
        let plan = ResamplePlan::with_kernel(
            kernel,
            reader.source_frames(),
            reader.source_frames(),
            definition.metadata.source_playback_frames(),
            hex_engine::STREAM_PAGE_FRAMES,
        )
        .map_err(map_import_error)?;
        if plan.output_frames() != definition.metadata.source_playback_frames() {
            return Err(StreamFailureKind::DecodeFailed);
        }
        let window_bound = max_source_window(
            reader.sample_rate(),
            definition.metadata.project_rate(),
            reader.source_frames(),
            plan.kernel_support_radius(),
        )?;
        if window_bound > source_scratch.len() || window_bound > reader.max_window_frames() {
            return Err(StreamFailureKind::CapacityExhausted);
        }
        if reader.encoding() == 3 {
            validate_finite_source(&mut reader, source_scratch, stop)?;
        }
        let pitch = if let Some((cents, law)) = definition.key.pitch() {
            let law = pitch_law(law);
            let pitch = PitchWindow::new(
                definition.metadata.playback_frames(),
                cents,
                law,
                &mut || stop.load(Ordering::Acquire),
            )
            .map_err(map_pitch_error)?;
            let bytes = pitch
                .filter_backing_bytes()
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            if bytes > PitchWindow::filter_reservation_bytes(law) {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            kernel_bytes = kernel_bytes
                .checked_add(bytes)
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            data.pitch_filter_weights = data
                .pitch_filter_weights
                .checked_add(bytes)
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            Some(pitch)
        } else {
            None
        };
        let repitch = if let Some((_, map, law)) = definition.repitch.take() {
            let window = RepitchWindow::new(
                definition.metadata.source_playback_frames(),
                map,
                law,
                &mut || stop.load(Ordering::Acquire),
            )
            .map_err(map_pitch_error)?;
            let filter_bytes = window
                .filter_backing_bytes()
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            if filter_bytes > RepitchWindow::filter_reservation_bytes(law) {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            let bytes = filter_bytes
                .checked_add(
                    window
                        .map_backing_bytes()
                        .ok_or(StreamFailureKind::CapacityExhausted)?,
                )
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            kernel_bytes = kernel_bytes
                .checked_add(bytes)
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            if kernel_bytes > config.max_kernel_table_bytes {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            data.pitch_filter_weights = data
                .pitch_filter_weights
                .checked_add(bytes)
                .ok_or(StreamFailureKind::CapacityExhausted)?;
            Some(window)
        } else {
            None
        };
        sources.push(Source {
            definition,
            reader,
            plan,
            pitch,
            repitch,
        });
    }
    let scratch_bytes = source_scratch
        .capacity()
        .checked_mul(std::mem::size_of::<hex_engine::Frame>())
        .ok_or(StreamFailureKind::CapacityExhausted)?;
    data.source_slots = crate::AllocationData::of::<Source>(sources.capacity());
    data.construction_kernel_cache =
        crate::AllocationData::of::<(u32, u32, ResampleKernel)>(kernels.capacity());
    data.kernel_handles = sources.len();
    data.manifest_handles = sources.len();
    let mut kernel_coverage_known = true;
    for source in &sources {
        let Source {
            definition,
            reader,
            plan,
            pitch: _,
            repitch: _,
        } = source;
        if !kernels
            .iter()
            .any(|(_, _, kernel)| plan.uses_kernel_allocation(kernel))
        {
            kernel_coverage_known = false;
        }
        let SourceDefinition {
            key: _,
            path,
            metadata: _,
            reader_limits: _,
            verified_manifest: _,
            repitch: _,
        } = definition;
        let layout = reader.control_data_layout();
        data.definition_paths = data
            .definition_paths
            .checked_add(path.capacity())
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        data.reader_paths = data
            .reader_paths
            .checked_add(layout.path_capacity)
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        data.manifests = data
            .manifests
            .checked_add(layout.manifest_owner_payload_bytes)
            .and_then(|bytes| {
                bytes.checked_add(
                    layout
                        .manifest_capacity
                        .checked_mul(layout.manifest_element_bytes)?,
                )
            })
            .ok_or(StreamFailureKind::CapacityExhausted)?;
    }
    for (index, (_, _, kernel)) in kernels.iter().enumerate() {
        if kernels[..index]
            .iter()
            .any(|(_, _, old)| old.shares_allocation_with(kernel))
        {
            continue;
        }
        let layout = kernel.control_data_layout();
        data.kernel_objects = data
            .kernel_objects
            .checked_add(layout.owner_payload_bytes)
            .ok_or(StreamFailureKind::CapacityExhausted)?;
        data.kernel_weights = data
            .kernel_weights
            .checked_add(
                layout
                    .weight_capacity
                    .checked_mul(layout.weight_element_bytes)
                    .ok_or(StreamFailureKind::CapacityExhausted)?,
            )
            .ok_or(StreamFailureKind::CapacityExhausted)?;
    }
    data.constructor_complete = kernel_coverage_known;
    let source_count = sources.len();
    Ok((
        sources,
        CatalogFootprint {
            granular_data_bytes: 0,
            sources: source_count,
            catalog_path_bytes,
            manifest_bytes,
            kernel_table_bytes: kernel_bytes,
            kernel_peak_bound_bytes: kernel_budget_reserved,
            verified_reader_scratch_bytes,
            source_scratch_bytes: scratch_bytes,
            pitch_scratch_bytes: data
                .pitch_scratch
                .bytes()
                .ok_or(StreamFailureKind::CapacityExhausted)?,
        },
        data,
    ))
}

fn max_source_window(
    source_rate: u32,
    target_rate: u32,
    source_frames: u64,
    support_radius: usize,
) -> Result<usize, StreamFailureKind> {
    if source_rate == target_rate {
        return usize::try_from(source_frames.min(hex_engine::STREAM_PAGE_FRAMES as u64))
            .map_err(|_| StreamFailureKind::CapacityExhausted);
    }
    let output_span = ((hex_engine::STREAM_PAGE_FRAMES - 1) as u128)
        .checked_mul(u128::from(source_rate))
        .ok_or(StreamFailureKind::CapacityExhausted)?
        .div_ceil(u128::from(target_rate));
    let radius =
        u128::try_from(support_radius).map_err(|_| StreamFailureKind::CapacityExhausted)?;
    let span = output_span
        .checked_add(
            radius
                .checked_mul(2)
                .ok_or(StreamFailureKind::CapacityExhausted)?,
        )
        .and_then(|value| value.checked_add(2))
        .ok_or(StreamFailureKind::CapacityExhausted)?;
    usize::try_from(span.min(u128::from(source_frames)))
        .map_err(|_| StreamFailureKind::CapacityExhausted)
}

pub(crate) fn validate_finite_source(
    reader: &mut VerifiedWavReader,
    scratch: &mut [[f32; 2]],
    stop: &AtomicBool,
) -> Result<(), StreamFailureKind> {
    let limit = reader.max_window_frames().min(scratch.len()).min(16_384);
    if limit == 0 {
        return Err(StreamFailureKind::DecodeFailed);
    }
    let mut start = 0_u64;
    while start < reader.source_frames() {
        if stop.load(Ordering::Acquire) {
            return Err(StreamFailureKind::Cancelled);
        }
        let count = usize::try_from((reader.source_frames() - start).min(limit as u64))
            .map_err(|_| StreamFailureKind::DecodeFailed)?;
        reader
            .read_frames_into_with_cancel(start, &mut scratch[..count], || {
                stop.load(Ordering::Acquire)
            })
            .map_err(map_import_error)?;
        if scratch[..count]
            .iter()
            .flatten()
            .any(|sample| !sample.is_finite())
        {
            return Err(StreamFailureKind::DecodeFailed);
        }
        start = start
            .checked_add(count as u64)
            .ok_or(StreamFailureKind::DecodeFailed)?;
    }
    Ok(())
}

fn render_page(
    source: &mut Source,
    key: PageKey,
    page: &mut PageBuffer,
    source_scratch: &mut [[f32; 2]],
    pitch_scratch: &mut [[f32; 2]],
    stop: &AtomicBool,
    #[cfg(test)] read_gate: Option<&TestReadGate>,
) -> Result<(), StreamFailureKind> {
    let output_start = key
        .page_index
        .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
        .ok_or(StreamFailureKind::DecodeFailed)?;
    let output_frames = source.definition.metadata.playback_frames();
    if output_start >= output_frames {
        return Err(StreamFailureKind::DecodeFailed);
    }
    let count =
        usize::try_from((output_frames - output_start).min(hex_engine::STREAM_PAGE_FRAMES as u64))
            .map_err(|_| StreamFailureKind::DecodeFailed)?;
    if source.pitch.is_some() || source.repitch.is_some() {
        let range = if let Some(pitch) = &source.pitch {
            pitch.required_source_range(output_start, count)
        } else if let Some(repitch) = &source.repitch {
            repitch.required_source_range(output_start, count)
        } else {
            return Err(StreamFailureKind::UnknownSource);
        }
        .map_err(map_pitch_error)?;
        let needed = usize::try_from(range.end - range.start)
            .map_err(|_| StreamFailureKind::CapacityExhausted)?;
        if needed > pitch_scratch.len() {
            return Err(StreamFailureKind::CapacityExhausted);
        }
        page.frames.fill([0.0; 2]);
        for (index, chunk) in pitch_scratch[..needed]
            .chunks_mut(hex_engine::STREAM_PAGE_FRAMES)
            .enumerate()
        {
            let start = range.start + (index * hex_engine::STREAM_PAGE_FRAMES) as u64;
            let (first, last) = source
                .plan
                .required_source_range(start, chunk.len())
                .map_err(map_import_error)?;
            let n =
                usize::try_from(last - first).map_err(|_| StreamFailureKind::CapacityExhausted)?;
            if n > source_scratch.len() || n > source.reader.max_window_frames() {
                return Err(StreamFailureKind::CapacityExhausted);
            }
            #[cfg(test)]
            let read_gate = read_gate.filter(|gate| gate.key == key && !gate.used.get());
            #[cfg(test)]
            if let Some(after) = read_gate.and_then(|gate| gate.failure_after_page_reads) {
                source.reader.fail_page_read_after(after);
            }
            #[cfg(test)]
            let mut checks = 0;
            source
                .reader
                .read_frames_into_with_cancel(first, &mut source_scratch[..n], || {
                    #[cfg(test)]
                    if let Some(gate) = read_gate {
                        checks += 1;
                        if checks == gate.after_checks {
                            gate.wait(stop);
                        }
                    }
                    stop.load(Ordering::Acquire)
                })
                .map_err(map_import_error)?;
            source
                .plan
                .render_window(first, &source_scratch[..n], start, chunk)
                .map_err(map_import_error)?;
        }
        let result = if let Some(pitch) = &source.pitch {
            pitch.render(
                output_start,
                range.start,
                &pitch_scratch[..needed],
                &mut page.frames[..count],
                &mut || stop.load(Ordering::Acquire),
            )
        } else if let Some(repitch) = &mut source.repitch {
            repitch.render(
                output_start,
                range.start,
                &pitch_scratch[..needed],
                &mut page.frames[..count],
                &mut || stop.load(Ordering::Acquire),
            )
        } else {
            return Err(StreamFailureKind::UnknownSource);
        };
        if let Err(error) = result {
            page.frames.fill([0.0; 2]);
            return Err(map_pitch_error(error));
        }
        page.key = Some(key.into());
        page.valid_frames = count;
        return Ok(());
    }
    let (source_start, source_end) = source
        .plan
        .required_source_range(output_start, count)
        .map_err(map_import_error)?;
    let source_count =
        usize::try_from(source_end - source_start).map_err(|_| StreamFailureKind::DecodeFailed)?;
    if source_count > source_scratch.len() || source_count > source.reader.max_window_frames() {
        return Err(StreamFailureKind::CapacityExhausted);
    }
    let input = &mut source_scratch[..source_count];
    #[cfg(test)]
    let read_gate = read_gate.filter(|gate| gate.key == key && !gate.used.get());
    #[cfg(test)]
    if let Some(after) = read_gate.and_then(|gate| gate.failure_after_page_reads) {
        source.reader.fail_page_read_after(after);
    }
    #[cfg(test)]
    let mut checks = 0;
    source
        .reader
        .read_frames_into_with_cancel(source_start, input, || {
            #[cfg(test)]
            if let Some(gate) = read_gate.filter(|gate| gate.key == key) {
                checks += 1;
                if checks == gate.after_checks {
                    gate.wait(stop);
                }
            }
            stop.load(Ordering::Acquire)
        })
        .map_err(map_import_error)?;
    page.frames.fill([0.0; 2]);
    source
        .plan
        .render_window(source_start, input, output_start, &mut page.frames[..count])
        .map_err(map_import_error)?;
    if page.frames[..count]
        .iter()
        .flatten()
        .any(|sample| !sample.is_finite())
    {
        page.frames.fill([0.0; 2]);
        return Err(StreamFailureKind::DecodeFailed);
    }
    page.key = Some(key.into());
    page.valid_frames = count;
    Ok(())
}

fn map_pitch_error(error: hex_warp::Error) -> StreamFailureKind {
    match error {
        hex_warp::Error::Cancelled => StreamFailureKind::Cancelled,
        hex_warp::Error::Allocation
        | hex_warp::Error::WorkLimit
        | hex_warp::Error::ScratchLimit => StreamFailureKind::CapacityExhausted,
        hex_warp::Error::Invalid(_) => StreamFailureKind::DecodeFailed,
    }
}

pub(crate) fn map_import_error(error: hex_media::ImportError) -> StreamFailureKind {
    match error {
        hex_media::ImportError::SourceChanged => StreamFailureKind::SourceChanged,
        hex_media::ImportError::Cancelled => StreamFailureKind::Cancelled,
        hex_media::ImportError::LimitExceeded(_) | hex_media::ImportError::AllocationFailed => {
            StreamFailureKind::CapacityExhausted
        }
        _ => StreamFailureKind::DecodeFailed,
    }
}

fn flush_held_pages(held: &mut Vec<Box<PageBuffer>>, ready: &mut Producer<Box<PageBuffer>>) {
    while let Some(page) = held.pop() {
        match ready.push(page) {
            Ok(()) => {}
            Err(PushError::Full(page)) => {
                held.push(page);
                break;
            }
        }
    }
}

fn flush_held_failures(held: &mut Vec<StreamFailure>, failures: &mut Producer<StreamFailure>) {
    while let Some(failure) = held.pop() {
        match failures.push(failure) {
            Ok(()) => {}
            Err(PushError::Full(failure)) => {
                held.push(failure);
                break;
            }
        }
    }
}

fn hold_failure(
    failure: StreamFailure,
    held: &mut Vec<StreamFailure>,
    queue: &mut Producer<StreamFailure>,
) {
    match queue.push(failure) {
        Ok(()) => {}
        Err(PushError::Full(failure)) => held.push(failure),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
    use hex_media::{VerifiedWavReaderLimits, WavReaderLimits};
    use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Project};
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        path::PathBuf,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    pub(crate) struct Fixture(pub(crate) PathBuf);

    impl Fixture {
        pub(crate) fn pcm16() -> Self {
            Self::pcm16_frames(4096)
        }

        pub(crate) fn pcm16_frames(frames: u16) -> Self {
            Self::pcm_frames(frames, 16)
        }

        pub(crate) fn pcm32_frames(frames: u16) -> Self {
            Self::pcm_frames(frames, 32)
        }

        fn pcm_frames(frames: u16, bits: u16) -> Self {
            assert!(matches!(bits, 16 | 32));
            let block_align = bits / 8 * 2;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hex-stream-cancel-{}-{stamp}-{sequence}.wav",
                std::process::id()
            ));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("create fixture");
            file.write_all(b"RIFF").expect("RIFF");
            file.write_all(&(36_u32 + u32::from(frames) * u32::from(block_align)).to_le_bytes())
                .expect("riff length");
            file.write_all(b"WAVEfmt ").expect("WAVE format");
            file.write_all(&16_u32.to_le_bytes())
                .expect("format length");
            file.write_all(&1_u16.to_le_bytes()).expect("PCM encoding");
            file.write_all(&2_u16.to_le_bytes()).expect("channel count");
            file.write_all(&48_000_u32.to_le_bytes())
                .expect("sample rate");
            file.write_all(&(48_000 * u32::from(block_align)).to_le_bytes())
                .expect("byte rate");
            file.write_all(&block_align.to_le_bytes())
                .expect("block align");
            file.write_all(&bits.to_le_bytes()).expect("bit depth");
            file.write_all(b"data").expect("data header");
            file.write_all(&(u32::from(frames) * u32::from(block_align)).to_le_bytes())
                .expect("data length");
            let mut pcm = Vec::with_capacity(usize::from(frames) * usize::from(block_align));
            for index in 0..i16::try_from(frames).expect("bounded fixture frames") {
                if bits == 16 {
                    pcm.extend_from_slice(&index.to_le_bytes());
                    pcm.extend_from_slice(&(-index).to_le_bytes());
                } else {
                    pcm.extend_from_slice(&(i32::from(index) << 16).to_le_bytes());
                    pcm.extend_from_slice(&(-i32::from(index) << 16).to_le_bytes());
                }
            }
            file.write_all(&pcm).expect("PCM samples");
            file.sync_all().expect("flush fixture");
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn reservation_counts_distinct_conversion_laws_at_the_same_rates() {
        let mut editor = Editor::new(Project::new(44_100).unwrap(), 2).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "conversion.wav".into(),
                sample_rate: 44_100,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset fixture")
        };
        let frames = hex_project::converted_frame_count(4096, 44_100, 48_000).unwrap();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let definitions: Vec<_> = [
            hex_project::SINC_CONVERSION_V1,
            hex_project::SINC_CONVERSION_V2,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, version)| {
            let metadata = StreamedSourceMetadata::new(
                StreamSourceKey::new(index as u64 + 1, asset),
                44_100,
                48_000,
                4096,
                frames,
                version,
                [0; 32],
            )
            .unwrap();
            SourceDefinition::new("conversion.wav".into(), metadata, limits)
        })
        .collect();
        let mut config = StreamConfig::initial();
        config.max_kernel_table_bytes = 2 * ResampleKernel::max_table_payload_bytes() - 1;
        assert!(preparation_data_bound(config, &definitions, definitions.capacity()).is_none());
    }

    #[test]
    fn catalog_refuses_unadmitted_derived_sources_before_opening_a_raw_file() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "missing.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let raw = StreamedSourceMetadata::new(
            StreamSourceKey::new(96, asset),
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let map = hex_warp::TimeMap::new(
            vec![hex_warp::MapSegment {
                output_start: 0,
                output_end: 4096,
                source_start_q32: 0,
                source_end_q32: 4096_i128 << 32,
            }],
            4096,
        )
        .unwrap();
        let identity = hex_engine::RepitchStreamIdentity::new(&raw, &map, Resampler::V2).unwrap();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let region = hex_warp::SourceRegion::new(4096, 0, 4096, 0, false, vec![]).unwrap();
        let granular = raw
            .with_granular(
                &region,
                &map,
                hex_warp::Mode::Tones { grain_ms: 30 },
                0,
                Resampler::V2,
            )
            .unwrap();
        let mut definition = SourceDefinition::new("missing.wav".into(), raw, limits);
        definition.key = StreamSourceKey::repitch(identity);
        let config = StreamConfig::initial();
        let mut scratch = vec![[0.0; 2]; config.source_scratch_frames];
        assert!(matches!(
            build_catalog(
                &config,
                vec![definition],
                &mut scratch,
                0,
                &AtomicBool::new(false)
            ),
            Err(StreamFailureKind::UnknownSource)
        ));
        let granular_definition = SourceDefinition::new("missing.wav".into(), granular, limits);
        assert!(matches!(
            build_catalog(
                &config,
                vec![granular_definition],
                &mut scratch,
                0,
                &AtomicBool::new(false)
            ),
            Err(StreamFailureKind::UnknownSource)
        ));
    }

    #[test]
    fn repitch_pages_match_resident_with_converted_source_and_distinct_extent() {
        for (project_rate, segmented) in [
            (48_000, false),
            (44_100, false),
            (48_000, true),
            (44_100, true),
        ] {
            let fixture = Fixture::pcm16();
            let limits = VerifiedWavReaderLimits::new(
                WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
                1024 * 1024,
            );
            let reader = VerifiedWavReader::open(&fixture.0, limits).unwrap();
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
            let Created::Asset(asset) = editor
                .execute(Command::AddAsset {
                    path: fixture.0.clone(),
                    sample_rate: 48_000,
                    frames: 4096,
                })
                .unwrap()
            else {
                panic!("asset fixture")
            };
            let input_frames =
                hex_project::converted_frame_count(4096, 48_000, project_rate).unwrap();
            let version = if project_rate == 48_000 {
                IDENTITY_CONVERSION
            } else {
                hex_project::SINC_CONVERSION_V2
            };
            let raw = StreamedSourceMetadata::new(
                StreamSourceKey::new(96, asset),
                48_000,
                project_rate,
                4096,
                input_frames,
                version,
                *reader.fingerprint(),
            )
            .unwrap();
            let output_frames = input_frames * 2;
            let segments = if segmented {
                let a = output_frames / 4;
                let b = output_frames / 2;
                let first = i128::from(input_frames) * (1_i128 << 32) / 4;
                let second = i128::from(input_frames) * 5 * (1_i128 << 32) / 8;
                vec![
                    hex_warp::MapSegment {
                        output_start: 0,
                        output_end: a,
                        source_start_q32: 0,
                        source_end_q32: first,
                    },
                    hex_warp::MapSegment {
                        output_start: a,
                        output_end: b,
                        source_start_q32: first,
                        source_end_q32: second,
                    },
                    hex_warp::MapSegment {
                        output_start: b,
                        output_end: output_frames,
                        source_start_q32: second,
                        source_end_q32: i128::from(input_frames) << 32,
                    },
                ]
            } else {
                vec![hex_warp::MapSegment {
                    output_start: 0,
                    output_end: output_frames,
                    source_start_q32: 0,
                    source_end_q32: i128::from(input_frames) << 32,
                }]
            };
            let map = hex_warp::TimeMap::new(segments, output_frames as usize).unwrap();
            let definition = SourceDefinition::new(fixture.0.clone(), raw, limits)
                .with_repitch(map.clone(), Resampler::V2)
                .unwrap();
            let key = definition.key;
            let config = StreamConfig::initial();
            let (bound, _) =
                preparation_data_bound(config, std::slice::from_ref(&definition), 1).unwrap();
            let mut source_scratch = vec![[0.0; 2]; config.source_scratch_frames];
            let mut pitch_scratch = vec![[0.0; 2]; PITCH_SCRATCH_FRAMES];
            let mut wrong_identity = definition.clone();
            wrong_identity.repitch.as_mut().unwrap().2 = Resampler::V1;
            assert!(matches!(
                build_catalog(
                    &config,
                    vec![wrong_identity],
                    &mut source_scratch,
                    pitch_scratch.len(),
                    &AtomicBool::new(false)
                ),
                Err(StreamFailureKind::UnknownSource)
            ));
            let mut too_small = config;
            too_small.max_kernel_table_bytes = 0;
            assert!(
                preparation_data_bound(too_small, std::slice::from_ref(&definition), 1).is_none()
            );
            assert!(matches!(
                build_catalog(
                    &too_small,
                    vec![definition.clone()],
                    &mut source_scratch,
                    pitch_scratch.len(),
                    &AtomicBool::new(false)
                ),
                Err(StreamFailureKind::CapacityExhausted)
            ));
            let (mut sources, _, data) = build_catalog(
                &config,
                vec![definition],
                &mut source_scratch,
                pitch_scratch.len(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(data.bytes().unwrap() <= bound);
            let source = &mut sources[0];
            let mut native = vec![[0.0; 2]; 4096];
            source
                .reader
                .read_frames_into_with_cancel(0, &mut native, || false)
                .unwrap();
            let mut converted = vec![[0.0; 2]; input_frames as usize];
            source
                .plan
                .render_window(0, &native, 0, &mut converted)
                .unwrap();
            let request = hex_warp::Request {
                attacks: &[],
                input: hex_warp::Input {
                    frames: &converted,
                    sample_rate: project_rate,
                    region_start: 0,
                    region_length: input_frames,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames: output_frames as usize,
                pitch_cents: 0,
                mode: hex_warp::Mode::RePitch,
            };
            let expected =
                hex_warp::render_with_resampler(&request, Resampler::V2, &mut || false).unwrap();
            for page_index in [1, 0] {
                let mut page = PageBuffer::new().unwrap();
                render_page(
                    source,
                    PageKey {
                        source: key,
                        page_index,
                    },
                    &mut page,
                    &mut source_scratch,
                    &mut pitch_scratch,
                    &AtomicBool::new(false),
                    None,
                )
                .unwrap();
                let start = page_index as usize * hex_engine::STREAM_PAGE_FRAMES;
                assert_eq!(
                    &page.frames[..page.valid_frames],
                    &expected.frames[start..start + page.valid_frames]
                );
            }
        }
    }

    #[test]
    fn pitched_worker_data_is_covered_by_its_preconstruction_bound() {
        let fixture = Fixture::pcm16();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(&fixture.0, limits).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: fixture.0.clone(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset fixture")
        };
        let raw = StreamedSourceMetadata::new(
            StreamSourceKey::new(96, asset),
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        let pitched = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        let definition = SourceDefinition::new(fixture.0.clone(), pitched, limits);
        let config = StreamConfig::initial();
        let (bound, _) =
            preparation_data_bound(config, std::slice::from_ref(&definition), 1).unwrap();
        let mut source_scratch = vec![[0.0; 2]; config.source_scratch_frames];
        let pitch_scratch = vec![[0.0; 2]; PITCH_SCRATCH_FRAMES];
        let (_, footprint, data) = build_catalog(
            &config,
            vec![definition],
            &mut source_scratch,
            pitch_scratch.capacity(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            data.pitch_scratch.bytes(),
            Some(PITCH_SCRATCH_FRAMES * std::mem::size_of::<[f32; 2]>())
        );
        assert_eq!(
            data.pitch_filter_weights,
            PitchWindow::filter_reservation_bytes(Resampler::V2)
        );
        assert_eq!(
            footprint.pitch_scratch_bytes,
            data.pitch_scratch.bytes().unwrap()
        );
        assert!(data.bytes().unwrap() <= bound);
    }

    #[test]
    fn cancelled_page_read_returns_a_typed_failure_without_marking_the_page_ready() {
        let fixture = Fixture::pcm16();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(&fixture.0, limits).expect("verified source");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let asset = match editor
            .execute(Command::AddAsset {
                path: "cancel-source.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .expect("asset")
        {
            Created::Asset(asset) => asset,
            other => panic!("unexpected asset result: {other:?}"),
        };
        let key = StreamSourceKey::new(93, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .expect("source metadata");
        let kernel = ResampleKernel::new(48_000, 48_000).expect("identity kernel");
        let plan =
            ResamplePlan::with_kernel(kernel, 4096, 4096, 4096, hex_engine::STREAM_PAGE_FRAMES)
                .expect("identity plan");
        let mut source = Source {
            definition: SourceDefinition::new(fixture.0.clone(), metadata, limits),
            reader,
            plan,
            pitch: None,
            repitch: None,
        };
        let key = PageKey {
            source: key,
            page_index: 0,
        };
        let mut page = PageBuffer::new().expect("page buffer");
        let mut scratch = vec![[0.0; 2]; 8192];
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            render_page(
                &mut source,
                key,
                &mut page,
                &mut scratch,
                &mut [],
                &cancelled,
                None
            ),
            Err(StreamFailureKind::Cancelled),
        );
        assert_eq!(page.key, None);
        assert_eq!(page.valid_frames, 0);
        assert!(page.frames.iter().all(|frame| *frame == [0.0; 2]));
    }

    #[test]
    fn full_ready_queue_retains_page_ownership_until_drain() {
        let source = StreamSourceKey::new(94, {
            let mut editor =
                Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
            match editor
                .execute(Command::AddAsset {
                    path: "ready-queue-test.wav".into(),
                    sample_rate: 48_000,
                    frames: 4096,
                })
                .expect("asset")
            {
                Created::Asset(asset) => asset,
                other => panic!("unexpected asset result: {other:?}"),
            }
        });
        let make_page = |index| {
            let mut page = PageBuffer::new().expect("page buffer");
            page.key = Some(
                PageKey {
                    source,
                    page_index: index,
                }
                .into(),
            );
            page.valid_frames = 1;
            page.frames[0] = [index as f32, -(index as f32)];
            page
        };
        let (mut ready, mut ready_rx) = rtrb::RingBuffer::new(1);
        assert!(ready.push(make_page(1)).is_ok());
        let mut held = vec![make_page(2), make_page(3)];
        let held_capacity = held.capacity();
        flush_held_pages(&mut held, &mut ready);
        assert_eq!(held.len(), 2);
        assert_eq!(held.capacity(), held_capacity);
        let queued = ready_rx.pop().expect("sentinel page");
        assert_eq!(
            queued.key,
            Some(
                PageKey {
                    source,
                    page_index: 1
                }
                .into()
            )
        );

        flush_held_pages(&mut held, &mut ready);
        assert_eq!(held.len(), 1);
        assert_eq!(held.capacity(), held_capacity);
        let first_recovered = ready_rx.pop().expect("first retained page");
        flush_held_pages(&mut held, &mut ready);
        assert!(held.is_empty());
        assert_eq!(held.capacity(), held_capacity);
        let second_recovered = ready_rx.pop().expect("second retained page");
        let mut recovered = [first_recovered, second_recovered];
        recovered.sort_by_key(|page| page.key.map(|key| key.page_index));
        assert_eq!(
            recovered[0].key,
            Some(
                PageKey {
                    source,
                    page_index: 2
                }
                .into()
            )
        );
        assert_eq!(
            recovered[1].key,
            Some(
                PageKey {
                    source,
                    page_index: 3
                }
                .into()
            )
        );
        assert_eq!(recovered[0].frames[0], [2.0, -2.0]);
        assert_eq!(recovered[1].frames[0], [3.0, -3.0]);
        assert!(ready_rx.pop().is_err());
    }

    #[test]
    fn full_failure_queue_retains_copy_events_until_drain() {
        let key = StreamSourceKey::new(95, {
            let mut editor =
                Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
            match editor
                .execute(Command::AddAsset {
                    path: "failure-queue-test.wav".into(),
                    sample_rate: 48_000,
                    frames: 4096,
                })
                .expect("asset")
            {
                Created::Asset(asset) => asset,
                other => panic!("unexpected asset result: {other:?}"),
            }
        });
        let failures = [
            StreamFailure {
                key: key.into(),
                page_index: 1,
                kind: StreamFailureKind::SourceChanged,
            },
            StreamFailure {
                key: key.into(),
                page_index: 2,
                kind: StreamFailureKind::Cancelled,
            },
            StreamFailure {
                key: key.into(),
                page_index: 3,
                kind: StreamFailureKind::CapacityExhausted,
            },
        ];
        let (mut queue, mut queue_rx) = rtrb::RingBuffer::new(1);
        assert!(queue.push(failures[0]).is_ok());
        let mut held = vec![failures[1], failures[2]];
        let held_capacity = held.capacity();
        flush_held_failures(&mut held, &mut queue);
        assert_eq!(held, failures[1..]);
        assert_eq!(held.capacity(), held_capacity);
        assert_eq!(queue_rx.pop(), Ok(failures[0]));

        flush_held_failures(&mut held, &mut queue);
        assert_eq!(held.len(), 1);
        assert_eq!(held.capacity(), held_capacity);
        let first_recovered = queue_rx.pop().expect("first retained event");
        flush_held_failures(&mut held, &mut queue);
        assert!(held.is_empty());
        assert_eq!(held.capacity(), held_capacity);
        let second_recovered = queue_rx.pop().expect("second retained event");
        let mut recovered = [first_recovered, second_recovered];
        recovered.sort_by_key(|failure| failure.page_index);
        assert_eq!(recovered, failures[1..]);
        assert!(queue_rx.pop().is_err());
    }

    /// Catalog rebuild read cost: a pre-verified manifest reopens the source
    /// from its header page; a changed file is refused rather than re-hashed.
    #[test]
    fn catalog_rebuild_reuses_a_captured_manifest_and_refuses_a_changed_file() {
        const FRAMES: u16 = 32_767; // The fixture maximum: three raw pages.
        let fixture = Fixture::pcm16_frames(FRAMES);
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, u64::from(FRAMES), 128, 8192),
            1024 * 1024,
        );
        let verified = VerifiedWavReader::open(&fixture.0, limits).expect("verified source");
        // Three hashed pages, then the verified header scan re-reads page 0.
        assert_eq!(verified.raw_page_reads(), 4);
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "rebuild-source.wav".into(),
                sample_rate: 48_000,
                frames: u64::from(FRAMES),
            })
            .expect("asset")
        else {
            panic!("asset fixture");
        };
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(95, asset),
            48_000,
            48_000,
            u64::from(FRAMES),
            u64::from(FRAMES),
            IDENTITY_CONVERSION,
            *verified.fingerprint(),
        )
        .expect("source metadata");
        let definition = || {
            SourceDefinition::new(fixture.0.clone(), metadata.clone(), limits)
                .with_verified_manifest(verified.manifest().clone())
        };
        let config = StreamConfig::initial();
        let mut scratch = vec![[0.0; 2]; config.source_scratch_frames];
        let stop = AtomicBool::new(false);
        let (sources, _, data) =
            build_catalog(&config, vec![definition()], &mut scratch, 0, &stop).expect("rebuild");
        let manifest_data =
            verified.page_manifest_bytes() + hex_media::VerifiedWavManifest::owner_payload_bytes();
        assert_eq!(data.manifests, manifest_data);
        assert_eq!(
            sources[0].reader.raw_page_reads(),
            1,
            "an unchanged pre-verified source reads only its header page"
        );
        // Two readers may share immutable manifest DATA; reserve it per reader
        // rather than omitting the sized owner or inferring exclusive custody.
        let mut second = definition();
        second.metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(96, asset),
            48_000,
            48_000,
            u64::from(FRAMES),
            u64::from(FRAMES),
            IDENTITY_CONVERSION,
            *verified.fingerprint(),
        )
        .unwrap();
        second.key = second.metadata.key();
        let definitions = vec![definition(), second];
        let mut shared_config = config;
        shared_config.max_manifest_bytes = 2 * verified.page_manifest_bytes();
        let mut short = shared_config;
        short.max_manifest_bytes -= 1;
        assert!(matches!(
            build_catalog(&short, definitions.clone(), &mut scratch, 0, &stop),
            Err(StreamFailureKind::CapacityExhausted)
        ));
        let (bound, _) =
            preparation_data_bound(shared_config, &definitions, definitions.capacity()).unwrap();
        let (shared, footprint, data) =
            build_catalog(&shared_config, definitions, &mut scratch, 0, &stop).unwrap();
        assert_eq!(data.manifests, 2 * manifest_data);
        assert_eq!(footprint.manifest_bytes, 2 * verified.page_manifest_bytes());
        assert!(std::ptr::eq(
            shared[0].reader.page_digests().as_ptr(),
            shared[1].reader.page_digests().as_ptr(),
        ));
        assert!(data.bytes().unwrap() <= bound);
        drop(shared);
        // Without a manifest the same rebuild hashes every page.
        let (sources, _, _) = build_catalog(
            &config,
            vec![SourceDefinition::new(
                fixture.0.clone(),
                metadata.clone(),
                limits,
            )],
            &mut scratch,
            0,
            &stop,
        )
        .expect("full rebuild");
        assert_eq!(sources[0].reader.raw_page_reads(), 4);
        drop(sources);
        // A touched but byte-identical file fails the identity check and is
        // re-hashed in full, which still matches the expected fingerprint.
        let mut file = OpenOptions::new()
            .write(true)
            .open(&fixture.0)
            .expect("open for mutation");
        let modified = file.metadata().unwrap().modified().unwrap();
        let touch = |file: &fs::File, seconds| {
            file.set_times(
                std::fs::FileTimes::new()
                    .set_modified(modified + std::time::Duration::from_secs(seconds)),
            )
            .unwrap()
        };
        touch(&file, 5);
        let (sources, _, _) = build_catalog(&config, vec![definition()], &mut scratch, 0, &stop)
            .expect("touched rebuild");
        assert_eq!(sources[0].reader.raw_page_reads(), 4);
        drop(sources);
        // A content change on disk is refused.
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(100)).unwrap();
        file.write_all(&[0x55, 0x55]).unwrap();
        touch(&file, 10);
        assert!(matches!(
            build_catalog(&config, vec![definition()], &mut scratch, 0, &stop),
            Err(StreamFailureKind::SourceChanged)
        ));
    }
}
