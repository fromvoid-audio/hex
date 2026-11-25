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
        waveforms: session.waveforms(),
        source_overviews: session
            .source_overviews()
            .at_context(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            }),
        audio_clip_waveforms: session.audio_clip_waveforms().unwrap_or_default(),
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
    Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        gain_preview: hex_engine::GainAutomationPreview::prepare(&project).map(Arc::new),
        project,
        waveforms: canonical.waveforms.clone(),
        source_overviews: canonical.source_overviews.clone(),
        audio_clip_waveforms: canonical.audio_clip_waveforms.clone(),
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
