//! Borrowed neutral classification of one instrument-state history edge.
use crate::{Device, GainLane, Project, Track, TrackId};

impl Project {
    /// Return exactly one changed existing instrument state, with all other
    /// authored fields identical. `self` is current; `target` may be the borrowed
    /// raw history top or its prepared normalized candidate. Only the existing
    /// monotonic ID/recording-epoch normalization is allowed. No DSP is inspected.
    pub fn instrument_state_history_delta<'a>(
        &'a self,
        target: &'a Self,
    ) -> Option<(TrackId, &'a Device, &'a Device)> {
        let Self {
            tuning,
            routing,
            audio_feeds,
            midi_routes,
            import_provenance,
            sample_rate,
            next_id,
            assets,
            tracks,
            returns,
            session,
            loop_region,
            master_gain,
            main_devices,
            tempo_map,
            meter_map,
        } = self;
        if tuning != &target.tuning
            || routing != &target.routing
            || audio_feeds != &target.audio_feeds
            || midi_routes != &target.midi_routes
            || import_provenance != &target.import_provenance
            || sample_rate != &target.sample_rate
            || target.next_id > *next_id
            || assets != &target.assets
            || returns != &target.returns
            || session != &target.session
            || loop_region != &target.loop_region
            || master_gain != &target.master_gain
            || main_devices != &target.main_devices
            || tempo_map != &target.tempo_map
            || meter_map != &target.meter_map
            || tracks.len() != target.tracks.len()
        {
            return None;
        }
        let mut changed = None;
        for (current, next) in tracks.iter().zip(&target.tracks) {
            let Track {
                color,
                gain_lane,
                id,
                name,
                gain,
                balance,
                muted,
                soloed,
                devices,
                instrument,
                instrument_position,
                content,
                sends,
            } = current;
            let GainLane {
                base,
                spans,
                recording_epoch,
            } = gain_lane;
            if color != &next.color
                || id != &next.id
                || name != &next.name
                || gain != &next.gain
                || balance != &next.balance
                || muted != &next.muted
                || soloed != &next.soloed
                || devices != &next.devices
                || instrument_position != &next.instrument_position
                || content != &next.content
                || sends != &next.sends
                || base != &next.gain_lane.base
                || spans != &next.gain_lane.spans
                || next.gain_lane.recording_epoch > *recording_epoch
            {
                return None;
            }
            if instrument == &next.instrument {
                continue;
            }
            let (Some(current), Some(next)) = (instrument, &next.instrument) else {
                return None;
            };
            let Device {
                id: device_id,
                state,
                bypassed,
                comparison,
            } = current;
            if device_id != &next.id
                || bypassed != &next.bypassed
                || comparison != &next.comparison
                || state == &next.state
                || changed.is_some()
            {
                return None;
            }
            changed = Some((*id, current, next));
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, Invalid};
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    fn fixture() -> (Editor, TrackId, crate::DeviceId) {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "sampler".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("test.instrument"), 1, vec![0])
                .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertInstrument {
                track,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: NativeDeviceState::new(state.type_id(), 1, vec![1]).unwrap(),
            })
            .unwrap();
        (editor, track, device)
    }
    #[test]
    fn borrowed_next_edge_is_global_exact_and_refusal_does_not_consume_it() {
        let (mut editor, track, device) = fixture();
        let before = editor.project().clone();
        for _ in 0..2 {
            assert!(
                editor
                    .inspect_history(false, |current, next| {
                        let (owner, a, b) = current.instrument_state_history_delta(next).unwrap();
                        assert_eq!((owner, a.id(), b.id()), (track, device, device));
                        assert_eq!(
                            (a.state().payload(), b.state().payload()),
                            (&[1][..], &[0][..])
                        );
                    })
                    .is_some()
            );
            let refused = editor.undo_prepared_transition::<(), Invalid>(|current, next| {
                assert!(current.instrument_state_history_delta(next).is_some());
                Err(Invalid("native refusal"))
            });
            assert!(refused.is_err());
            assert_eq!(editor.project(), &before);
            assert!(!editor.can_redo());
        }
        assert!(
            editor
                .undo_prepared_transition::<(), Invalid>(|current, next| {
                    assert!(current.instrument_state_history_delta(next).is_some());
                    Ok(())
                })
                .unwrap()
                .is_some()
        );
        assert!(
            editor
                .inspect_history(true, |current, next| current
                    .instrument_state_history_delta(next)
                    .is_some())
                .unwrap()
        );
        assert!(
            editor
                .redo_prepared_transition::<(), Invalid>(|current, next| {
                    assert!(current.instrument_state_history_delta(next).is_some());
                    Ok(())
                })
                .unwrap()
                .is_some()
        );
        assert_eq!(editor.project(), &before);
        let empty = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
        assert!(
            empty
                .inspect_history(false, |_, _| panic!("empty callback"))
                .is_none()
        );
        assert!(
            empty
                .inspect_history(true, |_, _| panic!("empty callback"))
                .is_none()
        );
    }
    #[test]
    fn only_monotonic_history_custody_is_ignored_and_other_authored_changes_refuse() {
        let (editor, _, _) = fixture();
        editor.inspect_history(false, |current, raw| {
            let mut current = current.clone();
            let mut target = raw.clone();
            current.next_id += 7;
            current.tracks[0].gain_lane.recording_epoch = 9;
            target.tracks[0].gain_lane.recording_epoch = 2;
            assert!(current.instrument_state_history_delta(&target).is_some());
            for case in 0..10 {
                let mut next = target.clone();
                match case {
                    0 => next.next_id = current.next_id + 1,
                    1 => next.tracks[0].gain_lane.recording_epoch = 10,
                    2 => next.tracks[0].gain = 0.5,
                    3 => next.tracks[0].name.push('x'),
                    4 => next.tracks[0].instrument.as_mut().unwrap().bypassed = true,
                    5 => next.tracks[0].instrument = None,
                    6 => next.tracks[0].instrument.as_mut().unwrap().id = crate::DeviceId(999),
                    7 => next.master_gain = 0.5,
                    8 => next.tracks[0].instrument_position = Some(1),
                    9 => next.tracks.push(next.tracks[0].clone()),
                    _ => panic!("case"),
                }
                assert!(
                    current.instrument_state_history_delta(&next).is_none(),
                    "case{case}"
                );
            }
        });
    }
}
