//! Canonical and preview `ViewState` snapshot builders for the session worker.
use super::*;

pub(super) fn view_state(
    session: &Session,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    revision: u64,
    project_generation: u64,
    unsaved: bool,
    document_path: Option<&Path>,
) -> Arc<ViewState> {
    view_state_bound(
        session,
        midi_preview,
        revision,
        project_generation,
        unsaved,
        document_path,
        None,
    )
}

pub(super) fn view_state_bound(
    session: &Session,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    revision: u64,
    project_generation: u64,
    unsaved: bool,
    document_path: Option<&Path>,
    binding: Option<(
        Arc<CanonicalAudioSourceSnapshot>,
        Option<ComparisonAudioBinding>,
    )>,
) -> Arc<ViewState> {
    let project = session.project_arc();
    let rate = u64::from(project.sample_rate());
    let latest_clip_end = latest_timeline_content_end(&project);
    let latest_content_end = latest_clip_end.max(
        project
            .loop_region()
            .map(|region| region.end())
            .unwrap_or(0),
    );
    Arc::new(ViewState {
        // This sealed same-audio authoring alias owns no native resources and grants no
        // publication authority. Source-bearing devices use the identical exact-context proof.
        comparison_audio: binding.as_ref().and_then(|(_, proof)| proof.clone()),
        // This existing projection contains legacy audio bindings and exact
        // context only. It never contains native_assets/Sampler PCM owners; the
        // private worker borrow is the sole sampler native construction input.
        canonical_audio_source: binding.map(|(source, _)| source).or_else(|| {
            Some(Arc::new(CanonicalAudioSourceSnapshot::capture(
                &project,
                project_generation,
                revision,
                session,
            )))
        }),
        gain_preview: hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new),
        project,
        waveforms: Arc::new(session.waveforms()),
        source_overviews: session
            .source_overviews()
            .at_context(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            }),
        audio_clip_waveforms: Arc::new(session.audio_clip_waveforms().unwrap_or_default()),
        midi_preview,
        timeline_frames: latest_content_end
            .saturating_add(rate.saturating_mul(2))
            .max(rate.saturating_mul(8)),
        can_undo: session.can_undo(),
        can_redo: session.can_redo(),
        revision,
        unsaved,
        document_path: document_path.map(Path::to_path_buf),
    })
}

fn gain_preview_inputs_unchanged(canonical: &Project, preview: &Project) -> bool {
    canonical.sample_rate() == preview.sample_rate()
        && canonical.tempo_map() == preview.tempo_map()
        && canonical.tracks().len() == preview.tracks().len()
        && canonical
            .tracks()
            .iter()
            .zip(preview.tracks().iter())
            .all(|(a, b)| {
                a.id() == b.id()
                    && a.gain().to_bits() == b.gain().to_bits()
                    && a.gain_lane() == b.gain_lane()
            })
}

pub(crate) fn preview_view_state(
    canonical: &Arc<ViewState>,
    project: Arc<Project>,
    prepared: Arc<PreparedRevision>,
) -> Arc<ViewState> {
    let rate = u64::from(project.sample_rate());
    let content_end = latest_timeline_content_end(&project).max(
        project
            .loop_region()
            .map(|region| region.end())
            .unwrap_or(0),
    );
    let gain_preview = if canonical.gain_preview.is_ok()
        && gain_preview_inputs_unchanged(&canonical.project, &project)
    {
        canonical.gain_preview.clone()
    } else {
        hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new)
    };
    Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        gain_preview,
        project,
        waveforms: Arc::clone(&canonical.waveforms),
        source_overviews: canonical.source_overviews.clone(),
        audio_clip_waveforms: Arc::clone(&canonical.audio_clip_waveforms),
        midi_preview: Arc::clone(prepared.midi_preview()),
        timeline_frames: content_end
            .saturating_add(rate.saturating_mul(2))
            .max(rate.saturating_mul(8)),
        can_undo: canonical.can_undo,
        can_redo: canonical.can_redo,
        revision: canonical.revision,
        unsaved: canonical.unsaved,
        document_path: canonical.document_path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, GainInterpolation, GainPoint};

    #[test]
    fn preview_view_state_arc_shares_waveforms_and_reuses_unchanged_gain_preview() {
        let mut session = Session::new(48_000, 16, 1024).expect("session");
        let Created::Track(track) = session
            .command(Command::AddTrack {
                name: "lead".into(),
            })
            .expect("track")
        else {
            panic!("expected track");
        };
        session
            .command(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![
                    GainPoint {
                        incoming_gain: None,
                        beat: BeatTime::ZERO,
                        gain: 0.5,
                        outgoing: GainInterpolation::Linear,
                    },
                    GainPoint {
                        incoming_gain: None,
                        beat: BeatTime::ONE,
                        gain: 1.0,
                        outgoing: GainInterpolation::Linear,
                    },
                ],
            })
            .expect("gain envelope");
        let prepared = Arc::new(
            PreparedRevision::prepare(
                session.project(),
                &BTreeMap::new(),
                session.sine_table(),
                None,
            )
            .expect("prepare"),
        );
        let canonical = view_state(
            &session,
            Arc::clone(prepared.midi_preview()),
            1,
            1,
            true,
            None,
        );

        // Preview with unchanged gain lanes (e.g. loop region edit) shares all Arcs.
        let mut editor = hex_project::Editor::new((*canonical.project).clone(), 4).expect("editor");
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 48_000,
                enabled: true,
            })
            .expect("set loop");
        let same_gain = preview_view_state(
            &canonical,
            Arc::new(editor.project().clone()),
            Arc::clone(&prepared),
        );
        assert!(Arc::ptr_eq(&canonical.waveforms, &same_gain.waveforms));
        assert!(Arc::ptr_eq(
            &canonical.audio_clip_waveforms,
            &same_gain.audio_clip_waveforms
        ));
        assert!(Arc::ptr_eq(
            canonical.gain_preview.as_ref().unwrap(),
            same_gain.gain_preview.as_ref().unwrap()
        ));

        // Preview with modified gain lane recomputes gain_preview while still sharing waveform Arcs.
        let mut editor = hex_project::Editor::new((*canonical.project).clone(), 4).expect("editor");
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::ZERO,
                    gain: 0.25,
                    outgoing: GainInterpolation::Linear,
                }],
            })
            .expect("edit gain");
        let changed_gain = preview_view_state(
            &canonical,
            Arc::new(editor.project().clone()),
            Arc::clone(&prepared),
        );
        assert!(Arc::ptr_eq(&canonical.waveforms, &changed_gain.waveforms));
        assert!(Arc::ptr_eq(
            &canonical.audio_clip_waveforms,
            &changed_gain.audio_clip_waveforms
        ));
        assert!(!Arc::ptr_eq(
            canonical.gain_preview.as_ref().unwrap(),
            changed_gain.gain_preview.as_ref().unwrap()
        ));
        assert_eq!(
            changed_gain
                .gain_preview
                .as_ref()
                .unwrap()
                .value_at(track, 0),
            Some(0.25)
        );
    }
}
