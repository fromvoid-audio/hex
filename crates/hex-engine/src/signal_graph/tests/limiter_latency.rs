use super::*;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

#[test]
fn limiter_lookahead_is_reported_and_compensated_in_actual_rendering() {
    let mut fixture = fixture(&[&[]], 32);
    let kind = DeviceTypeId::from_static("hex.limiter");
    let mut state =
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Effect, kind)
            .expect("registered limiter defaults");
    state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Effect,
        &state,
        ParameterId::from_static(6),
        ParameterValue::Choice(2),
    )
    .expect("select six millisecond lookahead");
    fixture
        .editor
        .execute(Command::InsertDevice {
            track: fixture.tracks[0],
            before: None,
            state,
        })
        .expect("insert limiter on the audio track");

    // Use the production registry; the surrounding delay fixture's prepare()
    // helper deliberately registers only its synthetic delay provider.
    let revision = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &fixture.media,
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepare actual registered native limiter graph"),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert_eq!(latency, 288);

    let rendered = session(
        Arc::clone(&revision),
        fixture.samples.len() + latency as usize,
        63,
    );
    assert!(
        rendered[..latency as usize]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(&rendered[latency as usize..], fixture.samples.as_slice());
}
