use super::*;
use crate::{Audio, audio_transform::AudioTransformPreparation};
use hex_project::{
    AudioEvent, AudioEvents, BeatTime, ClipGain, Command, Created, Editor, Project, Tempo,
    WarpMode, default_audio_warp,
};
use std::sync::Arc;

#[test]
fn signed_musical_destinations_read_original_warped_pcm_across_a_tempo_seam() {
    let beat = BeatTime::ONE.raw();
    let half = beat / 2;
    for warped in [false, true] {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 16_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        if warped {
            let project = editor.project();
            let settings = default_audio_warp(
                project.clip(clip).unwrap(),
                project.tempo_map(),
                WarpMode::RePitch,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })
                .unwrap();
        }
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(half),
                tempo: Tempo::new(240_000_000).unwrap(),
            })
            .unwrap();
        let project = editor.project();
        let tempo = project.tempo_map();
        let source_root = project.clip(clip).unwrap().audio_event_root();
        assert_eq!(source_root.domain.clock.length(), beat);
        let audio = Audio::new(
            16_000,
            (0..8_000)
                .map(|i| {
                    let x = (i + 1) as f32 / 8192.0;
                    [x, -x]
                })
                .collect(),
        )
        .unwrap();
        let context = AudioTransformPreparation::default();
        let source = context
            .prepare_audio_at_tempo(tempo, &source_root, project.asset(asset).unwrap(), &audio)
            .unwrap();
        assert_eq!(source.frames().len(), 6_000);
        let cancelled = AtomicBool::new(false);
        // Both cases retain the source at beat zero. One destination starts
        // a beat earlier, the other extends through the faster tempo.
        for (lead, target, expected_start, stride) in [(beat, 0, 0, 1), (0, beat + half, 8_000, 2)]
        {
            let event = |id, start, source_start| AudioEvent {
                id,
                start,
                end: start + half,
                source_start,
                source: 0,
                active: true,
                gain: ClipGain::UNITY,
                fade: None,
            };
            let moved = event(1, target, 0);
            let untouched = event(2, lead + half, half);
            let mut sequence = vec![moved, untouched];
            sequence.sort_by_key(|event| event.start);
            let mut destination = source_root.clone();
            destination.view_start += lead;
            destination.view_end += lead;
            let domain = Arc::make_mut(&mut destination.domain);
            domain.clock =
                AudioDomainClock::Beats(BeatTime::from_raw((beat + lead).max(moved.end)));
            domain.events = Some(Arc::new(AudioEvents {
                sources: Vec::new(),
                next_id: 3,
                events: sequence,
            }));
            let prepared = || EventSource {
                frames: source.frames(),
                root: &source_root,
                zero_in_destination: lead,
            };
            let output = compose(prepared(), &destination, tempo, &cancelled).unwrap();
            let unchanged_start = if lead == 0 { 4_000 } else { 12_000 };
            assert_eq!(
                &output[unchanged_start..unchanged_start + 2_000],
                &source.frames()[4_000..]
            );
            let moved_length = 4_000 / stride;
            for (index, sample) in output[expected_start..expected_start + moved_length]
                .iter()
                .enumerate()
            {
                assert_eq!(*sample, source.frames()[index * stride]);
            }
            for (index, sample) in output.iter().enumerate() {
                if !(expected_start..expected_start + moved_length).contains(&index)
                    && !(unchanged_start..unchanged_start + 2_000).contains(&index)
                {
                    assert_eq!(*sample, [0.0; 2]);
                }
            }

            // Valid destination extent cannot legitimize an overrun of the
            // smaller original source extent.
            let domain = Arc::make_mut(&mut destination.domain);
            let events = Arc::make_mut(domain.events.as_mut().unwrap());
            events
                .events
                .iter_mut()
                .find(|event| event.id == 1)
                .unwrap()
                .source_start = beat;
            assert_eq!(
                compose(prepared(), &destination, tempo, &cancelled)
                    .unwrap_err()
                    .0,
                "audio event exceeds its retained source clock"
            );
        }
    }
}
