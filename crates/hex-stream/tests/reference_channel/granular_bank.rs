//! Exact source dispatch over one dedicated worker and retained page custody.
use super::*;
use hex_engine::{STREAM_PAGE_FRAMES, audio_transform::streamed_granular_root};
use hex_project::{WarpMode, default_audio_warp};
use hex_stream::{
    GranularBankAllowance, GranularBankError, GranularSourceBuffers, GranularSourceError,
    GranularSourcePreparation, GranularSourcePreparationLimits, GranularWorkerBank,
    GranularWorkerError, GranularWorkerExitReason, GranularWorkerKey, GranularWorkerQueues,
    GranularWorkerRequest, GranularWorkerResult, GranularWorkerSeek, VerifiedGranularSource,
    run_granular_worker,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) fn admit(sources: Vec<VerifiedGranularSource>) -> GranularWorkerBank {
    GranularWorkerBank::try_new_on_control(
        sources,
        GranularBankAllowance {
            max_sources: 512,
            max_data_bytes: 64 * 1024 * 1024,
        },
    )
    .unwrap()
}
fn source(mode: WarpMode) -> (Wav, VerifiedGranularSource, Vec<[f32; 2]>) {
    source_at_rate(mode, 48_000)
}
fn source_at_rate(mode: WarpMode, rate: u32) -> (Wav, VerifiedGranularSource, Vec<[f32; 2]>) {
    source_fixture(mode, rate, false)
}
fn source_fixture(
    mode: WarpMode,
    rate: u32,
    nan_tail: bool,
) -> (Wav, VerifiedGranularSource, Vec<[f32; 2]>) {
    let (wav, source, expected) =
        source_fixture_using(mode, rate, nan_tail, |raw, root, reader, plan| {
            VerifiedGranularSource::prepare_from_reader_on_control(
                raw,
                root,
                reader,
                plan,
                preparation_limits(),
                || false,
            )
        });
    (wav, source.unwrap(), expected)
}
fn preparation_limits() -> GranularSourcePreparationLimits {
    GranularSourcePreparationLimits {
        source_scratch_frames: 8192,
        checkpoint_entries: 2,
        checkpoint_budget: 2_000_000,
        max_processing_bytes: hex_warp::MAX_SCRATCH_BYTES,
        max_construction_peak_bytes: 96 * 1024 * 1024,
    }
}
fn source_fixture_using(
    mode: WarpMode,
    rate: u32,
    nan_tail: bool,
    prepare: impl FnOnce(
        StreamedSourceMetadata,
        hex_engine::audio_transform::StreamedGranularRoot,
        VerifiedWavReader,
        ResamplePlan,
    ) -> Result<VerifiedGranularSource, GranularSourceError>,
) -> (
    Wav,
    Result<VerifiedGranularSource, GranularSourceError>,
    Vec<[f32; 2]>,
) {
    let (wav, pcm) = Wav::create(8192, false);
    if nan_tail {
        let mut bytes = fs::read(&wav.0).unwrap()[..44].to_vec();
        bytes[4..8].copy_from_slice(&(36u32 + 8192 * 8).to_le_bytes());
        bytes[20..22].copy_from_slice(&3u16.to_le_bytes());
        bytes[28..32].copy_from_slice(&384_000u32.to_le_bytes());
        bytes[32..34].copy_from_slice(&8u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&32u16.to_le_bytes());
        bytes[40..44].copy_from_slice(&(8192u32 * 8).to_le_bytes());
        for (index, frame) in pcm.iter().enumerate() {
            for sample in frame {
                bytes.extend_from_slice(
                    &if index == 8191 { f32::NAN } else { *sample }.to_le_bytes(),
                );
            }
        }
        fs::write(&wav.0, bytes).unwrap();
    }
    let version = if rate == 48_000 {
        ResampleVersion::V1
    } else {
        ResampleVersion::V2
    };
    let (reader, plan) = if nan_tail {
        let reader = VerifiedWavReader::open(
            &wav.0,
            VerifiedWavReaderLimits::new(
                WavReaderLimits::new(8192 * 8 + 44, 8192, 128, 8192),
                4096,
            ),
        )
        .unwrap();
        let plan = ResamplePlan::with_kernel(
            ResampleKernel::with_version(48_000, rate, version).unwrap(),
            8192,
            8192,
            16384,
            4096,
        )
        .unwrap();
        (reader, plan)
    } else {
        let (reader, plan, _) = setup(&wav, 8192, rate, version);
        (reader, plan)
    };
    let converted_frames = plan.output_frames();
    let pcm = resample_with_version(
        &Audio::new(48_000, pcm).unwrap(),
        rate,
        ResampleLimits {
            max_source_frames: 8192,
            max_output_frames: 16384,
        },
        version,
    )
    .unwrap()
    .frames()
    .to_vec();
    let mut editor = Editor::new(Project::new(rate).unwrap(), 8).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "granular".into(),
            path: wav.0.clone(),
            sample_rate: rate,
            frames: converted_frames,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    if nan_tail {
        editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: converted_frames / 2,
                source_offset: 0,
                source_length: converted_frames / 2,
            })
            .unwrap();
    }
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset),
        48_000,
        rate,
        8192,
        converted_frames,
        if rate == 48_000 {
            IDENTITY_CONVERSION
        } else {
            SINC_CONVERSION_V2
        },
        *reader.fingerprint(),
    )
    .unwrap();
    let root = || {
        streamed_granular_root(
            &raw,
            editor.project().clip(clip).unwrap().audio_edit().unwrap(),
            editor.project().tempo_map(),
            &AtomicBool::new(false),
        )
        .unwrap()
    };
    let (metadata, region, map, mode, pitch_cents, law) = root().into_parts();
    let expected = hex_warp::render_with_resampler(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: &pcm,
                sample_rate: rate,
                region_start: region.start(),
                region_length: region.length(),
                phase: region.phase(),
                looping: region.looping(),
                envelope: region.envelope(),
            },
            map: &map,
            output_frames: metadata.playback_frames() as usize,
            pitch_cents,
            mode,
        },
        law,
        &mut || false,
    )
    .unwrap()
    .frames;
    let source = prepare(raw.clone(), root(), reader, plan);
    (wav, source, expected)
}

#[test]
fn dispatch_preserves_distinct_pcm_and_each_sources_history() {
    let (_tones_wav, tones, tones_pcm) = source(WarpMode::Tones { grain_ms: 32 });
    let (_texture_wav, texture, texture_pcm) = source(WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
    let keys = [tones.metadata().key(), texture.metadata().key()];
    assert_ne!(keys[0], keys[1]);
    assert_ne!(tones_pcm, texture_pcm);
    let (mut tx, rx) = rtrb::RingBuffer::new(3);
    let (ready, mut results) = rtrb::RingBuffer::new(1);
    let mut expected = Vec::new();
    for (source, page_index) in [(0, 0), (1, 0), (0, 1)] {
        let key = GranularWorkerKey {
            source: keys[source],
            epoch: 1,
            page_index,
            seek: GranularWorkerSeek::Segmented {
                segment_frames: 4096,
            },
        };
        let frames = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        expected.push((key, frames.as_ptr() as usize, source));
        assert!(tx.push(GranularWorkerRequest { key, frames }).is_ok());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let bank = admit(vec![tones, texture]);
    let join = thread::spawn(move || {
        run_granular_worker(
            bank,
            GranularWorkerQueues {
                requests: rx,
                results: ready,
            },
            &worker_stop,
            &AtomicU64::new(1),
        )
    });
    for (key, pointer, source) in expected {
        let deadline = Instant::now() + Duration::from_secs(3);
        let outcome = loop {
            if let Ok(outcome) = results.pop() {
                break outcome;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not return source page"
            );
            thread::yield_now();
        };
        assert_eq!(outcome.key, key);
        let GranularWorkerResult::Ready(page) = outcome.result else {
            panic!("expected verified page")
        };
        assert_eq!(page.metadata.key(), key.source);
        assert_eq!(page.frames.as_ptr() as usize, pointer);
        let pcm = if source == 0 {
            &tones_pcm
        } else {
            &texture_pcm
        };
        let start = key.page_index as usize * STREAM_PAGE_FRAMES;
        assert_eq!(&*page.frames, &pcm[start..start + STREAM_PAGE_FRAMES]);
    }
    let key = GranularWorkerKey {
        source: StreamSourceKey::new(99, keys[0].asset()),
        epoch: 1,
        page_index: 0,
        seek: GranularWorkerSeek::Strict {
            max_preroll_frames: 0,
        },
    };
    let frames = vec![[17.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    let pointer = frames.as_ptr() as usize;
    assert!(tx.push(GranularWorkerRequest { key, frames }).is_ok());
    let deadline = Instant::now() + Duration::from_secs(3);
    let outcome = loop {
        if let Ok(outcome) = results.pop() {
            break outcome;
        }
        assert!(Instant::now() < deadline, "unknown source was not returned");
        thread::yield_now();
    };
    assert_eq!(outcome.key, key);
    let GranularWorkerResult::Failed {
        error: GranularWorkerError::UnknownSource,
        frames,
    } = outcome.result
    else {
        panic!("expected unknown source refusal")
    };
    assert_eq!(frames.as_ptr() as usize, pointer);
    assert!(frames.iter().all(|frame| *frame == [17.0; 2]));
    stop.store(true, Ordering::Release);
    join.thread().unpark();
    let exit = join.join().unwrap();
    assert_eq!(exit.reason, GranularWorkerExitReason::Stopped);
    assert_eq!(
        exit.sources
            .iter()
            .map(|s| s.metadata().key())
            .collect::<Vec<_>>(),
        keys
    );
    assert!(exit.interrupted.is_none());
}

#[test]
fn invalid_banks_return_all_sources_before_worker_start() {
    let (_wav_a, a, _) = source(WarpMode::Tones { grain_ms: 32 });
    let (_wav_b, b, _) = source(WarpMode::Tones { grain_ms: 32 });
    assert_eq!(a.metadata().key(), b.metadata().key());
    let mut oversized = Vec::with_capacity(513);
    let (_wav_c, c, _) = source(WarpMode::Tones { grain_ms: 32 });
    oversized.push(c);
    for sources in [vec![], vec![a, b], oversized] {
        let count = sources.len();
        let pointer = sources.as_ptr();
        let capacity = sources.capacity();
        let Err(refusal) = GranularWorkerBank::try_new_on_control(
            sources,
            GranularBankAllowance {
                max_sources: 512,
                max_data_bytes: 64 * 1024 * 1024,
            },
        ) else {
            panic!("invalid bank was admitted")
        };
        assert_eq!(refusal.reason, GranularBankError::InvalidSources);
        assert_eq!(refusal.sources.len(), count);
        assert_eq!(refusal.sources.capacity(), capacity);
        assert_eq!(refusal.sources.as_ptr(), pointer);
    }
}

#[cfg(feature = "test-support")]
#[test]
fn stop_reclaims_active_second_source_and_keeps_the_next_request_queued() {
    let (_tones_wav, tones, _) = source(WarpMode::Tones { grain_ms: 32 });
    let (_texture_wav, texture, _) = source(WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
    let keys = [tones.metadata().key(), texture.metadata().key()];
    let (mut tx, rx) = rtrb::RingBuffer::new(2);
    let (ready, mut results) = rtrb::RingBuffer::new(1);
    let mut pointers = Vec::new();
    for index in [1, 0] {
        let frames = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        pointers.push(frames.as_ptr() as usize);
        let key = GranularWorkerKey {
            source: keys[index],
            epoch: 1,
            page_index: 0,
            seek: GranularWorkerSeek::Strict {
                max_preroll_frames: 0,
            },
        };
        assert!(tx.push(GranularWorkerRequest { key, frames }).is_ok());
    }
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let bank = admit(vec![tones, texture]);
    let join = thread::spawn(move || {
        hex_stream::test_support::run_granular_worker_with_step_hook(
            bank,
            GranularWorkerQueues {
                requests: rx,
                results: ready,
            },
            &worker_stop,
            &AtomicU64::new(1),
            || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    stop.store(true, Ordering::Release);
    release_tx.send(()).unwrap();
    let mut exit = join.join().unwrap();
    assert_eq!(exit.reason, GranularWorkerExitReason::Stopped);
    assert_eq!(
        exit.sources
            .iter()
            .map(|s| s.metadata().key())
            .collect::<Vec<_>>(),
        keys
    );
    let active = exit.interrupted.take().unwrap();
    assert_eq!(active.key.source, keys[1]);
    assert_eq!(active.frames.as_ptr() as usize, pointers[0]);
    let queued = exit.queues.requests.pop().unwrap();
    assert_eq!(queued.key.source, keys[0]);
    assert_eq!(queued.frames.as_ptr() as usize, pointers[1]);
    assert!(results.pop().is_err());
    assert!(exit.held.is_none());
    assert!(
        exit.sources
            .iter_mut()
            .all(|source| source.reclaim().is_none())
    );
}

#[test]
fn aggregate_admission_includes_reader_kernel_and_processing_data() {
    let (_wav_a, a, _) = source(WarpMode::Tones { grain_ms: 32 });
    let (_wav_b, b, _) = source_at_rate(
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
        44_100,
    );
    let processing = a.reserved_processing_bytes() + b.reserved_processing_bytes();
    let bank = admit(vec![a, b]);
    let data = bank.data();
    assert_eq!(data.processing_reserved_bytes, processing);
    assert_eq!(data.source_readers, 2);
    assert_eq!(data.manifest_handles, 2);
    assert_eq!(data.kernel_handles, 2);
    assert!(
        data.reader_heap_bytes >= 2 * (32 + hex_media::VerifiedWavManifest::owner_payload_bytes()),
        "both verified manifests are charged"
    );
    let kernel = ResampleKernel::with_version(48_000, 44_100, ResampleVersion::V2).unwrap();
    assert!(kernel.table_payload_bytes() > 0);
    assert!(
        data.kernel_reserved_bytes
            >= 2 * ResampleKernel::owner_payload_bytes() + kernel.table_payload_bytes()
    );
    let bytes = data.bytes().unwrap();
    assert!(bytes > processing + data.kernel_reserved_bytes + data.reader_heap_bytes);
    let keys = bank
        .metadata()
        .map(StreamedSourceMetadata::key)
        .collect::<Vec<_>>();
    let Err(refusal) = GranularWorkerBank::try_new_on_control(
        bank.into_sources(),
        GranularBankAllowance {
            max_sources: 2,
            max_data_bytes: bytes - 1,
        },
    ) else {
        panic!("insufficient aggregate allowance was admitted")
    };
    assert_eq!(refusal.reason, GranularBankError::DataLimit);
    let bank = GranularWorkerBank::try_new_on_control(
        refusal.sources,
        GranularBankAllowance {
            max_sources: 2,
            max_data_bytes: bytes,
        },
    )
    .unwrap();
    assert_eq!(bank.data(), data);
    assert_eq!(
        bank.metadata()
            .map(StreamedSourceMetadata::key)
            .collect::<Vec<_>>(),
        keys
    );
}

#[test]
fn admission_returns_sources_holding_pending_or_complete_pages() {
    for complete in [false, true] {
        let (_wav, mut source, _) = source(WarpMode::Tones { grain_ms: 32 });
        let frames = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        let pointer = frames.as_ptr();
        source.begin_page(0, frames, 0, || false).unwrap();
        if complete {
            loop {
                if source.step(|| false).unwrap() == hex_stream::GranularPageProgress::Complete {
                    break;
                }
            }
        }
        let Err(mut refusal) = GranularWorkerBank::try_new_on_control(
            vec![source],
            GranularBankAllowance {
                max_sources: 1,
                max_data_bytes: 64 * 1024 * 1024,
            },
        ) else {
            panic!("bank took custody of a caller page")
        };
        assert_eq!(refusal.reason, GranularBankError::SourceHoldsPage);
        let frames = refusal.sources[0].reclaim().unwrap();
        assert_eq!(frames.as_ptr(), pointer);
        assert_eq!(admit(refusal.sources).metadata().count(), 1);
    }
}

fn control(bank: GranularWorkerBank, capacity: usize) -> hex_stream::GranularWorkerControl {
    let data = hex_stream::GranularWorkerControl::data_bound(&bank, capacity).unwrap();
    hex_stream::GranularWorkerControl::try_start_on_control(
        bank,
        capacity,
        hex_stream::GranularWorkerAllowance {
            max_data_bytes: data.bytes().unwrap(),
            max_shared_handles: data.shared_handle_bound().unwrap(),
        },
    )
    .unwrap()
}

#[test]
fn control_admission_refuses_short_data_handles_and_invalid_queues() {
    use hex_stream::{GranularWorkerAllowance, GranularWorkerControl, StreamRuntimeError};
    for case in 0..4 {
        let (wav, source, _) = source(WarpMode::Tones { grain_ms: 32 });
        let bank = admit(vec![source]);
        let data = GranularWorkerControl::data_bound(&bank, 1).unwrap();
        let mut allowance = GranularWorkerAllowance {
            max_data_bytes: data.bytes().unwrap(),
            max_shared_handles: data.shared_handle_bound().unwrap(),
        };
        let mut capacity = 1;
        match case {
            0 => allowance.max_data_bytes -= 1,
            1 => allowance.max_shared_handles -= 1,
            2 => capacity = 0,
            _ => capacity = 1025,
        }
        assert!(matches!(
            GranularWorkerControl::try_start_on_control(bank, capacity, allowance),
            Err(StreamRuntimeError::InvalidConfiguration)
        ));
        assert!(
            VerifiedWavReader::open(
                &wav.0,
                VerifiedWavReaderLimits::new(
                    WavReaderLimits::new(8192 * 4 + 44, 8192, 128, 8192),
                    4096
                )
            )
            .is_ok()
        );
    }
}

#[test]
fn control_fences_already_queued_pcm_then_completes_current_demand() {
    let (_wav, source, expected) = source(WarpMode::Tones { grain_ms: 32 });
    let source_key = source.metadata().key();
    let mut owner = control(admit(vec![source]), 1);
    let frames = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    let pointer = frames.as_ptr();
    let mut key = GranularWorkerKey {
        source: source_key,
        epoch: owner.epoch(),
        page_index: 0,
        seek: GranularWorkerSeek::Strict {
            max_preroll_frames: 0,
        },
    };
    for (bad_key, bad_frames) in [
        (key, vec![[7.0; 2]; 1].into_boxed_slice()),
        (
            GranularWorkerKey { epoch: 0, ..key },
            vec![[7.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice(),
        ),
    ] {
        let pointer = bad_frames.as_ptr();
        let refused = owner
            .try_submit_on_control(GranularWorkerRequest {
                key: bad_key,
                frames: bad_frames,
            })
            .unwrap_err();
        assert_eq!(refused.key, bad_key);
        assert_eq!(refused.frames.as_ptr(), pointer);
    }
    assert_eq!(owner.queued_results(), 0);
    assert_eq!(owner.available_request_slots(), 1);
    assert!(
        owner
            .try_submit_on_control(GranularWorkerRequest { key, frames })
            .is_ok()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while owner.queued_results() == 0 {
        assert!(
            Instant::now() < deadline,
            "worker did not queue completed PCM"
        );
        thread::yield_now();
    }
    assert_eq!(owner.supersede_on_control().unwrap(), 2);
    let outcome = owner.poll_on_control().unwrap();
    assert_eq!(outcome.key, key);
    let GranularWorkerResult::Cancelled { frames } = outcome.result else {
        panic!("stale PCM escaped the owner fence")
    };
    assert_eq!(frames.as_ptr(), pointer);
    key.epoch = owner.epoch();
    assert!(
        owner
            .try_submit_on_control(GranularWorkerRequest { key, frames })
            .is_ok()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let outcome = loop {
        if let Some(outcome) = owner.poll_on_control() {
            break outcome;
        }
        assert!(Instant::now() < deadline, "current demand did not complete");
        thread::yield_now();
    };
    assert_eq!(outcome.key, key);
    let GranularWorkerResult::Ready(page) = outcome.result else {
        panic!("current demand refused")
    };
    assert_eq!(page.frames.as_ptr(), pointer);
    assert_eq!(&*page.frames, &expected[..STREAM_PAGE_FRAMES]);
    let mut stopped = owner.shutdown_on_control().unwrap();
    assert_eq!(stopped.worker.reason, GranularWorkerExitReason::Stopped);
    assert_eq!(stopped.worker.sources.len(), 1);
    assert!(stopped.results.pop().is_err());
    assert!(stopped.worker.queues.requests.pop().is_err());
    assert!(stopped.worker.held.is_none() && stopped.worker.interrupted.is_none());
}

#[test]
fn control_shutdown_conserves_every_buffer_with_full_queues() {
    let (_wav_a, a, _) = source(WarpMode::Tones { grain_ms: 32 });
    let (_wav_b, b, _) = source(WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
    let keys = [a.metadata().key(), b.metadata().key()];
    let mut owner = control(admit(vec![a, b]), 1);
    let request = |source, epoch, value| GranularWorkerRequest {
        key: GranularWorkerKey {
            source,
            epoch,
            page_index: 0,
            seek: GranularWorkerSeek::Strict {
                max_preroll_frames: 0,
            },
        },
        frames: vec![[value; 2]; STREAM_PAGE_FRAMES].into_boxed_slice(),
    };
    let first = request(keys[0], owner.epoch(), 1.0);
    let second = request(keys[1], owner.epoch(), 2.0);
    let third = request(keys[0], owner.epoch(), 3.0);
    let mut expected = vec![
        first.frames.as_ptr() as usize,
        second.frames.as_ptr() as usize,
        third.frames.as_ptr() as usize,
    ];
    assert!(owner.try_submit_on_control(first).is_ok());
    let deadline = Instant::now() + Duration::from_secs(3);
    while owner.queued_results() == 0 {
        assert!(Instant::now() < deadline, "first result not queued");
        thread::yield_now();
    }
    assert!(owner.try_submit_on_control(second).is_ok());
    while owner.available_request_slots() == 0 {
        assert!(Instant::now() < deadline, "second request not dequeued");
        thread::yield_now();
    }
    assert!(owner.try_submit_on_control(third).is_ok());
    let refused = request(keys[0], owner.epoch(), 4.0);
    let pointer = refused.frames.as_ptr();
    let Err(refused) = owner.try_submit_on_control(refused) else {
        panic!("full queue accepted another page")
    };
    assert_eq!(refused.frames.as_ptr(), pointer);
    assert!(refused.frames.iter().all(|frame| *frame == [4.0; 2]));
    let mut stopped = owner.shutdown_on_control().unwrap();
    assert_eq!(stopped.worker.reason, GranularWorkerExitReason::Stopped);
    assert_eq!(
        stopped
            .worker
            .sources
            .iter()
            .map(|s| s.metadata().key())
            .collect::<Vec<_>>(),
        keys
    );
    let mut returned = Vec::new();
    while let Ok(outcome) = stopped.results.pop() {
        returned.push(outcome_frames(outcome).as_ptr() as usize);
    }
    if let Some(outcome) = stopped.worker.held.take() {
        returned.push(outcome_frames(outcome).as_ptr() as usize);
    }
    if let Some(request) = stopped.worker.interrupted.take() {
        returned.push(request.frames.as_ptr() as usize);
    }
    while let Ok(request) = stopped.worker.queues.requests.pop() {
        returned.push(request.frames.as_ptr() as usize);
    }
    expected.sort_unstable();
    returned.sort_unstable();
    assert_eq!(returned, expected);
}
fn outcome_frames(outcome: hex_stream::GranularWorkerOutcome) -> Box<[[f32; 2]]> {
    match outcome.result {
        GranularWorkerResult::Ready(page) => page.frames,
        GranularWorkerResult::Cancelled { frames }
        | GranularWorkerResult::Failed { frames, .. } => frames,
    }
}

fn runtime_allowance(
    bank: &GranularWorkerBank,
    pages: usize,
) -> hex_stream::GranularWorkerAllowance {
    let data = hex_stream::GranularWorkerControl::data_bound(bank, pages).unwrap();
    hex_stream::GranularWorkerAllowance {
        max_data_bytes: data.bytes().unwrap(),
        max_shared_handles: data.shared_handle_bound().unwrap(),
    }
}
fn runtime_page(
    runtime: &mut hex_stream::StreamRuntime,
    key: StreamSourceKey,
    index: u64,
) -> Vec<[f32; 2]> {
    use hex_engine::MediaPages;
    runtime.request_page(key, index);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        runtime.pump_ready(8);
        if let Some(page) = runtime.page(key, index) {
            return page.frames.to_vec();
        }
        let mut failures = [hex_stream::StreamFailure {
            key: key.into(),
            page_index: index,
            kind: StreamFailureKind::Cancelled,
        }; 1];
        if runtime.take_failures(&mut failures) != 0 {
            panic!("runtime page {key:?}/{index} failed: {:?}", failures[0]);
        }
        assert!(
            Instant::now() < deadline,
            "runtime page {key:?}/{index} did not arrive"
        );
        thread::yield_now();
    }
}
fn raw_definition() -> (Wav, hex_stream::SourceDefinition, Vec<[f32; 2]>) {
    let (wav, pcm) = Wav::create(8192, false);
    let (reader, _, raw) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let limits =
        VerifiedWavReaderLimits::new(WavReaderLimits::new(8192 * 4 + 44, 8192, 128, 8192), 4096);
    let definition = hex_stream::SourceDefinition::new(wav.0.clone(), raw, limits)
        .with_verified_manifest(reader.manifest().clone());
    (wav, definition, pcm)
}

#[test]
fn runtime_routes_mixed_raw_tones_and_texture_pages_and_admits_native_custody() {
    use hex_stream::{ConstructorStreamDomain, StreamConfig, StreamRuntime};
    let (_raw_wav, raw, raw_pcm) = raw_definition();
    let raw_key = raw.metadata.key();
    let (_tones_wav, tones, tones_pcm) = source(WarpMode::Tones { grain_ms: 200 });
    let (_texture_wav, texture, texture_pcm) = source_at_rate(
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
        44_100,
    );
    let keys = [raw_key, tones.metadata().key(), texture.metadata().key()];
    let bank = admit(vec![tones, texture]);
    let allowance = runtime_allowance(&bank, 4);
    let mut runtime = StreamRuntime::spawn_with_granular_sources(
        StreamConfig::with_pool_pages(4),
        vec![raw],
        vec![],
        bank,
        allowance,
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let footprint = runtime.catalog_footprint().unwrap();
    assert_eq!(footprint.sources, 3);
    assert!(footprint.granular_data_bytes > allowance.max_data_bytes);
    for (source, index) in [(0, 0), (1, 1), (2, 1), (1, 0), (2, 0), (0, 1)] {
        let actual = runtime_page(&mut runtime, keys[source], index);
        let expected = [&raw_pcm, &tones_pcm, &texture_pcm][source];
        let start = index as usize * STREAM_PAGE_FRAMES;
        assert_eq!(
            actual,
            expected[start..(start + STREAM_PAGE_FRAMES).min(expected.len())]
        );
    }
    let (control, pages) = runtime.split().ok().unwrap();
    {
        use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
        use hex_stream::{
            AudioStreamStorageLedger, AudioStreamStorageReservation, OpaqueStreamHandleGroup,
            StreamHandleLocation,
        };
        let roots = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
            data_rows: 0,
            handle_rows: 0,
            root_rows: 0,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        })
        .unwrap();
        let mut ledger = AudioStreamStorageLedger::try_new_on_control(
            &roots,
            AudioStreamStorageReservation {
                domains: 1,
                caller_leases: 0,
                max_listed_data_bytes: usize::MAX,
                max_listed_construction_peak_bytes: usize::MAX,
            },
        )
        .unwrap();
        ledger.borrow_domain_on_control(&control, &pages).unwrap();
        let handles = ledger
            .owning_handle_obligations()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(handles.len(), 15 + 2 + 8 + 4);
        for location in [
            StreamHandleLocation::SourceManifest(0),
            StreamHandleLocation::GranularManifest(0),
            StreamHandleLocation::GranularManifest(1),
            StreamHandleLocation::GranularKernel(0),
            StreamHandleLocation::GranularKernel(1),
            StreamHandleLocation::GranularOwnerEpoch,
            StreamHandleLocation::GranularWorkerEpoch,
        ] {
            assert!(handles.iter().any(|h| h.location == location));
        }
        assert!(
            ledger
                .opaque_handle_obligations()
                .any(|(_, kind)| kind == OpaqueStreamHandleGroup::GranularWorkerThread)
        );
        let summary = ledger.summary().unwrap().require_listed_data().unwrap();
        assert_eq!(summary.unresolved_known_stream_handles, handles.len());
        assert_eq!(summary.unresolved_opaque_handle_groups, 3);
        assert_eq!(
            summary.require_complete(),
            Err(hex_stream::AudioStreamStorageError::UnresolvedObligations)
        );
    }
    // 19 fixed pair/disposal/lease handles, raw manifest+plan, and eight
    // granular queue/atomic fields plus two manifest+plan pairs.
    let expected_handles = 19 + 2 + 8 + 4;
    let Err(refusal) = ConstructorStreamDomain::try_new_with_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        expected_handles - 1,
    ) else {
        panic!("short native handle allowance admitted")
    };
    assert_eq!(
        refusal.reason,
        hex_stream::AudioStreamStorageError::ReservationExceeded
    );
    let mut domain = ConstructorStreamDomain::try_new_with_limits_on_control(
        refusal.control,
        refusal.pages,
        usize::MAX,
        usize::MAX,
        expected_handles,
    )
    .ok()
    .unwrap();
    assert_eq!(domain.summary().shared_handle_bound, expected_handles);
    assert_eq!(domain.summary().worker_lifetimes, 2);
    assert_eq!(domain.summary().source_reader_lifetimes, 3);
    assert_eq!(domain.summary().kernel_handle_bound, 3);
    let report = loop {
        if let Some(report) = domain.advance_disposal_on_control().unwrap() {
            break report;
        }
    };
    assert!(report.accounting_complete && report.worker_joined);
    assert_eq!(report.page_buffers, 4);
}

#[cfg(feature = "test-support")]
#[test]
fn runtime_raw_service_and_stop_progress_while_granular_work_is_held() {
    use hex_engine::MediaPages;
    use hex_stream::{StreamConfig, StreamRuntime, test_support::PageGate};
    let (_raw_wav, raw, expected) = raw_definition();
    let raw_key = raw.metadata.key();
    let (_wav, source, _) = source(WarpMode::Tones { grain_ms: 32 });
    let key = source.metadata().key();
    let bank = admit(vec![source]);
    let allowance = runtime_allowance(&bank, 3);
    let (gate, entered, _release) = PageGate::new(hex_stream::PageKey {
        source: key,
        page_index: 0,
    });
    let mut config = StreamConfig::with_pool_pages(3);
    config.ready_capacity = 1;
    config.max_ready_pages_per_pump = 1;
    let mut runtime = StreamRuntime::new_with_granular_page_gate(
        config,
        vec![raw],
        vec![],
        bank,
        allowance,
        gate,
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    runtime.request_page(key, 0);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    runtime.request_page(key, 1);
    let mut failures = [hex_stream::StreamFailure {
        key: key.into(),
        page_index: 1,
        kind: StreamFailureKind::Cancelled,
    }; 1];
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime.take_failures(&mut failures) == 0 {
        assert!(
            Instant::now() < deadline,
            "granular loan saturation not reported"
        );
        thread::yield_now();
    }
    assert_eq!(failures[0].key, key.into());
    assert_eq!(failures[0].page_index, 1);
    assert_eq!(failures[0].kind, StreamFailureKind::CapacityExhausted);
    let actual = runtime_page(&mut runtime, raw_key, 0);
    assert_eq!(actual, expected[..STREAM_PAGE_FRAMES]);
    for index in [1, 0, 1] {
        let start = index as usize * STREAM_PAGE_FRAMES;
        assert_eq!(
            runtime_page(&mut runtime, raw_key, index),
            expected[start..start + STREAM_PAGE_FRAMES]
        );
    }
    assert!(runtime.page(key, 0).is_none());
    // Never release the held child: parent Stop must stop/join it and reclaim
    // its active original-pool buffer, with ordinary cache turnover complete.
    let report = runtime.shutdown().unwrap();
    assert_eq!(report.page_buffers, 3);
    assert!(report.accounting_complete && report.worker_joined);
}

#[test]
fn runtime_granular_failure_returns_pool_page_for_following_raw_request() {
    use hex_engine::MediaPages;
    use hex_stream::{StreamConfig, StreamFailure, StreamRuntime};
    let (_raw_wav, raw, expected) = raw_definition();
    let raw_key = raw.metadata.key();
    let (_wav, source, _) = source(WarpMode::Tones { grain_ms: 32 });
    let key = source.metadata().key();
    let bank = admit(vec![source]);
    let allowance = runtime_allowance(&bank, 3);
    let mut runtime = StreamRuntime::spawn_with_granular_sources(
        StreamConfig::with_pool_pages(3),
        vec![raw],
        vec![],
        bank,
        allowance,
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    runtime.request_page(key, u64::MAX);
    let mut failures = [StreamFailure {
        key: key.into(),
        page_index: 0,
        kind: StreamFailureKind::Cancelled,
    }; 1];
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime.take_failures(&mut failures) == 0 {
        assert!(Instant::now() < deadline, "granular failure not reported");
        thread::yield_now();
    }
    assert_eq!(failures[0].key, key.into());
    assert_eq!(failures[0].page_index, u64::MAX);
    assert_eq!(failures[0].kind, StreamFailureKind::DecodeFailed);
    assert_eq!(
        runtime_page(&mut runtime, raw_key, 1),
        expected[STREAM_PAGE_FRAMES..2 * STREAM_PAGE_FRAMES]
    );
    assert!(runtime.shutdown().unwrap().accounting_complete);
}

#[test]
fn runtime_refuses_source_changed_after_preparation_before_catalog_eligibility() {
    use hex_stream::{StreamConfig, StreamRuntime, StreamRuntimeError};
    let (wav, source, _) = source(WarpMode::Tones { grain_ms: 32 });
    let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    file.write_all(&1234i16.to_le_bytes()).unwrap();
    drop(file);
    let bank = admit(vec![source]);
    let allowance = runtime_allowance(&bank, 3);
    let mut runtime = StreamRuntime::spawn_with_granular_sources(
        StreamConfig::with_pool_pages(3),
        vec![],
        vec![],
        bank,
        allowance,
    )
    .unwrap();
    assert_eq!(
        runtime.wait_until_ready(Duration::from_secs(3)),
        Err(StreamRuntimeError::CatalogFailed(
            StreamFailureKind::SourceChanged
        ))
    );
    assert!(!runtime.catalog_ready());
    assert!(runtime.shutdown().unwrap().accounting_complete);
}

#[test]
fn runtime_float_preflight_refuses_nan_outside_granular_region() {
    use hex_stream::{StreamConfig, StreamRuntime, StreamRuntimeError};
    let (_wav, source, _) = source_fixture(WarpMode::Tones { grain_ms: 32 }, 48_000, true);
    let bank = admit(vec![source]);
    let allowance = runtime_allowance(&bank, 3);
    let mut runtime = StreamRuntime::spawn_with_granular_sources(
        StreamConfig::with_pool_pages(3),
        vec![],
        vec![],
        bank,
        allowance,
    )
    .unwrap();
    assert_eq!(
        runtime.wait_until_ready(Duration::from_secs(3)),
        Err(StreamRuntimeError::CatalogFailed(
            StreamFailureKind::DecodeFailed
        ))
    );
    assert!(!runtime.catalog_ready());
    assert!(runtime.shutdown().unwrap().accounting_complete);
}

fn pending_fixture() -> (
    Vec<Wav>,
    Vec<hex_stream::SourceDefinition>,
    GranularWorkerBank,
) {
    let (raw_wav, raw, _) = raw_definition();
    let (tones_wav, tones, _) = source(WarpMode::Tones { grain_ms: 32 });
    let (texture_wav, texture, _) = source_at_rate(
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
        44_100,
    );
    (
        vec![raw_wav, tones_wav, texture_wav],
        vec![raw],
        admit(vec![tones, texture]),
    )
}
fn pending_allowance(
    bound: hex_stream::StreamPreparationBound,
) -> hex_stream::GranularStreamPreparationAllowance {
    hex_stream::GranularStreamPreparationAllowance {
        stream: hex_stream::StreamPreparationAllowance {
            max_retained_data_bytes: bound.retained_data_bytes,
            max_construction_peak_data_bytes: bound.construction_peak_data_bytes,
            max_shared_handle_bound: bound.shared_handle_bound,
        },
        max_source_readers: bound.source_reader_bound,
        max_worker_lifetimes: bound.worker_lifetimes,
    }
}
#[test]
fn pending_exact_mixed_granular_allowance_seals_and_reclaims_the_original_pool() {
    use hex_stream::{PendingStreamDomain, StreamConfig, StreamReadinessReservation};
    let (_files, definitions, bank) = pending_fixture();
    let metadata = bank.metadata().cloned().collect::<Vec<_>>();
    let config = StreamConfig::with_pool_pages(4);
    let readiness = Some(StreamReadinessReservation {
        extents_per_role: 1,
    });
    assert!(
        PendingStreamDomain::preparation_bound_with_granular_sources(
            config,
            &definitions,
            usize::MAX,
            &[],
            0,
            &bank,
            readiness
        )
        .is_err()
    );
    let bound = PendingStreamDomain::preparation_bound_with_granular_sources(
        config,
        &definitions,
        definitions.capacity(),
        &[],
        0,
        &bank,
        readiness,
    )
    .unwrap();
    assert_eq!(bound.source_reader_bound, 3);
    assert_eq!(bound.worker_lifetimes, 2);
    let mut pending = PendingStreamDomain::try_start_with_granular_sources_on_control(
        config,
        definitions,
        vec![],
        bank,
        readiness,
        pending_allowance(bound),
    )
    .unwrap();
    assert_eq!(pending.reservation(), bound);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut domain = loop {
        if let Some(domain) = pending.try_seal_on_control().unwrap() {
            break domain;
        }
        assert!(
            Instant::now() < deadline,
            "pending granular catalog did not seal"
        );
        thread::yield_now();
    };
    let summary = domain.summary();
    assert_eq!(summary.worker_lifetimes, bound.worker_lifetimes);
    assert_eq!(summary.source_reader_lifetimes, bound.source_reader_bound);
    // The pending quote also reserves one possible raw kernel-cache handle
    // during construction; the sealed source retains only manifest + plan.
    assert_eq!(summary.shared_handle_bound + 1, bound.shared_handle_bound);
    assert!(summary.retained_data_bytes <= bound.retained_data_bytes);
    assert!(summary.construction_peak_data_bytes <= bound.construction_peak_data_bytes);
    assert!(metadata.iter().all(|m| domain.covers_source(m)));
    let mut loan = Some(domain.loan_pages_on_control().unwrap());
    domain.return_pages_on_control(&mut loan).unwrap();
    assert!(loan.is_none());
    let report = loop {
        if let Some(report) = domain.advance_disposal_on_control().unwrap() {
            break report;
        }
        assert!(
            Instant::now() < deadline,
            "sealed granular disposal did not complete"
        );
    };
    assert_eq!(report.page_buffers, 4);
    assert!(report.accounting_complete && report.worker_joined);
    assert!(domain.advance_disposal_on_control().is_err());
}
#[test]
fn pending_granular_short_resource_allowances_and_invalid_inputs_refuse_before_start() {
    use hex_stream::{
        PendingStreamDomain, StreamConfig, StreamReadinessReservation, StreamRuntimeError,
    };
    for case in 0..9 {
        let (_files, definitions, bank) = pending_fixture();
        let mut config = StreamConfig::with_pool_pages(4);
        let mut readiness = Some(StreamReadinessReservation {
            extents_per_role: 1,
        });
        let bound = PendingStreamDomain::preparation_bound_with_granular_sources(
            config,
            &definitions,
            definitions.capacity(),
            &[],
            0,
            &bank,
            readiness,
        )
        .unwrap();
        let mut allowance = pending_allowance(bound);
        match case {
            0 => allowance.stream.max_retained_data_bytes -= 1,
            1 => allowance.stream.max_construction_peak_data_bytes -= 1,
            2 => allowance.stream.max_shared_handle_bound -= 1,
            3 => allowance.max_source_readers -= 1,
            4 => allowance.max_worker_lifetimes -= 1,
            5 => config.page_buffers = 2,
            6 => config.max_sources = 2,
            _ => {
                readiness = Some(StreamReadinessReservation {
                    extents_per_role: 3,
                })
            }
        }
        assert!(
            matches!(
                PendingStreamDomain::try_start_with_granular_sources_on_control(
                    config,
                    definitions,
                    vec![],
                    bank,
                    readiness,
                    allowance
                ),
                Err(StreamRuntimeError::InvalidConfiguration)
            ),
            "case {case}"
        );
    }
}
#[test]
fn pending_granular_cancellation_before_and_after_catalog_returns_every_page() {
    use hex_stream::{PendingStreamDomain, StreamConfig};
    for wait in [false, true] {
        let (_files, definitions, bank) = pending_fixture();
        let config = StreamConfig::with_pool_pages(4);
        let bound = PendingStreamDomain::preparation_bound_with_granular_sources(
            config,
            &definitions,
            definitions.capacity(),
            &[],
            0,
            &bank,
            None,
        )
        .unwrap();
        let mut pending = PendingStreamDomain::try_start_with_granular_sources_on_control(
            config,
            definitions,
            vec![],
            bank,
            None,
            pending_allowance(bound),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        if wait {
            while !pending.poll_catalog_on_control().unwrap() {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
        }
        let report = loop {
            if let Some(report) = pending.advance_disposal_on_control().unwrap() {
                break report;
            }
            assert!(
                Instant::now() < deadline,
                "pending cancellation did not complete"
            );
        };
        assert_eq!(report.page_buffers, 4);
        assert!(report.accounting_complete && report.worker_joined);
        assert!(pending.advance_disposal_on_control().is_err());
    }
}

#[test]
fn allocated_scratch_matches_manual_owner_and_exact_processing_limit() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for rate in [44_100, 48_000] {
            let (_wav, automatic, expected) = source_at_rate(mode, rate);
            let bound = automatic.reserved_processing_bytes();
            let (_wav, manual, manual_expected) =
                source_fixture_using(mode, rate, false, |raw, root, reader, plan| {
                    VerifiedGranularSource::prepare(
                        GranularSourcePreparation {
                            raw,
                            root,
                            reader,
                            plan,
                            buffers: GranularSourceBuffers {
                                source: vec![[0.0; 2]; 8192].into_boxed_slice(),
                                cache: vec![[0.0; 2]; 8192].into_boxed_slice(),
                                grain: vec![[0.0; 2]; 10000].into_boxed_slice(),
                            },
                            scratch_budget: 1_000_000,
                            checkpoint_entries: 2,
                            checkpoint_budget: preparation_limits().checkpoint_budget,
                        },
                        || false,
                    )
                });
            assert_eq!(expected, manual_expected);
            assert!(manual.unwrap().reserved_processing_bytes() >= bound);
            for short in [false, true] {
                let (_wav, result, _) =
                    source_fixture_using(mode, rate, false, |raw, root, reader, plan| {
                        let mut limits = preparation_limits();
                        limits.max_processing_bytes = bound - usize::from(short);
                        limits.max_construction_peak_bytes =
                            limits.construction_processing_bound(rate).unwrap();
                        VerifiedGranularSource::prepare_from_reader_on_control(
                            raw,
                            root,
                            reader,
                            plan,
                            limits,
                            || false,
                        )
                    });
                if short {
                    assert!(matches!(result, Err(GranularSourceError::ScratchLimit)));
                } else {
                    assert_eq!(result.unwrap().reserved_processing_bytes(), bound);
                }
            }
        }
    }
}

#[test]
fn allocated_scratch_refuses_invalid_peak_cache_identity_and_cancellation() {
    for case in 0..9 {
        let (_wav, result, _) = source_fixture_using(
            WarpMode::Tones {
                grain_ms: if case == 8 { 200 } else { 32 },
            },
            48_000,
            false,
            |mut raw, root, reader, mut plan| {
                let mut limits = preparation_limits();
                match case {
                    0 => limits.source_scratch_frames = 0,
                    1 => limits.checkpoint_entries = 1,
                    2 => limits.checkpoint_budget = usize::MAX,
                    3 => {
                        limits.max_construction_peak_bytes =
                            limits.construction_processing_bound(48_000).unwrap() - 1
                    }
                    4 => {
                        let kernel = ResampleKernel::with_version(
                            plan.source_rate(),
                            plan.target_rate(),
                            plan.version(),
                        )
                        .unwrap();
                        plan =
                            ResamplePlan::with_kernel(kernel, plan.source_frames(), 8192, 16384, 1)
                                .unwrap();
                    }
                    5 => {
                        raw = StreamedSourceMetadata::new(
                            StreamSourceKey::new(99, raw.key().asset()),
                            raw.source_rate(),
                            raw.project_rate(),
                            raw.source_frames(),
                            raw.playback_frames(),
                            raw.conversion_version(),
                            raw.fingerprint(),
                        )
                        .unwrap()
                    }
                    6 => {
                        let mut fingerprint = raw.fingerprint();
                        fingerprint[0] ^= 1;
                        raw = StreamedSourceMetadata::new(
                            raw.key(),
                            raw.source_rate(),
                            raw.project_rate(),
                            raw.source_frames(),
                            raw.playback_frames(),
                            raw.conversion_version(),
                            fingerprint,
                        )
                        .unwrap();
                    }
                    8 => limits.checkpoint_budget = 1_000_000,
                    _ => {}
                }
                VerifiedGranularSource::prepare_from_reader_on_control(
                    raw,
                    root,
                    reader,
                    plan,
                    limits,
                    || case == 7,
                )
            },
        );
        assert!(
            result.is_err(),
            "case {case} returned a partial source owner"
        );
        if case <= 4 || case == 8 {
            assert!(
                matches!(result, Err(GranularSourceError::ScratchLimit)),
                "case {case}"
            );
        }
        if case == 5 {
            assert!(matches!(result, Err(GranularSourceError::InvalidSource)));
        }
        if case == 7 {
            assert!(matches!(
                result,
                Err(GranularSourceError::Source(StreamFailureKind::Cancelled))
            ));
        }
    }
}
