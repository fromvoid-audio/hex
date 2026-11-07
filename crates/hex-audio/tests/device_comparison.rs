//! Actual callback A/B handoff; no audio device, filesystem or UI.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    NativeDeviceClass, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, DeviceComparisonAction as Action, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};

/// Independent model of the host's static matrix ramp (owner:
/// `hex_lattice::matrix_ramp`) for Utility's linear gain. A changed matrix
/// across a compatible handoff ramps linearly from the old effective gain over
/// 5 ms rounded up; frame k of a ramp applies elapsed k + 1 and the final frame
/// applies the target exactly. An unchanged matrix continues its trajectory.
#[derive(Clone, Copy)]
struct GainRamp {
    target: f32,
    from: f32,
    elapsed: u32,
    total: u32,
}
impl GainRamp {
    const TOTAL: u32 = 48_000_u32.div_ceil(200);
    fn at(self, elapsed: u32) -> f32 {
        let position = f64::from(elapsed) / f64::from(self.total);
        let (from, to) = (f64::from(self.from), f64::from(self.target));
        (from + (to - from) * position) as f32
    }
    fn current(self) -> f32 {
        if self.elapsed < self.total {
            self.at(self.elapsed)
        } else {
            self.target
        }
    }
    fn handoff(self, target: f32) -> Self {
        if target == self.target {
            return self;
        }
        let from = self.current();
        let total = if from == target { 0 } else { Self::TOTAL };
        Self {
            target,
            from,
            elapsed: 0,
            total,
        }
    }
    /// Gain for the next frame; the second value marks the exact final frame.
    fn next(&mut self) -> (f32, bool) {
        if self.elapsed >= self.total {
            return (self.target, false);
        }
        self.elapsed += 1;
        if self.elapsed < self.total {
            (self.at(self.elapsed), false)
        } else {
            (self.target, true)
        }
    }
}

fn prepare(
    editor: &Editor,
    table: &Arc<SineTable>,
) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    Ok(Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::clone(table),
        None,
    )?))
}

#[test]
fn track_effect_comparison_keeps_held_pcm_transport_and_callback_allocation_zero()
-> Result<(), Box<dyn Error>> {
    callback_comparison(false)
}
#[test]
fn return_effect_comparison_keeps_held_pcm_transport_and_callback_allocation_zero()
-> Result<(), Box<dyn Error>> {
    callback_comparison(true)
}

fn callback_comparison(on_return: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 32)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "held comparison".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("track fixture".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(16)?,
    })?
    else {
        return Err("clip fixture".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(16)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    let a = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.utility"),
    )?;
    let inserted = if on_return {
        let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
            name: "comparison return".into(),
        })?
        else {
            return Err("return fixture".into());
        };
        editor.execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })?;
        editor.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: a.clone(),
        })?
    } else {
        editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: a.clone(),
        })?
    };
    let Created::Device(device) = inserted else {
        return Err("effect fixture".into());
    };
    let b = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &a,
        ParameterId::from_static(1),
        ParameterValue::Float(0.25),
    )?;
    editor.execute(Command::CompareDevice {
        device,
        expected: hex_project::DeviceVariant::A,
        action: Action::Switch,
    })?;
    editor.execute(Command::ReplaceDeviceState { device, state: b })?;
    editor.execute(Command::CompareDevice {
        device,
        expected: hex_project::DeviceVariant::B,
        action: Action::Switch,
    })?;
    let table = Arc::new(SineTable::new());
    let revision = prepare(&editor, &table)?;
    // This oracle is never replaced and never runs the inheritance algorithm.
    // Utility's independent linear gain supplies an analytical amplitude oracle.
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(revision)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "initial publication")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "initial play")?;
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut actual)?;
    oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1e-5));
    control.reclaim();
    let mut ramp = GainRamp {
        target: 1.0,
        from: 1.0,
        elapsed: 0,
        total: 0,
    };
    let mut completed = 0;
    // None is an ordinary active parameter edit to author a different A again.
    // Copy has the same expected amplitude before/after: it must not change sound.
    for (index, (action, gain)) in [
        (Some(Action::Switch), 0.25),
        (Some(Action::CopyToInactive), 0.25),
        (Some(Action::Switch), 0.25),
        (None, 0.75),
        (Some(Action::Switch), 0.25),
        (Some(Action::Switch), 0.75),
        (Some(Action::CopyToInactive), 0.75),
    ]
    .into_iter()
    .enumerate()
    {
        let device_state = editor
            .project()
            .device(device)
            .ok_or("missing comparison device")?;
        let command = if let Some(action) = action {
            Command::CompareDevice {
                device,
                expected: device_state.active_variant(),
                action,
            }
        } else {
            Command::ReplaceDeviceState {
                device,
                state: hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    device_state.state(),
                    ParameterId::from_static(1),
                    ParameterValue::Float(gain),
                )?,
            }
        };
        editor.execute(command)?;
        let revision = prepare(&editor, &table)?;
        let sequence = control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(revision)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "comparison publication")?;
        oracle.render(&mut expected)?;
        let mut rendered = None;
        let allocations = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut actual));
        });
        rendered.ok_or("callback did not render")??;
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0),
            "comparison handoff must not allocate/deallocate in callback"
        );
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            sequence
        );
        assert_eq!(
            control.status.position.load(Ordering::Acquire),
            u64::try_from((index + 2) * 128)?
        );
        // Only the effect path ramps; the return variant's dry track path
        // stays at unity beside it.
        let changed = gain != ramp.target;
        ramp = ramp.handoff(gain);
        assert_eq!(GainRamp::TOTAL, 240);
        for (frame, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            let (current, last) = ramp.next();
            if changed && frame == 0 {
                assert_ne!(current, gain, "a changed gain must ramp at step={index}");
            }
            if last {
                assert_eq!(current, gain, "the ramp lands exactly on its target");
                completed += 1;
            }
            let factor = if on_return {
                (1.0 + current) * 0.5
            } else {
                current
            };
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel] * factor).abs() < 1e-6,
                    "held phase/levels changed at step={index} frame={frame} channel={channel}: {} vs {}",
                    actual[channel],
                    expected[channel] * factor
                );
            }
        }
        control.reclaim();
    }
    // Step 0's ramp completes inside step 1 (a copy keeps the trajectory),
    // as does step 3's within step 4's handoff window only if uninterrupted.
    assert!(completed >= 1, "at least one ramp must land on its target");
    Ok(())
}
