//! Outgoing Session audio clips fade out over the shared transport declick
//! length `F` at launch, stop and replace, independent of block size, for
//! resident and streamed sources, and a release that would overlap a still
//! sounding one is refused explicitly.
use hex_engine::{
    Audio, BorrowedMediaPage, Frame, LaunchQuantization, MediaPages, PreparedRevision,
    STREAM_PAGE_FRAMES, SessionPlayback, SessionPlaybackCommand, SessionPlaybackError,
    SessionPlaybackOwner, SineTable, StreamSourceKey, StreamedSourceMetadata,
    compile::PreparedAudioSource,
};
use hex_project::{AssetId, ClipId, Command, Created, Editor, Project, SceneId, TrackId};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
/// `round(48000 * 128 / 48000)`, the shared declick length at this rate.
const F: usize = 128;
/// One thirty-second note at the default 120 bpm.
const BOUNDARY: usize = 3_000;
const CLIP_FRAMES: usize = 4 * 24_000;
const LEVEL: f32 = 0.5;
const DC: Frame = [LEVEL, -LEVEL];
/// Frames rendered before the transition is queued; a multiple of every
/// tested block size, so the quantized boundary falls inside a block.
const LEAD: usize = 1_024;
const TOTAL: usize = BOUNDARY + 4 * F;
const GENERATION: u64 = 41;

struct Fixture {
    revision: Arc<PreparedRevision>,
    track: TrackId,
    dc: ClipId,
    silent: ClipId,
    silent_scene: SceneId,
    stream: Option<StreamSourceKey>,
}

/// One track with a DC clip and a silent clip in two scenes; `streamed`
/// serves the DC clip from media pages instead of resident PCM.
fn fixture(streamed: bool) -> Fixture {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut scene = || {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        scene
    };
    let (dc_scene, silent_scene) = (scene(), scene());
    let mut clip = |path: &str, scene| {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: path.into(),
                sample_rate: RATE,
                frames: CLIP_FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "clip".into(),
                asset,
                source_start: 0,
                source_end: CLIP_FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        (asset, clip)
    };
    let (dc_asset, dc) = clip("memory/dc.wav", dc_scene);
    let (silent_asset, silent) = clip("memory/silent.wav", silent_scene);
    let silent_audio = Audio::new(RATE, vec![[0.0; 2]; CLIP_FRAMES]).unwrap();
    let project = editor.project();
    let (revision, stream) = if streamed {
        let key = StreamSourceKey::new(GENERATION, dc_asset);
        let sources = BTreeMap::from([
            (
                dc_asset,
                PreparedAudioSource::Streamed(Arc::new(metadata(project, key, dc_asset))),
            ),
            (silent_asset, PreparedAudioSource::Resident(silent_audio)),
        ]);
        let revision = PreparedRevision::prepare_with_prepared_audio_sources(
            project,
            &sources,
            GENERATION,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap();
        (revision, Some(key))
    } else {
        let media = BTreeMap::from([
            (dc_asset, Audio::new(RATE, vec![DC; CLIP_FRAMES]).unwrap()),
            (silent_asset, silent_audio),
        ]);
        let revision =
            PreparedRevision::prepare(project, &media, Arc::new(SineTable::new()), None).unwrap();
        (revision, None)
    };
    Fixture {
        revision: Arc::new(revision),
        track,
        dc,
        silent,
        silent_scene,
        stream,
    }
}

fn metadata(project: &Project, key: StreamSourceKey, asset: AssetId) -> StreamedSourceMetadata {
    let asset = project.assets().find(|a| a.id() == asset).unwrap();
    StreamedSourceMetadata::new(
        key,
        asset.source_sample_rate(),
        project.sample_rate(),
        asset.source_frames(),
        asset.playback_frames(),
        asset.conversion_version(),
        [9; 32],
    )
    .unwrap()
}

/// Serves every page of the streamed DC source.
struct DcPages {
    key: Option<StreamSourceKey>,
    page: Vec<Frame>,
}

impl DcPages {
    fn new(fixture: &Fixture) -> Self {
        Self {
            key: fixture.stream,
            page: vec![DC; STREAM_PAGE_FRAMES],
        }
    }
}

impl MediaPages for DcPages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        (Some(key) == self.key).then_some(BorrowedMediaPage {
            page_index,
            frames: &self.page,
        })
    }

    fn request_page(&mut self, _key: StreamSourceKey, _page_index: u64) {}
}

fn render(playback: &mut SessionPlayback, pages: &mut DcPages, output: &mut [Frame]) {
    playback.render_with_pages(output, pages).unwrap();
}

/// Plays the DC clip from frame zero.
fn playing(fixture: &Fixture) -> SessionPlayback {
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.dc,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
}

/// Plays the DC clip, queues `transition` at `LEAD` for the next boundary and
/// renders `TOTAL` frames in `block`-frame chunks.
fn render_transition(
    fixture: &Fixture,
    transition: SessionPlaybackCommand,
    block: usize,
) -> Vec<Frame> {
    let mut playback = playing(fixture);
    let mut pages = DcPages::new(fixture);
    let mut output = vec![[0.0; 2]; TOTAL];
    let (lead, rest) = output.split_at_mut(LEAD);
    for chunk in lead.chunks_mut(block) {
        render(&mut playback, &mut pages, chunk);
    }
    playback.apply(transition).unwrap();
    for chunk in rest.chunks_mut(block) {
        render(&mut playback, &mut pages, chunk);
    }
    output
}

fn largest_step(output: &[Frame]) -> f32 {
    output
        .windows(2)
        .map(|pair| {
            (pair[1][0] - pair[0][0])
                .abs()
                .max((pair[1][1] - pair[0][1]).abs())
        })
        .fold(0.0, f32::max)
}

fn transitions(fixture: &Fixture) -> [(&'static str, SessionPlaybackCommand); 5] {
    let quantization = LaunchQuantization::ThirtySecond;
    [
        (
            "clip launch",
            SessionPlaybackCommand::LaunchClip {
                clip: fixture.silent,
                quantization,
            },
        ),
        (
            "scene replace",
            SessionPlaybackCommand::LaunchScene {
                scene: fixture.silent_scene,
                quantization,
            },
        ),
        (
            "track stop",
            SessionPlaybackCommand::StopTrack {
                track: fixture.track,
                quantization,
            },
        ),
        (
            "stop all",
            SessionPlaybackCommand::StopAllClips { quantization },
        ),
        (
            "back to arrangement",
            SessionPlaybackCommand::BackToArrangement { quantization },
        ),
    ]
}

fn assert_fades_at_every_transition(fixture: &Fixture) {
    for (name, transition) in transitions(fixture) {
        let output = render_transition(fixture, transition, 64);
        assert_eq!(output[BOUNDARY - 1], DC, "{name}: full before");
        assert_eq!(output[BOUNDARY], DC, "{name}: release starts at full gain");
        assert_eq!(output[BOUNDARY + F], [0.0; 2], "{name}: silent after F");
        let step = largest_step(&output);
        assert!(
            step <= LEVEL / F as f32 + 1e-6,
            "{name}: largest step {step}"
        );
        for block in [1, 512] {
            assert_eq!(
                render_transition(fixture, transition, block),
                output,
                "{name}: block {block}"
            );
        }
    }
}

#[test]
fn outgoing_resident_audio_fades_over_f_frames_at_every_transition() {
    assert_fades_at_every_transition(&fixture(false));
}

#[test]
fn outgoing_streamed_audio_fades_over_f_frames_at_every_transition() {
    assert_fades_at_every_transition(&fixture(true));
}

#[test]
fn streamed_release_pages_stay_demanded_until_the_fade_ends() {
    let fixture = fixture(true);
    let key = fixture.stream.unwrap();
    let demanded = |playback: &SessionPlayback| {
        let mut found = false;
        playback
            .visit_next_stream_chunk(256, None, &mut |extent| {
                found |= extent.source == key.into();
                Ok(())
            })
            .unwrap();
        found
    };
    let mut playback = playing(&fixture);
    let mut pages = DcPages::new(&fixture);
    let mut output = vec![[0.0; 2]; BOUNDARY];
    render(&mut playback, &mut pages, &mut output[..LEAD]);
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::ThirtySecond,
        })
        .unwrap();
    render(&mut playback, &mut pages, &mut output[LEAD..]);
    assert!(
        demanded(&playback),
        "due stop still reads the outgoing clip"
    );
    render(&mut playback, &mut pages, &mut output[..F / 2]);
    assert!(demanded(&playback), "sounding release reads its clip");
    render(&mut playback, &mut pages, &mut output[..F / 2]);
    assert!(!demanded(&playback), "finished release reads nothing");
}

#[test]
fn a_release_overlapping_a_sounding_release_is_refused_explicitly() {
    let fixture = fixture(false);
    let stop = |quantization| SessionPlaybackCommand::StopTrack {
        track: fixture.track,
        quantization,
    };
    let silent = SessionPlaybackOwner::Session {
        clip: fixture.silent,
    };
    let mut pages = DcPages::new(&fixture);
    // Release the DC clip at `start`, then try to leave the silent clip
    // while that release still sounds.
    let replaced_at = |start: usize| {
        let mut playback = playing(&fixture);
        let mut pages = DcPages::new(&fixture);
        render(&mut playback, &mut pages, &mut vec![[0.0; 2]; start]);
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: fixture.silent,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback
    };

    let mut immediate = replaced_at(LEAD);
    render(&mut immediate, &mut pages, &mut [[0.0; 2]; F / 2]);
    let generation = immediate.route_state_generation();
    assert_eq!(
        immediate.apply(stop(LaunchQuantization::Immediate)),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(immediate.route_state_generation(), generation);
    assert_eq!(immediate.owner(fixture.track), Some(silent));
    render(&mut immediate, &mut pages, &mut [[0.0; 2]; F / 2]);
    immediate
        .apply(stop(LaunchQuantization::Immediate))
        .unwrap();
    assert_eq!(
        immediate.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );

    // A quantized stop due inside the release is refused when queued; one
    // due after it ends is admitted.
    let mut quantized = replaced_at(BOUNDARY - F / 2);
    assert_eq!(
        quantized.apply(stop(LaunchQuantization::ThirtySecond)),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    let mut later = replaced_at(BOUNDARY - 2 * F);
    later.apply(stop(LaunchQuantization::ThirtySecond)).unwrap();
}
