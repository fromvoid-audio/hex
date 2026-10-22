//! Standalone registry-driven authoring report: no project, CPAL, GPUI or filesystem.
use hex_lattice::{
    NativeInstrumentRuntimeLimits,
    conformance::{check_effect, check_instrument, check_metadata},
};
use hex_lattice_contract::{
    NativeDeviceMetadata, NativeDeviceState, ParameterKind, ParameterValue,
    validated_state_parameters,
};

fn quoted(text: &str) -> String {
    let mut output = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}
fn value(value: ParameterValue) -> String {
    match value {
        ParameterValue::Float(value) => value.to_string(),
        ParameterValue::Bool(value) => value.to_string(),
        ParameterValue::Choice(value) => value.to_string(),
    }
}
fn record<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    class: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut parameters = Vec::new();
    for (descriptor, semantics) in validated_state_parameters(definition, state)? {
        let kind = match descriptor.kind {
            ParameterKind::Float { min, max, unit } => format!(
                "{{\"kind\":\"float\",\"min\":{min},\"max\":{max},\"unit\":{}}}",
                unit.map_or("null".to_owned(), quoted)
            ),
            ParameterKind::Bool => "{\"kind\":\"bool\"}".to_owned(),
            ParameterKind::Choice { labels } => format!(
                "{{\"kind\":\"choice\",\"labels\":[{}]}}",
                labels
                    .iter()
                    .map(|label| quoted(label))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        };
        let semantics=semantics.map_or("null".to_owned(),|s| {
            let increment=match s.stepping {hex_lattice_contract::ParameterStepping::Increment(v)=>v.to_string(),_=>"null".to_owned()};
            format!("{{\"access\":{},\"normalization\":{},\"stepping\":{},\"increment\":{increment},\"automation\":{},\"modulation\":{},\"smoothing\":{},\"change_impact\":{}}}",quoted(s.access.label()),quoted(s.normalization.label()),quoted(s.stepping.label()),quoted(s.automation.label()),quoted(s.modulation.label()),quoted(s.smoothing.label()),quoted(s.change_impact.label()))
        });
        parameters.push(format!("{{\"id\":{},\"label\":{},\"domain\":{kind},\"default\":{},\"current\":{},\"presentation\":{},\"enabled\":{},\"semantics\":{semantics}}}",
            descriptor.id.value(), quoted(descriptor.label), value(descriptor.default),
            value(definition.read_parameter(state, descriptor.id)?), quoted(&format!("{:?}",descriptor.presentation)),
            definition.parameter_enabled(state, descriptor.id)?));
    }
    Ok(format!(
        "{{\"type_id\":{},\"state_version\":{},\"state_bytes\":{},\"tested_class\":{},\"parameters\":[{}]}}",
        quoted(definition.type_id().as_str()?),
        state.state_version(),
        state.payload().len(),
        quoted(class),
        parameters.join(",")
    ))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for definition in hex_devices::native_registry()?.definitions() {
        let state = definition.default_state()?;
        check_metadata(*definition, &state)?;
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            check_effect(*definition, &state, rate)?;
        }
        records.push(record(*definition, &state, "effect")?);
    }
    for definition in hex_devices::native_instrument_registry()?.definitions() {
        let state = definition.default_state()?;
        check_metadata(*definition, &state)?;
        let mut class = None;
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            class = Some(check_instrument(
                *definition,
                &state,
                rate,
                NativeInstrumentRuntimeLimits {
                    max_pair_bytes: 2 * 1024 * 1024,
                    max_events_per_block: 259,
                    max_active_occurrences: 64,
                    max_tail_frames: u64::from(rate) * 600,
                },
            )?);
        }
        records.push(record(*definition, &state, &format!("{class:?}"))?);
    }
    let mut eq = hex_devices::eq::EqState::default();
    eq.add(hex_devices::eq::Node::new(
        hex_devices::eq::NodeId::new(7)?,
        1000.0,
        3.0,
        0,
    )?)?;
    let eq = eq.encode()?;
    check_metadata(&hex_devices::eq::EQ_DEFINITION, &eq)?;
    let populated = record(
        &hex_devices::eq::EQ_DEFINITION,
        &eq,
        "populated eq semantic fixture",
    )?;
    println!(
        "{{\"schema\":2,\"scope\":\"default-state bootstrap conformance; not native acceptance or reference parity\",\"sample_rates\":[8000,44100,48000,96000,192000],\"devices\":[{}],\"fixtures\":[{populated}]}}",
        records.join(",")
    );
    Ok(())
}
