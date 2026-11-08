//! One selected input scheduler feeds the existing source receiver. Scheduling
//! happens before source synthesis; no keyboard-owned mutable pair is rendered.
use super::*;

impl SessionPlayback {
    pub(super) fn source_input_receiver(&self) -> Option<usize> {
        let keyboard = self
            .computer_keyboard
            .as_ref()
            .filter(|keyboard| keyboard.is_source_input())?;
        let contributor = self
            .routes
            .iter()
            .position(|route| route.id == keyboard.track())?;
        if !self.revision.source_routing().contributes(contributor) {
            return None;
        }
        self.revision.source_routing().receiver_index(contributor)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_source_live_input(
    keyboard: Option<&mut crate::computer_keyboard::ComputerKeyboardRuntime>,
    source: Option<&mut MutableInstrumentTrackRuntime>,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    authored: usize,
    performance: u64,
    output: &mut [Frame],
    timing: &crate::prepared_tempo::SessionTimingView<'_>,
    playing: bool,
    parameter_values: Option<&dyn hex_lattice::NativeParameterAutomation>,
) -> Result<(), SessionPlaybackError> {
    if source.is_some() && output.len() > 256 {
        return Err(SessionPlaybackError::InvalidCommand);
    }
    let keyboard = keyboard.ok_or(SessionPlaybackError::InvalidCommand)?;
    let input = keyboard
        .source_input_id()
        .ok_or(SessionPlaybackError::InvalidCommand)?;
    if let Some(source) = source.as_ref() {
        keyboard.set_source_capacity(source.live_input_voice_capacity(input, authored));
    }
    let release = keyboard.take_source_input_release();
    let clock = keyboard
        .musical_clock(timing, performance, playing)
        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
    keyboard
        .render(output, Some(&clock))
        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
    if let Some(source) = source {
        if release {
            source
                .release_live_input(input)
                .map_err(|error| error.render_error())?;
        }
        source
            .stage_live_events(input, keyboard.source_input_events())
            .map_err(|error| error.render_error())?;
        if !playing {
            onsets.clear();
            let source_clock = source
                .source_monitor_clock(
                    timing
                        .frame_to_beat(performance)
                        .map_err(|_| SessionPlaybackError::TempoConversion)?
                        .raw(),
                    timing
                        .tempo_micro_bpm_at_frame(performance)
                        .ok_or(SessionPlaybackError::TempoConversion)?,
                    timing.sample_rate(),
                )
                .map_err(|error| error.render_error())?;
            source
                .process_source_segment(
                    performance,
                    output,
                    onsets,
                    &[],
                    events,
                    Some(&source_clock),
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    None,
                    parameter_values,
                )
                .map_err(|error| error.render_error())?;
        }
    }
    Ok(())
}
