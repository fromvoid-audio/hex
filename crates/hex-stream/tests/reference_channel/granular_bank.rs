//! Exact source dispatch over one dedicated worker and retained page custody.
use super::*;
use hex_engine::{STREAM_PAGE_FRAMES, audio_transform::streamed_granular_root};
use hex_project::{WarpMode, default_audio_warp};
use hex_stream::{
    GranularBankAllowance, GranularBankError, GranularSourceBuffers, GranularSourcePreparation,
    GranularWorkerBank, GranularWorkerError, GranularWorkerExitReason, GranularWorkerKey,
    GranularWorkerQueues, GranularWorkerRequest, GranularWorkerResult, GranularWorkerSeek,
    VerifiedGranularSource, run_granular_worker,
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
    let (wav, pcm) = Wav::create(8192, false);
    let version = if rate == 48_000 {
        ResampleVersion::V1
    } else {
        ResampleVersion::V2
    };
    let (reader, plan, _) = setup(&wav, 8192, rate, version);
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
    let root = root();
    let source = VerifiedGranularSource::prepare(
        GranularSourcePreparation {
            raw,
            root,
            reader,
            plan,
            buffers: GranularSourceBuffers {
                source: vec![[0.0; 2]; 8192].into_boxed_slice(),
                cache: vec![[0.0; 2]; 8192].into_boxed_slice(),
                grain: vec![[0.0; 2]; 10_000].into_boxed_slice(),
            },
            scratch_budget: 1_000_000,
            checkpoint_entries: 2,
            checkpoint_budget: 1_000_000,
        },
        || false,
    )
    .unwrap();
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
