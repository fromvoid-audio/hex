#![cfg(feature = "ui-test-support")]
use gpui::TestAppContext;

#[gpui::test]
fn delivered_receipt_pressure_retains_the_take_until_discard(cx: &mut TestAppContext) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("hex-midi-pressure-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let reservation = hex_session::SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
            1024 * 1024,
        ),
        max_retained_sources: 16,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    };
    let (mut output, mut backend) = hex_audio::worker_fixture::open(48_000).unwrap();
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    hex_ui::exercise_midi_receipt_pressure(
        cx,
        root.clone(),
        reservation,
        output,
        |frames, host| backend.render_direct(frames, host),
    );
    backend.close();
    std::fs::remove_dir_all(root).unwrap();
}
