//! Control-side projection of admitted compensation facts for display.
use super::{SignalGraph, TrackOutput};

/// Algorithmic latency from one prepared owner, expressed in frames at the
/// applied render rate. Hardware buffers and round-trip latency are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLatency {
    /// Longest local device path, excluding input routes and alignment delay.
    pub local_frames: u32,
    /// Cumulative arrival at this owner's post-fader output.
    pub arrival_frames: u32,
    /// Delay inserted on the default route to the parent group or Main.
    /// Sends and additional audio routes can have separate delays.
    pub default_route_delay_frames: u32,
}

impl SignalGraph {
    pub(crate) fn track_latency(
        &self,
        index: usize,
        instrument_position: usize,
    ) -> Option<OutputLatency> {
        let track = self.tracks.get(index)?;
        let (prefix, suffix) = track.devices.split_at_checked(instrument_position)?;
        let prefix = self.chain_latency(prefix)?;
        let source = match track.generator {
            Some(index) => self.plan.nodes().get(index)?.latency_frames,
            None => 0,
        };
        let edge = match track.output {
            TrackOutput::Main { edge } | TrackOutput::Group { edge, .. } => edge,
        };
        Some(OutputLatency {
            local_frames: prefix
                .max(source)
                .checked_add(self.chain_latency(suffix)?)?,
            arrival_frames: self.plan.nodes().get(track.fader)?.output_arrival_frames,
            default_route_delay_frames: self.route_delay(edge)?,
        })
    }

    pub(crate) fn return_latency(&self, index: usize) -> Option<OutputLatency> {
        let bus = self.returns.get(index)?;
        Some(OutputLatency {
            local_frames: self.chain_latency(&bus.devices)?,
            arrival_frames: self.plan.nodes().get(bus.output)?.output_arrival_frames,
            default_route_delay_frames: self.route_delay(bus.main)?,
        })
    }

    pub(crate) fn main_output_latency(&self) -> Option<OutputLatency> {
        let output = self.main_devices.last().copied().unwrap_or(self.main);
        Some(OutputLatency {
            local_frames: self.chain_latency(&self.main_devices)?,
            arrival_frames: self.plan.nodes().get(output)?.output_arrival_frames,
            default_route_delay_frames: 0,
        })
    }

    fn route_delay(&self, index: usize) -> Option<u32> {
        let edge = *self.pre_main_edges.get(index)?;
        Some(self.plan.edges().get(edge)?.delay_frames)
    }

    fn chain_latency(&self, devices: &[usize]) -> Option<u32> {
        devices.iter().try_fold(0_u32, |total, &index| {
            total.checked_add(self.plan.nodes().get(index)?.latency_frames)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PreparedPlaybackObservation,
        signal_graph::tests::{fixture, prepare, state},
    };
    use hex_project::{Command, Created, TrackSelectionIds};

    #[test]
    fn observation_keeps_serial_arrival_default_alignment_and_main_separate() {
        let mut f = fixture(&[&[3, 5], &[], &[]], 32);
        let baseline =
            PreparedPlaybackObservation::prepare_on_control(&prepare(&f).unwrap()).unwrap();
        assert_eq!(
            baseline.track_latency(f.tracks[2]),
            Some(OutputLatency {
                local_frames: 0,
                arrival_frames: 0,
                default_route_delay_frames: 8,
            })
        );
        let mut children = f.tracks[..2].to_vec();
        let mut groups = Vec::new();
        for name in ["inner", "outer"] {
            let Created::Track(group) = f
                .editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(&children).unwrap(),
                    name: name.into(),
                })
                .unwrap()
            else {
                panic!("group")
            };
            groups.push(group);
            children = vec![group];
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        for command in [
            Command::SetReturnSend {
                track: groups[1],
                return_track: bus,
                gain: 1.0,
            },
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(4).unwrap(),
            },
            Command::InsertMainDevice {
                before: None,
                state: state(7).unwrap(),
            },
        ] {
            f.editor.execute(command).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let reading = PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        assert_eq!(reading.sample_rate(), 48_000);
        let facts = |local_frames, arrival_frames, default_route_delay_frames| {
            Some(OutputLatency {
                local_frames,
                arrival_frames,
                default_route_delay_frames,
            })
        };
        assert_eq!(reading.track_latency(f.tracks[0]), facts(8, 8, 0));
        assert_eq!(reading.track_latency(f.tracks[1]), facts(0, 0, 8));
        assert_eq!(reading.track_latency(f.tracks[2]), facts(0, 0, 12));
        assert_eq!(reading.track_latency(groups[0]), facts(0, 8, 0));
        assert_eq!(reading.track_latency(groups[1]), facts(0, 8, 4));
        assert_eq!(reading.return_latency(bus), facts(4, 12, 0));
        assert_eq!(reading.main_latency(), facts(7, 19, 0));
        assert_eq!(revision.arrangement().main_latency_frames(), 19);
        assert_eq!(baseline.return_latency(bus), None);

        let device = f.editor.project().track(f.tracks[0]).unwrap().devices()[1].id();
        f.editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let next = PreparedPlaybackObservation::prepare_on_control(&prepare(&f).unwrap()).unwrap();
        assert_eq!(next.track_latency(f.tracks[0]), facts(3, 3, 0));
        assert_eq!(next.main_latency(), facts(7, 14, 0));
        assert_eq!(
            reading.main_latency(),
            facts(7, 19, 0),
            "an existing observation stays immutable"
        );
    }

    #[test]
    fn bypassed_latency_aligned_effect_retains_the_admitted_delay() {
        let mut f = fixture(&[&[]], 32);
        let track = f.tracks[0];
        let observe = |f: &crate::signal_graph::tests::Fixture| {
            let revision = crate::PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                std::sync::Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap();
            PreparedPlaybackObservation::prepare_on_control(&revision).unwrap()
        };
        assert_eq!(observe(&f).track_latency(track).unwrap().local_frames, 0);
        let Created::Device(device) = f
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: crate::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.overdrive"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        let enabled = observe(&f);
        assert!(enabled.track_latency(track).unwrap().local_frames > 0);
        f.editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let bypassed = observe(&f);
        assert_eq!(bypassed.track_latency(track), enabled.track_latency(track));
        assert_eq!(bypassed.main_latency(), enabled.main_latency());
    }
}
