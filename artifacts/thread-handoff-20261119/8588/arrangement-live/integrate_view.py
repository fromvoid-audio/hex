from pathlib import Path
base=Path('artifacts/8588/arrangement-live')
p=base/'waveform.rs';s=p.read_text()
a=s.index('pub(super) fn waveform_canvas(')
# Preserve every existing caller; the extra fallback belongs only to the new path.
body=s[a:]
head_end=body.index(') -> Div {')+len(') -> Div {')
sig=body[:head_end]
new=sig+'''
    waveform_canvas_with_fallback(waveform, mapping, visible_start, visible_end, cache, grid, ink, None)
}

pub(super) fn frame_source_waveform_view_cached(
    waveform: Arc<CachedWaveform>,
    projection: Arc<super::waveform_frame_source::Projection>,
    fallback: (Arc<CachedWaveform>, WaveformMapping),
    visible: Range<u64>,
    cache: Rc<WaveformViewCache>,
    grid: Option<WaveformGrid>,
    ink: u32,
) -> Div {
    waveform_canvas_with_fallback(waveform, WaveformMapping::FrameSource(projection), visible.start, visible.end, Some(cache), grid, ink, Some(fallback))
}

#[allow(clippy::too_many_arguments)] // Existing canvas inputs plus a whole-geometry refusal fallback.
fn waveform_canvas_with_fallback(
    waveform: Arc<CachedWaveform>, mapping: WaveformMapping,
    visible_start: u64, visible_end: u64, cache: Option<Rc<WaveformViewCache>>,
    grid: Option<WaveformGrid>, ink: u32,
    fallback: Option<(Arc<CachedWaveform>, WaveformMapping)>,
) -> Div {'''+body[head_end:]
s=s[:a]+new
old='''            let geometry = geometry.or_else(|| {
                prepare_waveform_geometry_with_scale(&waveform, &request)
                    .map(|geometry| (Arc::new(geometry), gpui::Point::default()))
            });'''
new='''            let geometry = geometry.or_else(|| {
                // A failed frame-source build has already spent its admitted
                // shared range work. Do not repeat that attempt without cache.
                if cache.is_some() && matches!(mapping, WaveformMapping::FrameSource(_)) {
                    return None;
                }
                prepare_waveform_geometry_with_scale(&waveform, &request)
                    .map(|geometry| (Arc::new(geometry), gpui::Point::default()))
            });
            let geometry = geometry.or_else(|| {
                let (source, mapping) = fallback.as_ref()?;
                let request = WaveformGeometryRequest { mapping, ..request };
                cache.as_ref().and_then(|cache| cached_waveform_geometry(cache, source, &request))
                    .or_else(|| prepare_waveform_geometry_with_scale(source, &request)
                        .map(|geometry| (Arc::new(geometry), gpui::Point::default())))
            });'''
assert old in s;s=s.replace(old,new,1)
p.write_text(s)
p=base/'tracks.rs';s=p.read_text()
a=s.index('''        let body = if let Some(prepared)''',s.index('    fn audio_clip_body('))
s=s[:a]+s[a:].replace('''        let body = if let Some(prepared) = state.audio_clip_waveforms.get(&clip.id()) {''','''        let body = if let Some(body) = self.frame_source_audio_clip_body(state, clip, visible_start..visible_end, grid, ink) {
            body
        } else if let Some(prepared) = state.audio_clip_waveforms.get(&clip.id()) {''',1)
p.write_text(s)
