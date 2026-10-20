//! Sampler scalar controls. The caller ends its snapshot loan before rendering.
//! No source, authored state, waveform or playback owner is retained here.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::DeviceId;

// Existing Sampler metadata IDs; no device/DSP dependency enters hex-ui.
const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
const MODE: ParameterId = ParameterId::from_static(1);
const REVERSE: ParameterId = ParameterId::from_static(2);
const ROOT: ParameterId = ParameterId::from_static(3);
const TRANSPOSE: ParameterId = ParameterId::from_static(4);
const DETUNE: ParameterId = ParameterId::from_static(5);
const GAIN: ParameterId = ParameterId::from_static(6);
const PAN: ParameterId = ParameterId::from_static(7);
const VELOCITY: ParameterId = ParameterId::from_static(8);
const ATTACK: ParameterId = ParameterId::from_static(9);
const DECAY: ParameterId = ParameterId::from_static(10);
const SUSTAIN: ParameterId = ParameterId::from_static(11);
const RELEASE: ParameterId = ParameterId::from_static(12);
const VOICES: ParameterId = ParameterId::from_static(13);
const LOOP: ParameterId = ParameterId::from_static(14);
const START: ParameterId = ParameterId::from_static(17);
const END: ParameterId = ParameterId::from_static(18);
const LOOP_START: ParameterId = ParameterId::from_static(19);
const LOOP_END: ParameterId = ParameterId::from_static(20);

type Parameters = [NativeDeviceParameterView; 43];
type Sections = [Div; 4];

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::SAMPLER_FRAME_WIDTH;

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    parameters: &Parameters,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: TYPE_ID,
    };
    let [source, pitch, level, envelope] =
        sections(parameters, |parameter, presentation, display| {
            parameter_control_with_presentation(
                view,
                cx,
                device,
                identity,
                parameter,
                presentation,
                display,
            )
        });
    crate::device_composition::sampler(source, pitch, level, envelope)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

/// The same production composition accepts already-bound controls in leaf tests.
/// Its input consists only of copied parameter facts, never a snapshot loan.
pub(super) fn sections(
    parameters: &Parameters,
    mut bind: impl FnMut(&NativeDeviceParameterView, ControlPresentation, ControlText) -> Div,
) -> Sections {
    let mut control = |id, presentation, codec| match parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == id)
    {
        Some(parameter) => bind(parameter, presentation, ControlText { label: None, codec }),
        None => div().child(crate::theme::text("parameter unavailable")),
    };
    let start = control(
        START,
        ControlPresentation::CompactDial,
        Some(NumericDisplayCodec::Number),
    );
    let end = control(
        END,
        ControlPresentation::CompactDial,
        Some(NumericDisplayCodec::Number),
    );
    let reverse = control(REVERSE, ControlPresentation::CompactButton, None);
    let classic = parameters.iter().any(|parameter| {
        parameter.descriptor.id == MODE && parameter.value == ParameterValue::Choice(0)
    });
    let mut switches = crate::device_composition::sampler_switches().children([reverse]);
    if classic {
        switches = switches.child(control(LOOP, ControlPresentation::CompactButton, None));
    }
    let source_flags = crate::device_composition::sampler_flag_row().children([start, end]);
    let mut source = crate::device_composition::sampler_section("source · frames")
        .children([source_flags, switches]);
    if classic {
        let loop_start = control(
            LOOP_START,
            ControlPresentation::CompactDial,
            Some(NumericDisplayCodec::Number),
        );
        let loop_end = control(
            LOOP_END,
            ControlPresentation::CompactDial,
            Some(NumericDisplayCodec::Number),
        );
        let loop_flags =
            crate::device_composition::sampler_flag_row().children([loop_start, loop_end]);
        source = source.children([loop_flags]);
    }
    let pitch = crate::device_composition::sampler_section("pitch")
        .child(control(ROOT, ControlPresentation::Standard, None))
        .child(control(TRANSPOSE, ControlPresentation::Standard, None))
        .child(control(DETUNE, ControlPresentation::Standard, None))
        .child(control(VOICES, ControlPresentation::Standard, None));
    let level = crate::device_composition::sampler_section("level")
        .child(control(
            GAIN,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::AmplitudeDb),
        ))
        .child(control(
            PAN,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Balance),
        ))
        .child(control(
            VELOCITY,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Percent),
        ));
    let envelope = crate::device_composition::sampler_section("amplitude")
        .child(control(
            ATTACK,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Seconds),
        ))
        .child(control(
            DECAY,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Seconds),
        ))
        .child(control(
            SUSTAIN,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Percent),
        ))
        .child(control(
            RELEASE,
            ControlPresentation::Standard,
            Some(NumericDisplayCodec::Seconds),
        ));
    [source, pitch, level, envelope]
}
