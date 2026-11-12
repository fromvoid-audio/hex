//! Native control returns exact verified-file compiler facts without claiming a page loan.
use super::*;
use hex_session::service::ResidentFileInput;

#[test]
fn native_control_binds_actual_verified_file_to_ordinary_catalog_metadata() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = Harness::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let event = h.run(Request::Import(files.wav(48_000, 128)));
    let source = event.state.canonical_audio_source.as_ref().unwrap();
    let (asset, binding) = source.required_sources().next().unwrap();
    let AudioSourceBinding::VerifiedFile(file) = binding else {
        panic!("verified source")
    };
    let (sender, _receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(sender);
    let rows = [(asset, file)];
    let metadata = sink
        .prepare_resident_file_metadata(ResidentFileInput {
            context: source.context(),
            semantic_source_epoch: 1,
            files: &rows,
        })
        .unwrap()
        .unwrap();
    let catalog = metadata.catalog_generation();
    assert!(catalog > 0);
    assert_eq!(
        metadata.sources()[&asset].as_ref(),
        &file.source_metadata(asset, catalog).unwrap()
    );
    assert_eq!(
        source.context(),
        event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
    );
}
