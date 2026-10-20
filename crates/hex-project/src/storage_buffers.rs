//! Actual immutable exclusive model buffer fields. Shared children are separate.
//! Views create no snapshot, owner, pointer token or caller-authored byte claim.
use crate::*;

pub struct ProjectStorageBuffers<'a> {
    tracks: &'a Vec<Track>,
    returns: &'a Vec<ReturnTrack>,
    main_devices: &'a Vec<Device>,
    routing: &'a Vec<ReturnFeed>,
    audio_feeds: &'a Vec<AudioFeed>,
    midi_routes: &'a Vec<MidiRoute>,
    scenes: &'a Vec<Scene>,
    cells: &'a Vec<SessionCell>,
}
impl Project {
    pub fn storage_buffers_on_control(&self) -> ProjectStorageBuffers<'_> {
        let Self {
            tuning,
            routing,
            audio_feeds,
            midi_routes,
            import_provenance: _,
            sample_rate,
            next_id,
            assets: _,
            tracks,
            returns,
            session,
            loop_region,
            master_gain,
            main_devices,
            tempo_map: _,
            meter_map: _,
        } = self;
        let _: (
            &hex_tuning::ProjectTuning,
            &u32,
            &u64,
            &Option<LoopRegion>,
            &f32,
        ) = (tuning, sample_rate, next_id, loop_region, master_gain);
        let SessionGrid { scenes, cells } = session;
        ProjectStorageBuffers {
            tracks,
            returns,
            main_devices,
            routing,
            audio_feeds,
            midi_routes,
            scenes,
            cells,
        }
    }
}
impl<'a> ProjectStorageBuffers<'a> {
    pub fn tracks(&self) -> &'a Vec<Track> {
        self.tracks
    }
    pub fn returns(&self) -> &'a Vec<ReturnTrack> {
        self.returns
    }
    pub fn main_devices(&self) -> &'a Vec<Device> {
        self.main_devices
    }
    pub fn routing(&self) -> &'a Vec<ReturnFeed> {
        self.routing
    }
    pub fn audio_feeds(&self) -> &'a Vec<AudioFeed> {
        self.audio_feeds
    }
    pub fn midi_routes(&self) -> &'a Vec<MidiRoute> {
        self.midi_routes
    }
    pub fn scenes(&self) -> &'a Vec<Scene> {
        self.scenes
    }
    pub fn cells(&self) -> &'a Vec<SessionCell> {
        self.cells
    }
}

pub struct TrackStorageBuffers<'a> {
    name: &'a String,
    devices: &'a Vec<Device>,
    audio_clips: &'a Vec<Clip>,
    midi_clips: &'a Vec<MidiClip>,
    sends: &'a Vec<SendLevel>,
    gain_points: &'a Vec<GainPoint>,
    gain_spans: &'a Vec<GainTouchSpan>,
}
impl Track {
    pub fn storage_buffers_on_control(&self) -> TrackStorageBuffers<'_> {
        let Self {
            color,
            gain_lane,
            id,
            name,
            gain,
            balance,
            muted,
            soloed,
            devices,
            instrument: _,
            instrument_position,
            content,
            sends,
        } = self;
        let _: (
            &ElementColor,
            &TrackId,
            &f32,
            &f32,
            &bool,
            &bool,
            &Option<usize>,
        ) = (color, id, gain, balance, muted, soloed, instrument_position);
        let GainLane {
            base,
            spans,
            recording_epoch,
        } = gain_lane;
        let _: &u64 = recording_epoch;
        let TrackContent {
            audio_clips,
            midi_clips,
        } = content;
        TrackStorageBuffers {
            name,
            devices,
            audio_clips,
            midi_clips,
            sends,
            gain_points: base,
            gain_spans: spans,
        }
    }
}
impl<'a> TrackStorageBuffers<'a> {
    pub fn name(&self) -> &'a String {
        self.name
    }
    pub fn devices(&self) -> &'a Vec<Device> {
        self.devices
    }
    pub fn audio_clips(&self) -> &'a Vec<Clip> {
        self.audio_clips
    }
    pub fn midi_clips(&self) -> &'a Vec<MidiClip> {
        self.midi_clips
    }
    pub fn sends(&self) -> &'a Vec<SendLevel> {
        self.sends
    }
    pub fn gain_points(&self) -> &'a Vec<GainPoint> {
        self.gain_points
    }
    pub fn gain_spans(&self) -> &'a Vec<GainTouchSpan> {
        self.gain_spans
    }
}
impl ReturnTrack {
    pub fn device_buffer_on_control(&self) -> &Vec<Device> {
        &self.devices
    }
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}
impl Clip {
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}
impl MidiClip {
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}
impl SessionMidiClip {
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}
impl SessionAudioClip {
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}
impl Scene {
    pub fn name_buffer_on_control(&self) -> &String {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_model_buffer_views_keep_spare_and_empty_capacity_without_cloning() {
        let mut project = Project::new(48_000).unwrap();
        project
            .apply(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap();
        project.tracks.reserve_exact(7);
        project.routing.reserve_exact(11);
        project.session.cells.reserve_exact(13);
        let track = &mut project.tracks[0];
        track.name.reserve_exact(29);
        track.devices.reserve_exact(17);
        track.content.midi_clips.reserve_exact(19);
        track.gain_lane.base.reserve_exact(23);
        let buffers = project.storage_buffers_on_control();
        assert!(std::ptr::eq(buffers.tracks(), &project.tracks));
        assert!(std::ptr::eq(buffers.routing(), &project.routing));
        assert_eq!(buffers.routing().capacity(), 11);
        assert_eq!(buffers.cells().capacity(), 13);
        assert!(buffers.routing().is_empty());
        let track = &buffers.tracks()[0];
        let actual = track.storage_buffers_on_control();
        assert!(std::ptr::eq(actual.name(), &track.name));
        assert_eq!(actual.devices().capacity(), 17);
        assert_eq!(actual.midi_clips().capacity(), 19);
        assert_eq!(actual.gain_points().capacity(), 23);
        assert!(actual.name().capacity() >= 35);
        assert_eq!(project.tracks[0].name(), "source");
    }
}
