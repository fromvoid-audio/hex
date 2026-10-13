use super::*;
use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
use hex_media::{VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use std::path::PathBuf;

fn inputs() -> Vec<SourceDefinition> {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let mut definitions = Vec::with_capacity(100);
    for (index, name) in ["UPPER/first.wav", "second.wav", ""]
        .into_iter()
        .enumerate()
    {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: if name.is_empty() {
                    "empty-path-fixture.wav"
                } else {
                    name
                }
                .into(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(91, asset),
            48_000,
            48_000,
            8192,
            8192,
            0,
            [index as u8; 32],
        )
        .unwrap();
        let mut path = PathBuf::with_capacity(4096);
        path.push(name);
        definitions.push(SourceDefinition::new(
            path,
            metadata,
            VerifiedWavReaderLimits::new(WavReaderLimits::new(8192, 8192, 128, 256), 4096),
        ));
    }
    definitions
}

fn reservations() -> CompactCopyReservations {
    // Explicit test-fixture reservations only, not application policy.
    CompactCopyReservations {
        max_copy_bytes: 8192,
        max_overlap_bytes: 65536,
        other_retained_bytes: 111,
        other_construction_bytes: 222,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Owners {
    pointer: usize,
    capacity: usize,
    paths: Vec<(usize, usize, PathBuf)>,
    metadata: Vec<StreamedSourceMetadata>,
    keys: Vec<StreamSourceKey>,
    reader_limits: Vec<VerifiedWavReaderLimits>,
}
fn owners(definitions: &Vec<SourceDefinition>) -> Owners {
    Owners {
        pointer: definitions.as_ptr() as usize,
        capacity: definitions.capacity(),
        paths: definitions
            .iter()
            .map(|d| {
                (
                    d.path.as_os_str().as_encoded_bytes().as_ptr() as usize,
                    d.path.capacity(),
                    d.path.clone(),
                )
            })
            .collect(),
        metadata: definitions.iter().map(|d| d.metadata.clone()).collect(),
        keys: definitions.iter().map(|d| d.key).collect(),
        reader_limits: definitions.iter().map(|d| d.reader_limits).collect(),
    }
}

#[test]
fn compact_preserves_order_metadata_and_original_overreservations() {
    let original = inputs();
    let identity = owners(&original);
    let logical_bytes = original.iter().map(|d| d.path.as_os_str().len()).sum();
    let mut input = CompactSourceInput::new(original);
    let report = input
        .try_compact(
            StreamConfig {
                max_catalog_path_bytes: logical_bytes,
                ..StreamConfig::with_pool_pages(4)
            },
            reservations(),
        )
        .unwrap();
    assert_eq!(owners(&input.originals), identity);
    let compact = input.compact_sources().unwrap();
    assert_eq!(compact.len(), input.originals().len());
    for (copy, original) in compact.iter().zip(input.originals()) {
        assert_eq!(copy.path, original.path);
        assert_eq!(copy.key, original.key);
        assert_eq!(copy.metadata, original.metadata);
        assert_eq!(copy.reader_limits, original.reader_limits);
        assert!(copy.path.capacity() < original.path.capacity());
    }
    assert_eq!(report.originals.source_slots.capacity, identity.capacity);
    assert_eq!(
        report.compact.source_slots.capacity,
        input.compact.as_ref().unwrap().capacity()
    );
    assert_eq!(
        report.compact.path_capacity_bytes,
        compact.iter().map(|d| d.path.capacity()).sum()
    );
    assert_eq!(
        report.originals.path_capacity_bytes,
        input.originals().iter().map(|d| d.path.capacity()).sum()
    );
    assert_eq!(
        report.compact.logical_path_bytes,
        report.originals.logical_path_bytes
    );
    assert!(report.compact.bytes().unwrap() < report.originals.bytes().unwrap());
    assert_eq!(
        report.overlap_bytes(),
        Some(report.originals.bytes().unwrap() + report.compact.bytes().unwrap() + 333)
    );
    assert_eq!(input.report(), Some(report));
    assert_eq!(
        input.try_compact(StreamConfig::default(), reservations()),
        Err(CompactCopyError::AlreadyCompacted)
    );
    assert_eq!(input.report(), Some(report));
    assert_eq!(owners(&input.into_originals()), identity);
}

#[cfg(unix)]
#[test]
fn compact_preserves_non_utf8_os_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut definitions = inputs();
    let mut bytes = Vec::with_capacity(2048);
    bytes.extend_from_slice(b"Case/\xff\xfe.wav");
    definitions[0].path = OsString::from_vec(bytes).into();
    let identity = owners(&definitions);
    let mut input = CompactSourceInput::new(definitions);
    input
        .try_compact(StreamConfig::default(), reservations())
        .unwrap();
    assert_eq!(
        input.compact_sources().unwrap()[0]
            .path
            .as_os_str()
            .as_bytes(),
        b"Case/\xff\xfe.wav"
    );
    assert_eq!(owners(&input.into_originals()), identity);
}

#[test]
fn every_copy_reservation_failure_reclaims_partial_copy_and_preserves_owners() {
    // Vec plus two nonempty path allocations; empty path needs no allocation.
    for fail_at in 0..3 {
        let originals = inputs();
        let identity = owners(&originals);
        let mut input = CompactSourceInput::new(originals);
        let mut attempt = 0;
        let result = input.copy_with(StreamConfig::default(), reservations(), || {
            let current = attempt;
            attempt += 1;
            if current == fail_at {
                Err(CompactCopyError::AllocationFailed)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(CompactCopyError::AllocationFailed));
        assert_eq!(attempt, fail_at + 1);
        assert!(input.report().is_none());
        assert!(input.compact_sources().is_none());
        assert_eq!(owners(&input.originals), identity);
        // Retrying has no stale partial state.
        input
            .try_compact(StreamConfig::default(), reservations())
            .unwrap();
        assert_eq!(owners(&input.into_originals()), identity);
    }
}

fn refusal(
    definitions: Vec<SourceDefinition>,
    config: StreamConfig,
    reservations: CompactCopyReservations,
    expected: CompactCopyError,
) {
    let identity = owners(&definitions);
    let mut input = CompactSourceInput::new(definitions);
    let mut allocations = 0;
    assert_eq!(
        input.copy_with(config, reservations, || {
            allocations += 1;
            Ok(())
        }),
        Err(expected)
    );
    assert_eq!(allocations, 0, "borrowed preflight must precede copying");
    assert!(input.compact_sources().is_none());
    assert!(input.report().is_none());
    assert_eq!(owners(&input.into_originals()), identity);
}

#[test]
fn validation_refuses_without_mutating_or_allocating_copy() {
    let config = StreamConfig::default();
    refusal(
        inputs(),
        StreamConfig {
            page_buffers: 1,
            ..config
        },
        reservations(),
        CompactCopyError::InvalidConfiguration,
    );
    refusal(
        inputs(),
        StreamConfig {
            max_sources: 2,
            ..config
        },
        reservations(),
        CompactCopyError::SourceLimit,
    );
    refusal(
        inputs(),
        StreamConfig {
            max_reader_scratch_bytes: hex_media::VERIFIED_WAV_PAGE_BYTES,
            ..config
        },
        reservations(),
        CompactCopyError::ReaderScratchLimit,
    );
    refusal(
        inputs(),
        StreamConfig {
            max_catalog_path_bytes: 1,
            ..config
        },
        reservations(),
        CompactCopyError::LogicalPathLimit,
    );
    let mut mismatched = inputs();
    mismatched[0].key = mismatched[1].key;
    refusal(
        mismatched,
        config,
        reservations(),
        CompactCopyError::KeyMetadataMismatch,
    );
    let mut duplicates = inputs();
    duplicates[1].key = duplicates[0].key;
    duplicates[1].metadata = duplicates[0].metadata.clone();
    refusal(
        duplicates,
        config,
        reservations(),
        CompactCopyError::DuplicateKey,
    );
}

#[test]
fn reservations_refuse_excess_original_or_copy_capacity_and_checked_overflow() {
    let r = reservations();
    refusal(
        inputs(),
        StreamConfig::default(),
        CompactCopyReservations {
            max_copy_bytes: 0,
            ..r
        },
        CompactCopyError::ReservationExceeded,
    );
    refusal(
        inputs(),
        StreamConfig::default(),
        CompactCopyReservations {
            max_overlap_bytes: 1,
            ..r
        },
        CompactCopyError::ReservationExceeded,
    );
    refusal(
        inputs(),
        StreamConfig::default(),
        CompactCopyReservations {
            other_retained_bytes: usize::MAX,
            ..r
        },
        CompactCopyError::ArithmeticOverflow,
    );
    refusal(
        inputs(),
        StreamConfig::default(),
        CompactCopyReservations {
            other_construction_bytes: usize::MAX,
            ..r
        },
        CompactCopyError::ArithmeticOverflow,
    );
    let mut input = CompactSourceInput::new(inputs());
    let report = input.try_compact(StreamConfig::default(), r).unwrap();
    assert_eq!(
        CompactCopyReport {
            reservations: CompactCopyReservations {
                max_copy_bytes: report.compact.bytes().unwrap() - 1,
                ..r
            },
            ..report
        }
        .check(),
        Err(CompactCopyError::ReservationExceeded)
    );
    assert_eq!(
        CompactCopyReport {
            reservations: CompactCopyReservations {
                max_overlap_bytes: report.overlap_bytes().unwrap() - 1,
                ..r
            },
            ..report
        }
        .check(),
        Err(CompactCopyError::ReservationExceeded)
    );
    assert_eq!(
        InputAllocationFootprint {
            source_slots: AllocationCapacity {
                capacity: usize::MAX,
                element_bytes: 2
            },
            ..InputAllocationFootprint::default()
        }
        .bytes(),
        None
    );
    assert_eq!(
        InputAllocationFootprint {
            source_slots: AllocationCapacity {
                capacity: 1,
                element_bytes: usize::MAX
            },
            path_capacity_bytes: 1,
            logical_path_bytes: 0,
        }
        .bytes(),
        None
    );
}

#[test]
fn empty_copy_completes_without_reserving_and_charges_original_capacity() {
    let originals = Vec::with_capacity(7);
    let identity = owners(&originals);
    let mut input = CompactSourceInput::new(originals);
    let report = input
        .copy_with(StreamConfig::default(), reservations(), || {
            panic!("no allocation")
        })
        .unwrap();
    assert_eq!(report.compact.bytes(), Some(0));
    assert_eq!(report.originals.source_slots.capacity, 7);
    assert_eq!(owners(&input.into_originals()), identity);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn ownership_move_refusal_and_control_reclamation_have_exact_allocation_balance() {
    use allocation_counter::measure;
    let mut originals = Some(inputs());
    let identity = owners(originals.as_ref().unwrap());
    let mut stage = None;
    let moving = measure(|| stage = Some(CompactSourceInput::new(originals.take().unwrap())));
    assert_eq!(moving.count_total, 0);
    assert_eq!(moving.count_current, 0);
    assert_eq!(moving.bytes_total, 0);
    assert_eq!(moving.bytes_current, 0);
    let mut input = stage.unwrap();
    let mut result = None;
    let refused = measure(|| {
        result = Some(input.try_compact(
            StreamConfig::default(),
            CompactCopyReservations {
                max_copy_bytes: 0,
                ..reservations()
            },
        ));
    });
    assert_eq!(result.unwrap(), Err(CompactCopyError::ReservationExceeded));
    assert_eq!(refused.count_total, 0);
    assert_eq!(refused.count_current, 0);
    assert_eq!(refused.bytes_total, 0);
    assert_eq!(refused.bytes_current, 0);
    for fail_at in 0..3 {
        let mut attempt = 0;
        let failure = measure(|| {
            result = Some(
                input.copy_with(StreamConfig::default(), reservations(), || {
                    let current = attempt;
                    attempt += 1;
                    if current == fail_at {
                        Err(CompactCopyError::AllocationFailed)
                    } else {
                        Ok(())
                    }
                }),
            );
        });
        assert_eq!(result.unwrap(), Err(CompactCopyError::AllocationFailed));
        assert_eq!(failure.count_total, fail_at);
        assert_eq!(
            failure.count_current, 0,
            "all partial staging allocations reclaimed"
        );
        assert_eq!(failure.bytes_current, 0);
    }
    let copied =
        measure(|| result = Some(input.try_compact(StreamConfig::default(), reservations())));
    let report = result.unwrap().unwrap();
    println!(
        "compact input reserved data capacities: {report:?}; overlap={:?}",
        report.overlap_bytes()
    );
    assert_eq!(copied.count_total, 3); // one Vec, two nonempty raw paths
    assert_eq!(copied.count_current, 3);
    assert_eq!(copied.bytes_total as usize, report.compact.bytes().unwrap());
    assert_eq!(
        copied.bytes_current as usize,
        report.compact.bytes().unwrap()
    );
    let reclaimed = measure(|| originals = Some(input.into_originals()));
    assert_eq!(reclaimed.count_total, 0);
    assert_eq!(reclaimed.count_current, -3);
    assert_eq!(reclaimed.bytes_current, -copied.bytes_current);
    assert_eq!(owners(originals.as_ref().unwrap()), identity);
    let final_drop = measure(|| drop(originals.take()));
    assert_eq!(final_drop.count_total, 0);
    assert_eq!(final_drop.count_current, -4); // original Vec + three reserved paths
    assert_eq!(
        final_drop.bytes_current,
        -(report.originals.bytes().unwrap() as i64)
    );
}
