use crate::{
    endpoint::{PageBuffer, PageRequest},
    types::{
        CatalogFootprint, PageKey, SourceDefinition, StreamConfig, StreamFailure,
        StreamFailureKind, VerifiedCatalog,
    },
};
use hex_engine::StreamedSourceMetadata;
use hex_media::{ResampleKernel, ResamplePlan, VerifiedWavReader};
use rtrb::{Consumer, Producer, PushError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread,
    time::Duration,
};

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
}

#[cfg(test)]
// Observe actual worker publication/holding without exposing a production API.
// The gate also honors Stop, so a failed test cannot strand a paused worker.
pub(crate) struct TestWorkerHooks {
    pub(crate) entered: SyncSender<()>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
    pub(crate) publications: SyncSender<usize>,
}

#[cfg(test)]
impl TestWorkerHooks {
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

pub(crate) fn run_worker(
    config: StreamConfig,
    definitions: Vec<SourceDefinition>,
    mut free_pages: Vec<Box<PageBuffer>>,
    mut queues: WorkerQueues,
    stop: Arc<AtomicBool>,
    catalog_result: SyncSender<Result<VerifiedCatalog, StreamFailureKind>>,
    #[cfg(test)] test_hooks: Option<TestWorkerHooks>,
) -> Vec<Box<PageBuffer>> {
    let mut source_scratch = Vec::new();
    let scratch_ok = source_scratch
        .try_reserve_exact(config.source_scratch_frames)
        .is_ok();
    if scratch_ok {
        source_scratch.resize(config.source_scratch_frames, [0.0; 2]);
    }
    let mut held_pages = Vec::new();
    let mut held_failures = Vec::new();
    let held_ok = held_pages.try_reserve_exact(config.page_buffers).is_ok()
        && held_failures
            .try_reserve_exact(config.request_capacity.saturating_mul(2))
            .is_ok();
    let catalog = if scratch_ok && held_ok {
        build_catalog(&config, definitions, &mut source_scratch, &stop)
    } else {
        Err(StreamFailureKind::DecodeFailed)
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
            verified_membership(sources, *footprint, data)
        },
    );
    if catalog_result.send(catalog_message).is_err() {
        return free_pages;
    }
    let (mut sources, _footprint, _data) = match catalog {
        Ok(sources) => sources,
        Err(_) => return free_pages,
    };
    #[cfg(test)]
    if test_hooks
        .as_ref()
        .is_some_and(|hooks| !hooks.wait_for_release(&stop))
    {
        return free_pages;
    }
    loop {
        while let Ok(page) = queues.returns.pop() {
            free_pages.push(page);
        }
        flush_held_pages(&mut held_pages, &mut queues.ready);
        flush_held_failures(&mut held_failures, &mut queues.failures);
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
        let Ok(request) = queues.requests.pop() else {
            thread::park_timeout(Duration::from_millis(2));
            continue;
        };
        let PageRequest(key) = request;
        let Some(mut page) = free_pages.pop() else {
            // No source buffer is available until the endpoint returns an
            // evicted/stale page. Preserve the request without an allocation.
            let failure = StreamFailure {
                key: key.source,
                page_index: key.page_index,
                kind: StreamFailureKind::CapacityExhausted,
            };
            hold_failure(failure, &mut held_failures, &mut queues.failures);
            continue;
        };
        let Some(source_index) = sources
            .binary_search_by_key(&key.source, |source| source.definition.key)
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
        match render_page(source, key, &mut page, &mut source_scratch, &stop) {
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
    // All final descriptor/kernel and scratch destruction occurs after the
    // worker loop, and the caller joins this thread outside rendering.
    free_pages.append(&mut held_pages);
    drop(sources);
    drop(source_scratch);
    free_pages
}

fn verified_membership(
    sources: &[Source],
    footprint: CatalogFootprint,
    mut data: crate::WorkerData,
) -> Result<VerifiedCatalog, StreamFailureKind> {
    let mut members = Vec::new();
    members
        .try_reserve_exact(sources.len())
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
    members.extend(
        sources
            .iter()
            .map(|source| source.definition.metadata.clone()),
    );
    members.sort_unstable_by_key(StreamedSourceMetadata::key);
    data.membership = crate::AllocationData::of::<StreamedSourceMetadata>(members.capacity());
    data.catalog_object = std::mem::size_of::<VerifiedCatalog>();
    Ok(VerifiedCatalog {
        data,
        footprint,
        sources: members,
    })
}

fn build_catalog(
    config: &StreamConfig,
    mut definitions: Vec<SourceDefinition>,
    source_scratch: &mut Vec<[f32; 2]>,
    stop: &AtomicBool,
) -> Result<(Vec<Source>, CatalogFootprint, crate::WorkerData), StreamFailureKind> {
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
    let mut manifest_bytes = 0_usize;
    let mut kernel_bytes = 0_usize;
    let mut kernel_budget_reserved = 0_usize;
    for definition in definitions {
        if stop.load(Ordering::Acquire) {
            return Err(StreamFailureKind::Cancelled);
        }
        if definition.key != definition.metadata.key() {
            return Err(StreamFailureKind::UnknownSource);
        }
        let remaining_manifest = config.max_manifest_bytes.saturating_sub(manifest_bytes);
        let mut reader_limits = definition.reader_limits;
        reader_limits.max_manifest_bytes = reader_limits.max_manifest_bytes.min(remaining_manifest);
        let mut reader =
            VerifiedWavReader::open_with_cancel(&definition.path, reader_limits, || {
                stop.load(Ordering::Acquire)
            })
            .map_err(map_import_error)?;
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
        let kernel_index = kernels.iter().position(|(source_rate, target_rate, _)| {
            *source_rate == reader.sample_rate()
                && *target_rate == definition.metadata.project_rate()
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
            let kernel =
                ResampleKernel::new(reader.sample_rate(), definition.metadata.project_rate())
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
            definition.metadata.playback_frames(),
            hex_engine::STREAM_PAGE_FRAMES,
        )
        .map_err(map_import_error)?;
        if plan.output_frames() != definition.metadata.playback_frames() {
            return Err(StreamFailureKind::DecodeFailed);
        }
        let window_bound = max_source_window(
            reader.sample_rate(),
            definition.metadata.project_rate(),
            reader.source_frames(),
        )?;
        if window_bound > source_scratch.len() || window_bound > reader.max_window_frames() {
            return Err(StreamFailureKind::CapacityExhausted);
        }
        if reader.encoding() == 3 {
            validate_finite_source(&mut reader, source_scratch, stop)?;
        }
        sources.push(Source {
            definition,
            reader,
            plan,
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
    let mut kernel_coverage_known = true;
    for source in &sources {
        let Source {
            definition,
            reader,
            plan,
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
            .checked_add(
                layout
                    .manifest_capacity
                    .checked_mul(layout.manifest_element_bytes)
                    .ok_or(StreamFailureKind::CapacityExhausted)?,
            )
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
            sources: source_count,
            catalog_path_bytes,
            manifest_bytes,
            kernel_table_bytes: kernel_bytes,
            kernel_peak_bound_bytes: kernel_budget_reserved,
            verified_reader_scratch_bytes,
            source_scratch_bytes: scratch_bytes,
        },
        data,
    ))
}

fn max_source_window(
    source_rate: u32,
    target_rate: u32,
    source_frames: u64,
) -> Result<usize, StreamFailureKind> {
    if source_rate == target_rate {
        return usize::try_from(source_frames.min(hex_engine::STREAM_PAGE_FRAMES as u64))
            .map_err(|_| StreamFailureKind::CapacityExhausted);
    }
    let output_span = ((hex_engine::STREAM_PAGE_FRAMES - 1) as u128)
        .checked_mul(u128::from(source_rate))
        .ok_or(StreamFailureKind::CapacityExhausted)?
        .div_ceil(u128::from(target_rate));
    let radius = if target_rate >= source_rate {
        32_u128
    } else {
        32_u128
            .checked_mul(u128::from(source_rate))
            .ok_or(StreamFailureKind::CapacityExhausted)?
            .div_ceil(u128::from(target_rate))
    };
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

fn validate_finite_source(
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
    stop: &AtomicBool,
) -> Result<(), StreamFailureKind> {
    let output_start = key
        .page_index
        .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
        .ok_or(StreamFailureKind::DecodeFailed)?;
    if output_start >= source.plan.output_frames() {
        return Err(StreamFailureKind::DecodeFailed);
    }
    let count = usize::try_from(
        (source.plan.output_frames() - output_start).min(hex_engine::STREAM_PAGE_FRAMES as u64),
    )
    .map_err(|_| StreamFailureKind::DecodeFailed)?;
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
    source
        .reader
        .read_frames_into_with_cancel(source_start, input, || stop.load(Ordering::Acquire))
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
    page.key = Some(key);
    page.valid_frames = count;
    Ok(())
}

fn map_import_error(error: hex_media::ImportError) -> StreamFailureKind {
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
            file.write_all(&(36_u32 + u32::from(frames) * 4).to_le_bytes())
                .expect("riff length");
            file.write_all(b"WAVEfmt ").expect("WAVE format");
            file.write_all(&16_u32.to_le_bytes())
                .expect("format length");
            file.write_all(&1_u16.to_le_bytes()).expect("PCM encoding");
            file.write_all(&2_u16.to_le_bytes()).expect("channel count");
            file.write_all(&48_000_u32.to_le_bytes())
                .expect("sample rate");
            file.write_all(&192_000_u32.to_le_bytes())
                .expect("byte rate");
            file.write_all(&4_u16.to_le_bytes()).expect("block align");
            file.write_all(&16_u16.to_le_bytes()).expect("bit depth");
            file.write_all(b"data").expect("data header");
            file.write_all(&(u32::from(frames) * 4).to_le_bytes())
                .expect("data length");
            for index in 0..i16::try_from(frames).expect("bounded fixture frames") {
                file.write_all(&index.to_le_bytes()).expect("left sample");
                file.write_all(&(-index).to_le_bytes())
                    .expect("right sample");
            }
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
        };
        let key = PageKey {
            source: key,
            page_index: 0,
        };
        let mut page = PageBuffer::new().expect("page buffer");
        let mut scratch = vec![[0.0; 2]; 8192];
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            render_page(&mut source, key, &mut page, &mut scratch, &cancelled),
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
            page.key = Some(PageKey {
                source,
                page_index: index,
            });
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
            Some(PageKey {
                source,
                page_index: 1
            })
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
            Some(PageKey {
                source,
                page_index: 2
            })
        );
        assert_eq!(
            recovered[1].key,
            Some(PageKey {
                source,
                page_index: 3
            })
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
                key,
                page_index: 1,
                kind: StreamFailureKind::SourceChanged,
            },
            StreamFailure {
                key,
                page_index: 2,
                kind: StreamFailureKind::Cancelled,
            },
            StreamFailure {
                key,
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
}
