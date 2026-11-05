//! Borrowed neutral classification of one instrument-state history edge.
use crate::{Asset, AssetId, Device, GainLane, Project, Track, TrackId};
use std::collections::BTreeMap;

/// At most one catalog membership change; every shared asset is identical.
#[derive(Debug, Clone, Copy)]
pub enum InstrumentAssetHistoryDelta<'a> {
    Unchanged,
    Added(&'a Asset),
    Removed(&'a Asset),
}
/// Borrowed neutral facts about one changed existing instrument history edge.
#[derive(Debug, Clone, Copy)]
pub struct InstrumentHistoryDelta<'a> {
    pub track: TrackId,
    pub current: &'a Device,
    pub target: &'a Device,
    pub assets: InstrumentAssetHistoryDelta<'a>,
}
fn added_asset<'a>(
    smaller: &BTreeMap<AssetId, Asset>,
    larger: &'a BTreeMap<AssetId, Asset>,
) -> Option<&'a Asset> {
    let mut added = None;
    for (id, asset) in larger {
        if let Some(shared) = smaller.get(id) {
            if shared != asset {
                return None;
            }
        } else if added.replace(asset).is_some() {
            return None;
        }
    }
    added
}
fn asset_delta<'a>(
    current: &'a BTreeMap<AssetId, Asset>,
    target: &'a BTreeMap<AssetId, Asset>,
) -> Option<InstrumentAssetHistoryDelta<'a>> {
    if current == target {
        Some(InstrumentAssetHistoryDelta::Unchanged)
    } else if current.len().checked_add(1)? == target.len() {
        added_asset(current, target).map(InstrumentAssetHistoryDelta::Added)
    } else if target.len().checked_add(1)? == current.len() {
        added_asset(target, current).map(InstrumentAssetHistoryDelta::Removed)
    } else {
        None
    }
}

impl Project {
    /// Return one changed existing instrument state and at most one catalog
    /// addition/removal, with all shared assets and other authored fields identical.
    /// `self` is current; `target` may be the borrowed
    /// raw history top or its prepared normalized candidate. Only the existing
    /// monotonic ID/recording-epoch normalization is allowed. No DSP is inspected.
    pub fn instrument_history_delta<'a>(
        &'a self,
        target: &'a Self,
    ) -> Option<InstrumentHistoryDelta<'a>> {
        let Self {
            aliases,
            native_sources,
            arrangement_sections,
            tuning,
            routing,
            audio_feeds,
            midi_routes,
            sidechains,
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
            offline_assets,
        } = self;
        if aliases != &target.aliases
            || offline_assets != &target.offline_assets
            || native_sources.len() != target.native_sources.len()
            || arrangement_sections != &target.arrangement_sections
            || tuning != &target.tuning
            || routing != &target.routing
            || audio_feeds != &target.audio_feeds
            || midi_routes != &target.midi_routes
            || sidechains != &target.sidechains
            || import_provenance != &target.import_provenance
            || sample_rate != &target.sample_rate
            || target.next_id > *next_id
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
        let asset_change = asset_delta(assets, &target.assets)?;
        let mut changed = None;
        for (current, next) in tracks.iter().zip(&target.tracks) {
            let Track {
                tags,
                kind,
                parent_group,
                color,
                gain_lane,
                id,
                name,
                automatic_name,
                gain,
                balance,
                muted,
                soloed,
                devices,
                instrument,
                retained_source_path,
                instrument_position,
                content,
                sends,
            } = current;
            let GainLane {
                base,
                spans,
                recording_epoch,
            } = gain_lane;
            if kind != &next.kind
                || parent_group != &next.parent_group
                || color != &next.color
                || id != &next.id
                || tags != &next.tags
                || name != &next.name
                || automatic_name != &next.automatic_name
                || gain != &next.gain
                || balance != &next.balance
                || muted != &next.muted
                || soloed != &next.soloed
                || devices != &next.devices
                || retained_source_path != &next.retained_source_path
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
                automation,
                id: device_id,
                state,
                bypassed,
                comparison,
            } = &**current;
            if current.source_id() != next.source_id()
                || device_id != &next.id
                || bypassed != &next.bypassed
                || automation != &next.automation
                || comparison != &next.comparison
                || state.type_id() != next.state.type_id()
                || state == &next.state
                || changed.is_some()
            {
                return None;
            }
            changed = Some((*id, current, next));
        }
        let (track, current, next_instrument) = changed?;
        let source = current.source_id();
        for (old, new) in native_sources.iter().zip(&target.native_sources) {
            if old == new {
                continue;
            }
            if old.id() != source
                || new.id() != source
                || old.name() != new.name()
                || !old.is_singleton()
                || !new.is_singleton()
                || old.instrument() != &**current
                || new.instrument() != &**next_instrument
            {
                return None;
            }
        }
        let target = next_instrument;
        Some(InstrumentHistoryDelta {
            track,
            current,
            target,
            assets: asset_change,
        })
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
                        let delta = current.instrument_history_delta(next).unwrap();
                        let (owner, a, b) = (delta.track, delta.current, delta.target);
                        assert!(matches!(
                            delta.assets,
                            InstrumentAssetHistoryDelta::Unchanged
                        ));
                        assert_eq!((owner, a.id(), b.id()), (track, device, device));
                        assert_eq!(
                            (a.state().payload(), b.state().payload()),
                            (&[1][..], &[0][..])
                        );
                    })
                    .is_some()
            );
            let refused = editor.undo_prepared_transition::<(), Invalid>(|current, next| {
                assert!(current.instrument_history_delta(next).is_some());
                Err(Invalid("native refusal"))
            });
            assert!(refused.is_err());
            assert_eq!(editor.project(), &before);
            assert!(!editor.can_redo());
        }
        assert!(
            editor
                .undo_prepared_transition::<(), Invalid>(|current, next| {
                    assert!(current.instrument_history_delta(next).is_some());
                    Ok(())
                })
                .unwrap()
                .is_some()
        );
        assert!(
            editor
                .inspect_history(true, |current, next| current
                    .instrument_history_delta(next)
                    .is_some())
                .unwrap()
        );
        assert!(
            editor
                .redo_prepared_transition::<(), Invalid>(|current, next| {
                    assert!(current.instrument_history_delta(next).is_some());
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
    fn one_catalog_membership_change_is_borrowed_but_shared_mutation_or_multiple_changes_refuse() {
        let (editor, _, _) = fixture();
        editor.inspect_history(false, |current, raw| {
            let asset = |id| Asset {
                id: AssetId(id), path: format!("{id}.wav").into(),
                source_sample_rate: 48000, source_frames: 4, playback_frames: 4,
                conversion_version: 1, original_source: None,
            };
            let mut current = current.clone();
            let mut target = raw.clone();
            current.assets.insert(AssetId(50), asset(50));
            target.assets = current.assets.clone();
            target.assets.insert(AssetId(51), asset(51));
            assert!(matches!(current.instrument_history_delta(&target).unwrap().assets, InstrumentAssetHistoryDelta::Added(a) if a.id() == AssetId(51)));
            assert!(matches!(target.instrument_history_delta(&current).unwrap().assets, InstrumentAssetHistoryDelta::Removed(a) if a.id() == AssetId(51)));
            target.assets.get_mut(&AssetId(50)).unwrap().source_frames = 3;
            assert!(current.instrument_history_delta(&target).is_none());
            target.assets.insert(AssetId(50), asset(50));
            target.assets.insert(AssetId(52), asset(52));
            assert!(current.instrument_history_delta(&target).is_none());
            target.assets.remove(&AssetId(52));
            target.tracks[0].instrument = current.tracks[0].instrument.clone();
            assert!(current.instrument_history_delta(&target).is_none(), "asset-only change");
        });
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
            assert!(current.instrument_history_delta(&target).is_some());
            for case in 0..11 {
                let mut next = target.clone();
                match case {
                    0 => next.next_id = current.next_id + 1,
                    1 => next.tracks[0].gain_lane.recording_epoch = 10,
                    2 => next.tracks[0].gain = 0.5,
                    3 => next.tracks[0].name.push('x'),
                    4 => {
                        let id = next.tracks[0].instrument().unwrap().id();
                        next.edit_device(id, |d| {
                            d.bypassed = true;
                            Ok(())
                        })
                        .unwrap();
                    }
                    5 => next.tracks[0].instrument = None,
                    6 => {
                        let id = next.tracks[0].instrument().unwrap().id();
                        next.edit_device(id, |d| {
                            d.id = crate::DeviceId(999);
                            Ok(())
                        })
                        .unwrap();
                    }
                    7 => next.master_gain = 0.5,
                    8 => next.tracks[0].instrument_position = Some(1),
                    9 => next.tracks.push(next.tracks[0].clone()),
                    10 => next.tracks[0].retained_source_path = true,
                    _ => panic!("case"),
                }
                assert!(
                    current.instrument_history_delta(&next).is_none(),
                    "case{case}"
                );
            }
        });
    }
}
