use super::*;

fn fixture_group() -> (Project, Vst3GroupTarget, DraftProvenance, CaptureClock) {
    let (source, target, provenance) = super::vst3_tests::vst3_fixture();
    let DraftTarget::Device { device, .. } = target else {
        panic!("vst3 fixture must own its device");
    };
    let group = Vst3GroupTarget::new(device, [9; 16], &[0, 11]).unwrap();
    let capture = CaptureClock {
        sample_rate: 48_000,
        loop_region: Some(hex_project::LoopRegion::new(64, 128, true).unwrap()),
    };
    (source, group, provenance, capture)
}

fn events() -> [GroupDraftEvent; 5] {
    let row =
        |ordinal, parameter, sdk_gesture, sdk_sequence, offset, content_frame, application| {
            GroupDraftEvent {
                ordinal,
                parameter,
                sdk_sequence,
                sdk_gesture,
                clock: DraftClock {
                    performance_frame: 100 + offset,
                    content_frame,
                    loop_pass: u64::from(offset >= 2),
                },
                application,
            }
        };
    [
        row(1, 0, 10, 20, 0, 126, GroupApplication::Value(1.0 / 3.0)),
        row(2, 11, 11, 21, 0, 126, GroupApplication::Value(0.75)),
        row(3, 11, 11, 23, 2, 64, GroupApplication::Value(0.625)),
        // Earlier SDK End than the already delivered Perform on the other
        // target. Durability orders by Main application, not SDK callback time.
        row(4, 0, 10, 22, 3, 65, GroupApplication::Restored),
        row(5, 11, 11, 24, 4, 66, GroupApplication::Restored),
    ]
}

#[test]
fn group_reopens_exact_clocked_values_and_only_completes_as_one_take() {
    let (source, group, provenance, capture) = fixture_group();
    let directory = TestDir::new();
    let path = directory.path();
    let wrong_class = Vst3GroupTarget::new(group.device(), [8; 16], group.ids()).unwrap();
    assert!(
        DraftWriter::create_with_clock(
            &path,
            &source,
            DraftTarget::Vst3Group(wrong_class),
            provenance,
            capture,
        )
        .is_err()
    );
    assert!(!path.exists());
    let mut writer = DraftWriter::create_with_clock(
        &path,
        &source,
        DraftTarget::Vst3Group(group),
        provenance,
        capture,
    )
    .unwrap();
    assert!(writer.append(&[]).is_err());
    let events = events();
    writer.append_group(&events[..2]).unwrap();
    let progress = writer.progress();
    assert!(
        writer
            .finish(DraftTerminal::Finished(events[1].clock))
            .is_err()
    );
    let wrong_clock = GroupDraftEvent {
        clock: DraftClock {
            content_frame: 67,
            ..events[3].clock
        },
        ..events[3]
    };
    assert!(writer.append_group(&[events[2], wrong_clock]).is_err());
    assert_eq!(writer.progress(), progress);
    writer.append_group(&events[2..]).unwrap();
    writer
        .finish(DraftTerminal::Finished(events[4].clock))
        .unwrap();
    drop(writer);

    let original = fs::read(&path).unwrap();
    assert_eq!(u32_at(&original, 8), VST3_GROUP_VERSION);
    let ordinary = DraftReader::open(&path, limits()).unwrap();
    assert!(ordinary.into_completed().is_err());
    let mut complete = CompletedGroupedDraft::open(&path, limits(), 5).unwrap();
    assert_eq!(complete.target(), group);
    assert_eq!(complete.source(), &source);
    assert_eq!(complete.provenance(), provenance);
    assert_eq!(complete.capture_clock(), Some(capture));
    assert_eq!(complete.end_clock(), events[4].clock);
    assert_eq!(complete.event_count(), 5);
    let mut page = [None; 4];
    assert_eq!(complete.read_events(2, &mut page).unwrap(), 3);
    assert_eq!(
        page,
        [Some(events[2]), Some(events[3]), Some(events[4]), None]
    );
    assert_eq!(complete.read_events(0, &mut page).unwrap(), 4);
    assert_eq!(
        page,
        [
            Some(events[0]),
            Some(events[1]),
            Some(events[2]),
            Some(events[3])
        ]
    );
    assert!(CompletedGroupedDraft::open(&path, limits(), 4).is_err());
    drop(complete);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn grouped_torn_batch_copies_its_prefix_without_inventing_a_terminal() {
    let (source, group, provenance, capture) = fixture_group();
    let directory = TestDir::new();
    let path = directory.path();
    let copied = path.with_extension("copied.hexadft");
    let target = DraftTarget::Vst3Group(group);
    let mut writer =
        DraftWriter::create_with_clock(&path, &source, target, provenance, capture).unwrap();
    let rows = events();
    writer.append_group(&rows[..2]).unwrap();
    let prefix = writer.durable_prefix_identity();
    let mut calls = 0;
    let refused = writer
        .append_group_with(
            &rows[2..4],
            |file, bytes| {
                calls += 1;
                if calls == 1 {
                    return file.write_all(bytes);
                }
                file.write_all(&bytes[..12])?;
                Err(io::Error::other("interrupted group append"))
            },
            File::sync_all,
        )
        .unwrap_err();
    assert_eq!(refused.progress.accepted_events, 4);
    assert_eq!(refused.progress.durable_events, 2);
    assert!(writer.append_group(&rows[4..]).is_err());
    drop(writer);

    let original = fs::read(&path).unwrap();
    let mut reader = DraftReader::open(&path, limits()).unwrap();
    assert!(reader.matches_prefix(prefix).unwrap());
    assert_eq!(reader.progress().durable_events, 3);
    assert_eq!(
        reader.outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::TruncatedRecord
        }
    );
    let mut page = [None; 4];
    assert_eq!(reader.read_group_events(0, &mut page).unwrap(), 3);
    assert_eq!(page, [Some(rows[0]), Some(rows[1]), Some(rows[2]), None]);
    let mut copy = DraftWriter::create_with_clock(
        &copied,
        reader.source(),
        reader.target(),
        reader.provenance(),
        reader.capture_clock().unwrap(),
    )
    .unwrap();
    copy.append_group(&page[..2].iter().flatten().copied().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(copy.durable_prefix_identity(), prefix);
    copy.append_group(&rows[2..3]).unwrap();
    let copied_prefix = copy.durable_prefix_identity();
    assert!(copy.finish(DraftTerminal::Finished(rows[2].clock)).is_err());
    copy.finish(DraftTerminal::Interrupted).unwrap();
    drop(copy);
    assert!(reader.into_completed_group().is_err());
    let reopened = DraftReader::open(&copied, limits()).unwrap();
    assert_eq!(reopened.progress().durable_events, 3);
    assert_eq!(reopened.outcome(), DraftOutcome::Interrupted);
    assert!(reopened.into_completed_group().is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    // A structurally valid and correctly chained finish cannot make an
    // incomplete member set appear adoptable.
    let mut false_finish = fs::read(&copied).unwrap();
    let start = false_finish.len() - RECORD_BYTES;
    false_finish[start..].copy_from_slice(&terminal_record(
        DraftTerminal::Finished(rows[2].clock),
        copied_prefix.chain,
    ));
    let forged = path.with_extension("incomplete-finish.hexadft");
    fs::write(&forged, false_finish).unwrap();
    let damaged = DraftReader::open(&forged, limits()).unwrap();
    assert_eq!(
        damaged.outcome(),
        DraftOutcome::Damaged {
            reason: DraftDamage::InvalidRecord
        }
    );
    assert!(damaged.into_completed_group().is_err());
}
