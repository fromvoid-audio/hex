//! Real bank admission through the public native control and existing queues.
use super::*;
use crate::candidate_custody::granular_test_fixture::bank;
use hex_stream::{StreamConfig, StreamReadinessReservation};
fn ready(control: &mut Control, token: crate::AudioCandidateDomainToken) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(token)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => return,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected granular preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn two_real_granular_domains_stage_with_four_source_workers_and_allocation_free_callback_handoff() {
    let _serial = serial();
    let mut owner = candidate().expect("actual supported constructor fixture");
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let mut files = Vec::new();
    let mut tokens = Vec::new();
    for (index, mode) in [
        hex_project::WarpMode::Tones { grain_ms: 32 },
        hex_project::WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let (file, bank, metadata) = bank(mode, 71 + index as u64);
        files.push(file);
        let token = control
            .begin_audio_candidate_granular_domain_preparation(
                StreamConfig::with_pool_pages(4),
                vec![],
                bank,
                StreamReadinessReservation {
                    extents_per_role: 2,
                },
            )
            .unwrap();
        ready(&mut control, token);
        assert!(control.audio_candidate_domain_is_sealed(token));
        assert!(control.audio_candidate_domain_covers_source(token, &metadata));
        // The real publication ring has one slot: consume this exact endpoint
        // before polling the next domain. Every callback handoff stays measured.
        let measured = allocation_counter::measure(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        tokens.push(token);
    }
    for token in &tokens {
        assert!(control.audio_candidate_domain_endpoint_is_installed(*token));
    }
    // The candidate is resident, but its Root must cover both retained source
    // domains too. The old two-worker witness refused this real four-worker Root.
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
        .unwrap_or_else(|refused| panic!("resource witness refused: {}", refused.reason));
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let recovered = recover(&mut control, &ticket);
    assert_eq!(
        recovered.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(recovered);
    for token in tokens {
        assert!(control.retire_audio_candidate_domain(token));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let report = loop {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block());
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(token) {
                break report;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(report.page_buffers, 4);
        assert!(report.worker_joined && report.accounting_complete);
    }
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn granular_public_entry_refuses_nonnative_and_short_handle_roots_without_occupancy() {
    let _serial = serial();
    let (mut naked, callback) = channel(48_000).unwrap();
    let (_file, bank_a, _) = bank(hex_project::WarpMode::Tones { grain_ms: 32 }, 71);
    assert!(matches!(
        naked.begin_audio_candidate_granular_domain_preparation(
            StreamConfig::with_pool_pages(4),
            vec![],
            bank_a,
            StreamReadinessReservation {
                extents_per_role: 2
            }
        ),
        Err(crate::AudioCandidateDomainStartError::Busy(
            "candidate native custody unavailable"
        ))
    ));
    drop(naked);
    drop(callback);
    let mut owner = candidate().expect("actual supported constructor fixture");
    let mut limits = reservation(&mut owner);
    limits.handle_rows = 32;
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let before = observed_usage(&control);
    let (_file, bank_a, _) = bank(hex_project::WarpMode::Tones { grain_ms: 32 }, 72);
    assert!(matches!(
        control.begin_audio_candidate_granular_domain_preparation(
            StreamConfig::with_pool_pages(4),
            vec![],
            bank_a,
            StreamReadinessReservation {
                extents_per_role: 2
            }
        ),
        Err(crate::AudioCandidateDomainStartError::Busy(_))
    ));
    assert_eq!(observed_usage(&control), before);
    drop(owner);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
