use super::*;
#[test]
fn foreign_packet_field_refuses_with_same_valid_scalar_and_project_endpoint_fields() {
    let _serial = crate::prepared::candidate_tests::serial();
    let root_a = crate::candidate_custody::CandidateRoot::new(3).unwrap();
    let root_b = crate::candidate_custody::CandidateRoot::new(3).unwrap();
    let parent_a =
        crate::native_callback_owner::NativeRetirement::begin_construction(root_a.clone());
    let parent_b =
        crate::native_callback_owner::NativeRetirement::begin_construction(root_b.clone());
    let stamp = SpectrumStamp {
        target: SpectrumTargetId(1),
        generation: 1,
        sample_rate_hz: 48000,
        applied_revision: 1,
    };
    let (mut capture_a, reader_a, mut reaper_a) =
        crate::spectrum_worker::spawn_worker_with_parent(stamp, true, None, Some(&parent_a))
            .unwrap();
    let (mut capture_b, reader_b, mut reaper_b) =
        crate::spectrum_worker::spawn_worker_with_parent(stamp, true, None, Some(&parent_b))
            .unwrap();
    let measured = allocation_counter::measure(|| {
        assert!(root_a.matches_spectrum_capture(Some(&capture_a)));
        std::mem::swap(
            &mut capture_a.project.as_mut().unwrap().packet,
            &mut capture_b.project.as_mut().unwrap().packet,
        );
        assert!(!root_a.matches_spectrum_capture(Some(&capture_a)));
        assert!(!root_b.matches_spectrum_capture(Some(&capture_b)));
        std::mem::swap(
            &mut capture_a.project.as_mut().unwrap().packet,
            &mut capture_b.project.as_mut().unwrap().packet,
        );
        assert!(root_a.matches_spectrum_capture(Some(&capture_a)));
        assert!(root_b.matches_spectrum_capture(Some(&capture_b)));
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    drop(capture_a);
    drop(capture_b);
    reaper_a.advance_shutdown_on_control().unwrap();
    reaper_a
        .finalize_captured_frame_on_control(&parent_a)
        .unwrap();
    reaper_b.advance_shutdown_on_control().unwrap();
    reaper_b
        .finalize_captured_frame_on_control(&parent_b)
        .unwrap();
    drop((reader_a, reader_b));
    assert_eq!(
        root_a.test_spectrum_packet_facts().unwrap().1,
        PROJECT_PACKET_OWNERS
    );
    assert_eq!(
        root_b.test_spectrum_packet_facts().unwrap().1,
        PROJECT_PACKET_OWNERS
    );
}
