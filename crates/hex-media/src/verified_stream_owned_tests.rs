use super::*;
use crate::verified_stream::open_owned_with_reserve_for_test;
use crate::{OwnedVerifiedOpenFailure, VerifiedOpenCapacity};

fn owned_path(path: &Path) -> PathBuf {
    let mut owned = PathBuf::with_capacity(path.as_os_str().len() + 4096);
    owned.push(path);
    owned
}

fn identity(path: &PathBuf) -> (usize, usize, Vec<u8>) {
    (
        path.as_os_str().as_encoded_bytes().as_ptr() as usize,
        path.capacity(),
        path.as_os_str().as_encoded_bytes().to_vec(),
    )
}

fn recover(
    failure: OwnedVerifiedOpenFailure,
    expected: &(usize, usize, Vec<u8>),
) -> (ImportError, VerifiedOpenCapacity) {
    assert_eq!(failure.path().as_os_str().as_encoded_bytes(), expected.2);
    assert_eq!(failure.capacity().path_capacity_bytes, expected.1);
    let (path, error, capacity) = failure.into_parts();
    assert_eq!(&identity(&path), expected);
    assert_eq!(capacity.path_capacity_bytes, path.capacity());
    (error, capacity)
}

#[test]
fn success_retains_original_allocation_and_matches_borrowed_reader() {
    let fixture = TempFile::create(&pcm16_wav(&[[100, -200], [300, -400]], true));
    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let mut owned = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false).unwrap();
    let mut borrowed = VerifiedWavReader::open(fixture.path(), limits(8)).unwrap();
    assert_eq!(
        owned.path().as_os_str().as_encoded_bytes().as_ptr() as usize,
        expected.0
    );
    assert_eq!(owned.path_capacity_bytes(), expected.1);
    assert_eq!(owned.path().as_os_str().as_encoded_bytes(), expected.2);
    assert_eq!(owned.fingerprint(), borrowed.fingerprint());
    assert_eq!(owned.page_digests(), borrowed.page_digests());
    assert_eq!(owned.sample_rate(), borrowed.sample_rate());
    assert_eq!(owned.source_frames(), borrowed.source_frames());
    assert_eq!(
        owned.read_frames(0, 2).unwrap(),
        borrowed.read_frames(0, 2).unwrap()
    );
    let capacity = owned.opening_capacity();
    assert!(capacity.manifest_reservation_attempted);
    assert_eq!(capacity.manifest_bytes(), Some(owned.page_manifest_bytes()));
    assert_eq!(
        capacity.data_bytes(),
        Some(expected.1 + owned.page_manifest_bytes())
    );
}

#[test]
fn missing_invalid_and_numeric_refusals_return_same_owner_before_reservation() {
    let missing = temp_path();
    let path = owned_path(&missing);
    let expected = identity(&path);
    let failure = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false)
        .err()
        .unwrap();
    assert!(
        matches!(failure.error(), ImportError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
    );
    let (error, capacity) = recover(failure, &expected);
    assert!(matches!(error, ImportError::Io(_)));
    assert!(!capacity.manifest_reservation_attempted);
    assert_eq!(capacity.manifest_slots, 0);

    for bytes in [b"not a wav".to_vec(), {
        let mut bytes = pcm16_wav(&[[1, 2]], false);
        bytes[24..28].copy_from_slice(&7999_u32.to_le_bytes());
        bytes
    }] {
        let fixture = TempFile::create(&bytes);
        let path = owned_path(fixture.path());
        let expected = identity(&path);
        let failure = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false)
            .err()
            .unwrap();
        let (error, capacity) = recover(failure, &expected);
        assert!(matches!(error, ImportError::Invalid(_)));
        assert!(!capacity.manifest_reservation_attempted);
    }
    let fixture = TempFile::create(&pcm16_wav(&[[1, 2], [3, 4]], false));
    let initial = limits(8);
    for constrained in [
        VerifiedWavReaderLimits {
            max_manifest_bytes: 0,
            ..initial
        },
        VerifiedWavReaderLimits {
            reader: WavReaderLimits {
                max_file_bytes: 1,
                ..initial.reader
            },
            ..initial
        },
        VerifiedWavReaderLimits {
            reader: WavReaderLimits {
                max_source_frames: 1,
                ..initial.reader
            },
            ..initial
        },
        VerifiedWavReaderLimits {
            reader: WavReaderLimits {
                max_header_count: 0,
                ..initial.reader
            },
            ..initial
        },
    ] {
        let path = owned_path(fixture.path());
        let expected = identity(&path);
        let mut calls = 0;
        let failure = VerifiedWavReader::open_owned_with_cancel(path, constrained, || {
            calls += 1;
            true
        })
        .err()
        .unwrap();
        let (error, capacity) = recover(failure, &expected);
        assert!(matches!(error, ImportError::LimitExceeded(_)));
        assert_eq!(
            calls, 0,
            "existing numeric/parser refusal precedes cancellation"
        );
        assert!(!capacity.manifest_reservation_attempted);
    }
}

#[test]
fn injected_reservation_failure_and_actual_excess_capacity_are_observed() {
    let fixture = TempFile::create(&pcm16_wav(&[[1, 2]], false));
    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let failure = open_owned_with_reserve_for_test(
        path,
        limits(8),
        || false,
        |_, _| Err(ImportError::AllocationFailed),
    )
    .err()
    .unwrap();
    let (error, capacity) = recover(failure, &expected);
    assert!(matches!(error, ImportError::AllocationFailed));
    assert!(capacity.manifest_reservation_attempted);
    assert_eq!(capacity.manifest_slots, 0);

    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let mut returned_slots = 0;
    let mut constrained = limits(8);
    constrained.max_manifest_bytes = 32;
    let failure = open_owned_with_reserve_for_test(
        path,
        constrained,
        || false,
        |manifest, count| {
            manifest
                .try_reserve_exact(count + 7)
                .map_err(|_| ImportError::AllocationFailed)?;
            returned_slots = manifest.capacity();
            Ok(())
        },
    )
    .err()
    .unwrap();
    let (error, capacity) = recover(failure, &expected);
    assert!(matches!(
        error,
        ImportError::LimitExceeded("verified page manifest bytes")
    ));
    assert_eq!(capacity.manifest_slots, returned_slots);
    assert!(capacity.manifest_bytes().unwrap() > constrained.max_manifest_bytes);
    // Storage has been reclaimed, but actual capacity remains in the error.
    assert!(capacity.manifest_reservation_attempted);
}

#[test]
fn cancellation_call_order_matches_existing_api_and_retains_reclaimed_manifest_observation() {
    let fixture = TempFile::create(&pcm16_wav(&vec![[1, 2]; 34_000], false));
    for cancel_at in [0, 1, 3, usize::MAX] {
        let path = owned_path(fixture.path());
        let expected = identity(&path);
        let mut owned_calls = 0;
        let owned = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || {
            let stop = owned_calls == cancel_at;
            owned_calls += 1;
            stop
        });
        let mut borrowed_calls = 0;
        let borrowed = VerifiedWavReader::open_with_cancel(fixture.path(), limits(8), || {
            let stop = borrowed_calls == cancel_at;
            borrowed_calls += 1;
            stop
        });
        assert_eq!(owned_calls, borrowed_calls);
        match (owned, borrowed) {
            (Ok(owned), Ok(borrowed)) => assert_eq!(owned.fingerprint(), borrowed.fingerprint()),
            (Err(failure), Err(ImportError::Cancelled)) => {
                let (error, capacity) = recover(failure, &expected);
                assert!(matches!(error, ImportError::Cancelled));
                assert!(capacity.manifest_reservation_attempted);
                assert!(capacity.manifest_slots >= 3);
            }
            _ => panic!("owned and borrowed cancellation behavior differs"),
        }
    }
}

#[test]
fn mutation_of_already_hashed_page_returns_original_and_manifest_observation() {
    let fixture = TempFile::create(&pcm16_wav(&vec![[0, 0]; 34_000], false));
    let mut writer = OpenOptions::new().write(true).open(fixture.path()).unwrap();
    let modified = writer.metadata().unwrap().modified().unwrap();
    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let mut calls = 0;
    let failure = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || {
        calls += 1;
        if calls == 2 {
            writer.seek(SeekFrom::Start(48)).unwrap();
            writer.write_all(&[1, 2, 3, 4]).unwrap();
            set_modified(&writer, modified);
        }
        false
    })
    .err()
    .unwrap();
    let (error, capacity) = recover(failure, &expected);
    assert!(matches!(error, ImportError::SourceChanged));
    assert!(capacity.manifest_reservation_attempted);
    assert!(capacity.manifest_slots >= 3);
}

#[test]
fn fingerprint_and_rate_are_actual_file_facts_not_expected_metadata_acceptance() {
    let original = TempFile::create(&pcm16_wav(&[[1, 2]], false));
    let reference = VerifiedWavReader::open(original.path(), limits(8)).unwrap();
    let mut bytes = pcm16_wav(&[[3, 4]], false);
    bytes[24..28].copy_from_slice(&44_100_u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&(44_100_u32 * 4).to_le_bytes());
    let changed = TempFile::create(&bytes);
    let path = owned_path(changed.path());
    let expected = identity(&path);
    let reader = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false).unwrap();
    assert_ne!(reader.fingerprint(), reference.fingerprint());
    assert_ne!(reader.sample_rate(), reference.sample_rate());
    assert_eq!(reader.sample_rate(), 44_100);
    assert_eq!(
        reader.path().as_os_str().as_encoded_bytes().as_ptr() as usize,
        expected.0
    );
    // Expected graph metadata is checked by the stream worker, not this opener.
}

#[test]
fn owned_reader_pins_opened_descriptor_after_path_replacement() {
    let original = TempFile::create(&pcm16_wav(&[[11, -11]], false));
    let replacement = TempFile::create(&pcm16_wav(&[[99, -99]], false));
    let path = owned_path(original.path());
    let expected = identity(&path);
    let mut reader = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false).unwrap();
    fs::rename(replacement.path(), original.path()).unwrap();
    assert_eq!(
        reader.read_frames(0, 1).unwrap(),
        [[11.0 / 32768.0, -11.0 / 32768.0]]
    );
    assert_eq!(
        reader.path().as_os_str().as_encoded_bytes().as_ptr() as usize,
        expected.0
    );
    assert_eq!(reader.path_capacity_bytes(), expected.1);
}

#[cfg(unix)]
#[test]
fn non_utf8_success_and_failure_preserve_exact_path_bytes_and_allocation() {
    use std::os::unix::ffi::OsStringExt;
    let mut fixture = TempFile::create(&pcm16_wav(&[[11, -11]], false));
    let mut raw = fixture.path().as_os_str().as_encoded_bytes().to_vec();
    raw.extend_from_slice(b"-Case-\xff\xfe");
    let renamed = PathBuf::from(std::ffi::OsString::from_vec(raw));
    let original_identity = identity(&fixture.0);
    let original_bytes = fs::read(fixture.path()).unwrap();
    match fs::rename(fixture.path(), &renamed) {
        Ok(()) => fixture.0 = renamed,
        // Some macOS filesystems reject invalid UTF8 names. Accept only
        // the observed EILSEQ capability refusal, never another OS error.
        Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) => {
            assert_eq!(identity(&fixture.0), original_identity);
            assert_eq!(fs::read(fixture.path()).unwrap(), original_bytes);
            let mut valid = VerifiedWavReader::open(fixture.path(), limits(8)).unwrap();
            assert_eq!(
                valid.read_frames(0, 1).unwrap(),
                [[11.0 / 32768.0, -11.0 / 32768.0]]
            );
            drop(valid);
            for request_cancel in [false, true] {
                let path = owned_path(&renamed);
                let expected = identity(&path);
                let mut cancel_calls = 0;
                let failure = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || {
                    cancel_calls += 1;
                    request_cancel
                })
                .err()
                .unwrap();
                let (error, capacity) = recover(failure, &expected);
                assert!(
                    matches!(error, ImportError::Io(ref error) if error.raw_os_error() == Some(92))
                );
                // File::open precedes page-verification cancellation in both
                // legacy and owned APIs. This is IO refusal, not Cancelled.
                assert_eq!(cancel_calls, 0);
                assert!(!capacity.manifest_reservation_attempted);
                assert_eq!(capacity.manifest_slots, 0);
            }
            assert_eq!(identity(&fixture.0), original_identity);
            assert_eq!(fs::read(fixture.path()).unwrap(), original_bytes);
            let original_path = fixture.path().to_path_buf();
            drop(fixture); // Cleanup owns only the unchanged valid fixture.
            assert_eq!(
                fs::metadata(original_path).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
            return;
        }
        Err(error) => panic!("unexpected non-UTF8 fixture rename refusal: {error}"),
    }
    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let reader = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || false).unwrap();
    assert_eq!(
        reader.path().as_os_str().as_encoded_bytes().as_ptr() as usize,
        expected.0
    );
    assert_eq!(reader.path().as_os_str().as_encoded_bytes(), expected.2);
    assert_eq!(reader.path_capacity_bytes(), expected.1);
    let path = owned_path(fixture.path());
    let expected = identity(&path);
    let failure = VerifiedWavReader::open_owned_with_cancel(path, limits(8), || true)
        .err()
        .unwrap();
    let (error, capacity) = recover(failure, &expected);
    assert!(matches!(error, ImportError::Cancelled));
    assert!(capacity.manifest_slots >= 1);
}

#[test]
fn opening_capacity_arithmetic_refuses_overflow() {
    assert_eq!(
        VerifiedOpenCapacity {
            manifest_slots: usize::MAX,
            ..VerifiedOpenCapacity::default()
        }
        .manifest_bytes(),
        None
    );
    assert_eq!(
        VerifiedOpenCapacity {
            path_capacity_bytes: usize::MAX,
            manifest_slots: 1,
            manifest_reservation_attempted: true
        }
        .data_bytes(),
        None
    );
}
