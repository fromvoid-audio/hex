use super::*;
use hex_engine::Audio;
use hex_project::{
    AudioEventEdit, AudioEventFade, AudioViewResizeOperation, Command, Created, Editor, Project,
};

fn fixture(samples: Vec<[f32; 2]>) -> (Editor, hex_project::ClipId, CachedWaveform) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 44_100,
            frames: samples.len() as u64,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let audio = Audio::new(44_100, samples).unwrap();
    let waveform = hex_media::waveform::summarize(&audio, 16).unwrap();
    (editor, clip, waveform)
}
fn projection(
    editor: &Editor,
    clip: hex_project::ClipId,
    waveform: &CachedWaveform,
    coordinates: Coordinates,
) -> Projection {
    let project = editor.project();
    let clip = project.clip(clip).unwrap();
    Projection::new(
        clip,
        project.asset(clip.asset()).unwrap(),
        project.tempo_map(),
        waveform,
        None,
        None,
        None,
        coordinates,
    )
    .unwrap()
}

#[test]
fn effective_identity_and_exact_samples_survive_first_fade_ack() {
    let (editor, clip, waveform) = fixture(
        (0..256)
            .map(|i| [(i % 17) as f32 / 8.0, -((i % 23) as f32) / 10.0])
            .collect(),
    );
    let project = editor.project();
    let row = project.clip(clip).unwrap();
    let fade = ClipFade::new(256, 0, 71, 61).unwrap();
    let event_fade = AudioEventFade::new(0, ClipFade::new(256, 0, 33, 47).unwrap()).unwrap();
    let operation = AudioEventEdit::SetFade {
        event: 1,
        fade: Some(event_fade),
    };
    let events = row
        .editable_audio_events()
        .unwrap()
        .edited(operation.clone(), 256)
        .unwrap();
    let gain = ClipGain::new(9_089).unwrap();
    let held = Projection::new(
        row,
        project.asset(row.asset()).unwrap(),
        project.tempo_map(),
        &waveform,
        Some(fade),
        Some(&events),
        Some(gain),
        Coordinates::Timeline,
    )
    .unwrap();
    let mut accepted = Editor::new(project.clone(), 8).unwrap();
    accepted
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    accepted
        .execute(Command::EditAudioEvents { clip, operation })
        .unwrap();
    accepted
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: gain.millidb(),
        })
        .unwrap();
    let canonical = projection(&accepted, clip, &waveform, Coordinates::Timeline);
    assert_eq!(held.key(), canonical.key());
    let hq = held.query().unwrap();
    let cq = canonical.query().unwrap();
    let mut hs = held.sampler(&hq, &waveform).unwrap();
    let mut cs = canonical.sampler(&cq, &waveform).unwrap();
    for start in (0..256).step_by(8) {
        assert_eq!(hs.exact(start..start + 8, 8), cs.exact(start..start + 8, 8));
    }
}

#[test]
fn canonical_cropped_event_uses_the_same_virtual_id_and_original_samples() {
    let (mut editor, clip, waveform) = fixture((0..256).map(|i| [i as f32 / 256.0; 2]).collect());
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::CycleRange {
                start: 32,
                end: 224,
                looping: true,
            },
        })
        .unwrap();
    let row = editor.project().clip(clip).unwrap();
    let id = row
        .editable_audio_events()
        .unwrap()
        .events
        .iter()
        .find(|e| e.start == 32)
        .unwrap()
        .id;
    assert_ne!(id, 1);
    let p = projection(&editor, clip, &waveform, Coordinates::Event(id));
    let query = p.query().unwrap();
    let mut sampler = p.sampler(&query, &waveform).unwrap();
    let column = sampler.exact(32..40, 8).unwrap();
    assert_eq!(
        column.peaks,
        [CachedPeak {
            min: 32.0 / 256.0,
            max: 39.0 / 256.0
        }; 2]
    );
}

#[test]
fn coarse_silence_and_shared_budget_are_not_partial_or_safe_overrange() {
    let (mut editor, clip, waveform) = fixture(vec![[0.25; 2]; 256]);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Delete { start: 64, end: 96 },
        })
        .unwrap();
    let p = projection(&editor, clip, &waveform, Coordinates::Timeline);
    let query = p.query().unwrap();
    let mut sampler = p.sampler(&query, &waveform).unwrap();
    assert_eq!(
        sampler.peaks(0..256, 0),
        Some(
            [CachedPeak {
                min: 0.0,
                max: 0.25
            }; 2]
        )
    );
    sampler.range_remaining = 1;
    assert!(sampler.peaks(0..256, 0).is_none());
    assert_eq!(sampler.range_remaining, 0);
    sampler.detail_remaining = 0;
    assert!(sampler.exact(0..1, 1).is_none());
    let peak_only =
        CachedWaveform::from_peaks(waveform.source_frames(), waveform.bins().to_vec()).unwrap();
    let row = editor.project().clip(clip).unwrap();
    assert!(
        Projection::new(
            row,
            editor.project().asset(row.asset()).unwrap(),
            editor.project().tempo_map(),
            &peak_only,
            None,
            None,
            None,
            Coordinates::Timeline
        )
        .is_none()
    );
}

#[test]
fn exact_overrange_uses_sequential_owner_gains() {
    let fade = ClipFade::new(256, 0, 256, 0).unwrap();
    let gain = ClipGain::new(9_089).unwrap();
    let f = fade.gain_at(50);
    let g = gain.linear();
    let base = (1.0_f32 / (f * g)).to_bits();
    let source = (base - 128..base + 128)
        .map(f32::from_bits)
        .find(|value| (*value * f) * g > 1.0 && *value * (f * g) <= 1.0)
        .unwrap();
    let (editor, clip, waveform) = fixture(vec![[source, -source]; 256]);
    let row = editor.project().clip(clip).unwrap();
    let p = Projection::new(
        row,
        editor.project().asset(row.asset()).unwrap(),
        editor.project().tempo_map(),
        &waveform,
        Some(fade),
        None,
        Some(gain),
        Coordinates::Timeline,
    )
    .unwrap();
    let query = p.query().unwrap();
    let column = p
        .sampler(&query, &waveform)
        .unwrap()
        .exact(50..51, 1)
        .unwrap();
    assert_eq!(column.overrange.positive, [true, false]);
    assert_eq!(column.overrange.negative, [false, true]);
}
