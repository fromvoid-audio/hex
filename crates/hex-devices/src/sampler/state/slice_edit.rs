//! One control-authored marker edit. The caller commits the returned whole
//! state with its existing expected-state gesture/Undo transaction. No PCM,
//! automatic detection, runtime admission or UI-owned source is introduced.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceEdit {
    Insert { frame: usize },
    Move { index: usize, frame: usize },
    Remove { index: usize },
}
/// The first marker is the source start flag; move it through the existing
/// START edit instead. Other markers cannot cross neighbours or leave bounds.
/// Insert/remove renumber subsequent MIDI keys; the mapped range must remain
/// within 0..128. Refusal leaves the supplied current state untouched.
pub fn edit_slice_on_control(
    current: &NativeDeviceState,
    edit: SliceEdit,
) -> Result<NativeDeviceState, Error> {
    let DeviceState::Sample(mut state) = DeviceState::from_native(current)? else {
        return Err(Error::AssetResourcesUnavailable);
    };
    let AuthoredMode::Slice {
        starts, base_note, ..
    } = &mut state.mode
    else {
        return Err(Error::InvalidParameterValue);
    };
    match edit {
        SliceEdit::Insert { frame } => {
            if frame <= state.region.start
                || frame >= state.region.end
                || starts.len() >= MAX_SLICES
                || starts.len() >= 128 - usize::from(*base_note)
            {
                return Err(Error::InvalidParameterValue);
            }
            let index = starts
                .binary_search(&frame)
                .err()
                .ok_or(Error::InvalidParameterValue)?;
            starts
                .try_reserve_exact(1)
                .map_err(|_| Error::InvalidParameterValue)?;
            starts.insert(index, frame);
        }
        SliceEdit::Move { index, frame } => {
            let previous = index
                .checked_sub(1)
                .and_then(|i| starts.get(i))
                .ok_or(Error::InvalidParameterValue)?;
            let _ = starts.get(index).ok_or(Error::InvalidParameterValue)?;
            let next = starts.get(index + 1).copied().unwrap_or(state.region.end);
            if frame <= *previous || frame >= next {
                return Err(Error::InvalidParameterValue);
            }
            if starts[index] == frame {
                return Ok(current.clone());
            }
            starts[index] = frame;
        }
        SliceEdit::Remove { index } => {
            if index == 0 || index >= starts.len() {
                return Err(Error::InvalidParameterValue);
            }
            starts.remove(index);
        }
    }
    state.to_native()?.with_host_metadata_from(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    fn native() -> NativeDeviceState {
        let mut state = SamplerState::for_source_frames(12).unwrap();
        state.mode = AuthoredMode::Slice {
            starts: vec![0, 4, 9],
            base_note: 60,
            gate: GateMode::Trigger,
        };
        state
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(SOURCE_SLOT).unwrap(),
                asset: DeviceAssetKey::new(7).unwrap(),
            }])
            .unwrap()
            .with_input_gain_millidb(-1500)
            .unwrap()
            .with_output_gain_millidb(2250)
            .unwrap()
    }
    fn markers(native: &NativeDeviceState) -> Vec<usize> {
        let DeviceStateRead::Sample(read) = DeviceStateRead::from_native(native).unwrap() else {
            panic!("loaded")
        };
        read.slice_starts().collect()
    }
    #[test]
    fn slice_edits_preserve_binding_controls_and_current_roundtrip() {
        let original = native();
        let inserted = edit_slice_on_control(&original, SliceEdit::Insert { frame: 7 }).unwrap();
        assert_eq!(markers(&inserted), [0, 4, 7, 9]);
        let moved =
            edit_slice_on_control(&inserted, SliceEdit::Move { index: 1, frame: 3 }).unwrap();
        assert_eq!(markers(&moved), [0, 3, 7, 9]);
        let removed = edit_slice_on_control(&moved, SliceEdit::Remove { index: 2 }).unwrap();
        assert_eq!(markers(&removed), [0, 3, 9]);
        assert_eq!(markers(&original), [0, 4, 9]);
        for state in [&inserted, &moved, &removed] {
            assert_eq!(state.asset_references(), original.asset_references());
            assert_eq!(state.input_gain_millidb(), -1500);
            assert_eq!(state.output_gain_millidb(), 2250);
            let decoded = SamplerState::from_native(state).unwrap();
            let roundtrip = decoded
                .to_native()
                .unwrap()
                .with_host_metadata_from(state)
                .unwrap();
            assert_eq!(&roundtrip, state);
        }
    }
    #[test]
    fn slice_edits_refuse_crossing_first_marker_duplicates_and_invalid_modes_atomically() {
        let original = native();
        for edit in [
            SliceEdit::Insert { frame: 0 },
            SliceEdit::Insert { frame: 4 },
            SliceEdit::Insert { frame: 12 },
            SliceEdit::Insert { frame: usize::MAX },
            SliceEdit::Move { index: 0, frame: 1 },
            SliceEdit::Move { index: 1, frame: 9 },
            SliceEdit::Move { index: 2, frame: 4 },
            SliceEdit::Move {
                index: usize::MAX,
                frame: 5,
            },
            SliceEdit::Remove { index: 0 },
            SliceEdit::Remove { index: 99 },
        ] {
            assert!(edit_slice_on_control(&original, edit).is_err(), "{edit:?}");
            assert_eq!(markers(&original), [0, 4, 9]);
        }
        assert_eq!(
            edit_slice_on_control(&original, SliceEdit::Move { index: 1, frame: 4 }).unwrap(),
            original
        );
        let classic = SamplerState::for_source_frames(12)
            .unwrap()
            .to_native()
            .unwrap();
        let empty = DeviceState::Empty.to_native().unwrap();
        for state in [&classic, &empty] {
            assert!(edit_slice_on_control(state, SliceEdit::Insert { frame: 3 }).is_err());
        }
    }
    #[test]
    fn slice_insert_checks_exact_midi_mapping_capacity_and_last_marker_removal() {
        let mut state = SamplerState::for_source_frames(256).unwrap();
        state.mode = AuthoredMode::Slice {
            starts: (0..128).collect(),
            base_note: 0,
            gate: GateMode::Gate,
        };
        let full = state.to_native().unwrap();
        assert!(edit_slice_on_control(&full, SliceEdit::Insert { frame: 200 }).is_err());
        let removed = edit_slice_on_control(&full, SliceEdit::Remove { index: 127 }).unwrap();
        let exact = edit_slice_on_control(&removed, SliceEdit::Insert { frame: 200 }).unwrap();
        assert_eq!(markers(&exact).len(), 128);
        state.mode = AuthoredMode::Slice {
            starts: vec![0],
            base_note: 127,
            gate: GateMode::Gate,
        };
        let one = state.to_native().unwrap();
        assert!(edit_slice_on_control(&one, SliceEdit::Insert { frame: 1 }).is_err());
        assert!(edit_slice_on_control(&one, SliceEdit::Remove { index: 0 }).is_err());
    }
    #[test]
    fn edited_slice_markers_change_actual_pcm_note_regions_without_pitch_transposition() {
        let pcm: Vec<f32> = (0..12).map(|frame| frame as f32 / 16.0).collect();
        let original = native();
        let moved =
            edit_slice_on_control(&original, SliceEdit::Move { index: 1, frame: 6 }).unwrap();
        for (native, start, end) in [(&original, 4, 9), (&moved, 6, 9)] {
            let state = SamplerState::from_native(native).unwrap();
            let plan = PreparedSamplerPlan::new(
                SampleShape::new(12, 8_000, SampleChannels::Mono).unwrap(),
                8_000,
                state.settings(),
            )
            .unwrap();
            let voice = plan.prepare_voice_coefficients(61, 127).unwrap().unwrap();
            assert_eq!(voice.region, Region { start, end });
            let mut whole = [[0.0; 2]; 8];
            voice.render(
                hex_lattice::DeviceResourcePcm::Mono(&pcm),
                0,
                u64::MAX,
                &mut whole,
            );
            let expected: Vec<_> = (start..end)
                .map(|frame| [pcm[frame]; 2])
                .chain(std::iter::repeat_n([0.0; 2], 8 - (end - start)))
                .collect();
            assert_eq!(whole.as_slice(), expected);
            let mut partitioned = [[0.0; 2]; 8];
            voice.render(
                hex_lattice::DeviceResourcePcm::Mono(&pcm),
                0,
                u64::MAX,
                &mut partitioned[..2],
            );
            voice.render(
                hex_lattice::DeviceResourcePcm::Mono(&pcm),
                2,
                u64::MAX,
                &mut partitioned[2..],
            );
            assert_eq!(partitioned, whole);
        }
    }
}
