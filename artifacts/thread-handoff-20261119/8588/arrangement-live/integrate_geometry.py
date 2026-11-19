from pathlib import Path
base=Path('artifacts/8588/arrangement-live')
p=base/'waveform_frame_source.rs';s=p.read_text();s=s.replace('''    pub(super) fn key(&self) -> Key {''','''    pub(super) fn clip(&self) -> &Clip { &self.clip }
    pub(super) fn key(&self) -> Key {''');p.write_text(s)
p=base/'waveform_geometry.rs';s=p.read_text()
s=s.replace('''enum WaveformMappingKey {
''','''enum WaveformMappingKey {
    FrameSource(super::waveform_frame_source::Key),
''',1)
s=s.replace('''pub(super) enum WaveformMapping {
''','''pub(super) enum WaveformMapping {
    FrameSource(Arc<super::waveform_frame_source::Projection>),
''',1)
a=s.index('''        let WaveformMapping::Clip {''',s.index('impl OverrangeCache'))
b=s.index('''        if let Some(index)''',a)
s=s[:a]+'''        let (clip, timeline_origin, fade, frame_source) = match mapping {
            WaveformMapping::Clip { clip, timeline_origin, gain: None, fade: Some(fade) } =>
                (clip, *timeline_origin, Some(*fade), None),
            WaveformMapping::FrameSource(projection) =>
                (projection.clip(), projection.clip().start(), None, Some(Arc::clone(projection))),
            _ => return WaveformOverrange::NotApplicable,
        };
''' +s[b:]
s=s.replace('''            timeline_origin: *timeline_origin,
            fade: Some(*fade),
            columns,
''','''            timeline_origin,
            fade,
            frame_source,
            columns,
''',1)
# Existing direct request test fixture uses the original plain-clip route.
s=s.replace('''        let filler = super::super::waveform_overrange::Request {
''','''        let filler = super::super::waveform_overrange::Request {
            frame_source: None,
''',1)
a=s.index('''    let WaveformMappingKey::Clip {''',s.index('fn coarse_entry_key('))
b=s.index('''    let WaveformPlacementKey::Coarse''',a)
s=s[:a]+'''    let (id, timeline_origin) = match &key.mapping {
        WaveformMappingKey::Clip { id, timeline_origin, .. } => (*id, *timeline_origin),
        WaveformMappingKey::FrameSource(key) => (key.clip_id, key.timeline_origin()),
        _ => return None,
    };
''' +s[b:]
a=s.index('''    Some(CoarseEntryKey {''',s.index('fn coarse_entry_key('));b=s.index('''\n    })''',a)
part=s[a:b].replace('clip_id: *id','clip_id: id').replace('timeline_origin: *timeline_origin','timeline_origin')
s=s[:a]+part+s[b:]
s=s.replace('''    if matches!(request.mapping, WaveformMapping::Clip { .. })
''','''    if matches!(request.mapping, WaveformMapping::Clip { .. } | WaveformMapping::FrameSource(_))
''',1)
a=s.index('''            if let WaveformMappingKey::Clip {''',s.index('pub(super) fn cached_waveform_geometry'))
b=s.index('''            key.placement = local.key;''',a)
s=s[:a]+'''            match &mut key.mapping {
                WaveformMappingKey::Clip { id, transformed: false, .. } => *id = None,
                WaveformMappingKey::FrameSource(key) => key.clip_id = None,
                _ => {}
            }
''' +s[b:]
s=s.replace('''if matches!(request.mapping, WaveformMapping::Clip { fade: Some(_), .. })''','''if matches!(request.mapping, WaveformMapping::Clip { fade: Some(_), .. } | WaveformMapping::FrameSource(_))''')
s=s.replace('''    let mapping = match mapping {
''','''    let mapping = match mapping {
        WaveformMapping::FrameSource(projection) => WaveformMappingKey::FrameSource(projection.key()),
''',1)
s=s.replace('''    let gain = match mapping {
''','''    let gain = match mapping {
        WaveformMapping::FrameSource(_) => 1.0,
''',1)
a=s.index('''    let mut stereo_peaks = ''',s.index('fn build_waveform_geometry'))
s=s[:a]+'''    let frame_query = match mapping {
        WaveformMapping::FrameSource(projection) => Some(projection.query()?),
        _ => None,
    };
    let mut frame_sampler = match (mapping, frame_query.as_ref()) {
        (WaveformMapping::FrameSource(projection), Some(query)) => Some(projection.sampler(query, waveform)?),
        _ => None,
    };
''' +s[a:]
s=s.replace('''        let peaks = match mapping {
''','''        let peaks = match mapping {
            WaveformMapping::FrameSource(_) => Some(frame_sampler.as_mut()?.peaks(start..end, detail_budget)?),
''',1)
p.write_text(s)
p=base/'waveform_overrange.rs';s=p.read_text()
s=s.replace('''    pub columns: Vec<Range<u64>>,
''','''    pub columns: Vec<Range<u64>>,
    pub frame_source: Option<Arc<super::waveform_frame_source::Projection>>,
''',1)
s=s.replace('''    let mut budget = MAX_FRAMES;
''','''    let frame_query = request.frame_source.as_ref().and_then(|projection| projection.query());
    let mut frame_sampler = request.frame_source.as_ref().zip(frame_query.as_ref())
        .and_then(|(projection, query)| projection.sampler(query, &request.waveform));
    let mut budget = MAX_FRAMES;
''',1)
a=s.index('''            let next = bounded_clip_overrange(''');b=s.index('''            result = result.zip(next)''',a)
s=s[:a]+'''            let next = if request.frame_source.is_some() {
                frame_sampler.as_mut().and_then(|sampler| sampler.exact(start..end, CHUNK_FRAMES as usize)).map(|column| column.overrange)
            } else {
                bounded_clip_overrange(&request.waveform, &request.clip, request.timeline_origin, start..end, request.fade, CHUNK_FRAMES as usize)
            };
''' +s[b:]
s=s.replace('''        Request {
            id,''','''        Request {
            frame_source: None,
            id,''',1)
p.write_text(s)
