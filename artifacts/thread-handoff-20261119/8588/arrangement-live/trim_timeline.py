from pathlib import Path
p=Path('artifacts/8588/arrangement-live/waveform_frame_source.rs')
s=p.read_text().replace('FrameFadeEvent, ', '')
a=s.index('#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub(super) enum Coordinates')
b=s.index('/// Complete effective geometry identity',a)
s=s[:a]+s[b:]
s=s.replace('    coordinates: Coordinates,\n','')
a=s.index('        match self.coordinates {',s.index('impl Key'))
b=s.index('\n    }\n}',a)
s=s[:a]+'        self.start'+s[b:]
s=s.replace('        coordinates: Coordinates,\n','')
a=s.index('        if let Coordinates::Event(id) = coordinates {')
b=s.index('        let mut root',a)
s=s[:a]+s[b:]
s=s.replace('            coordinates,\n','')
a=s.index('        let clock = match self.coordinates {')
b=s.index('        Some(Sampler {',a)
s=s[:a]+s[b:]
s=s.replace('            clock,','            query,')
a=s.index('enum Clock')
b=s.index('pub(super) struct Sampler',a)
s=s[:a]+s[b:]
s=s.replace("    clock: Clock<'p, 'q>,", "    query: &'q FrameFadePreview<'p>,")
s=s.replace('self.clock.sample(frame)?','self.query.timeline_sample(frame).ok()?')
s=s.replace('self.clock\n            .visit(range, &mut self.range_remaining, |part| {','self.query\n            .visit_timeline_ranges(range, &mut self.range_remaining, |part| {')
s=s.replace('''            })?;''','''            }).ok()??;''',1) if False else s
# The range call now returns Result<Option<Coverage>>; preserve both refusal layers.
a=s.index('        let coverage = ')
b=s.index('        if !valid',a)
segment=s[a:b]
segment=segment.replace('        })?;', '        }).ok()??;')
s=s[:a]+segment+s[b:]
p.write_text(s)
p=Path('artifacts/8588/arrangement-live/waveform_frame_source/tests.rs')
s=p.read_text().replace('    coordinates: Coordinates,\n','').replace('        coordinates,\n','').replace('        Coordinates::Timeline,\n','')
s=s.replace('projection(&accepted, clip, &waveform, Coordinates::Timeline)','projection(&accepted, clip, &waveform)')
s=s.replace('projection(&editor, clip, &waveform, Coordinates::Timeline)','projection(&editor, clip, &waveform)')
s=s.replace('projection(&editor, clip, &waveform, Coordinates::Event(id))','projection(&editor, clip, &waveform)')
s=s.replace('''    let column = sampler.exact(32..40, 8).unwrap();''','''    let column = sampler.exact(row.start()..row.start() + 8, 8).unwrap();''')
s=s.replace('canonical_cropped_event_uses_the_same_virtual_id_and_original_samples','canonical_cropped_timeline_uses_original_samples')
p.write_text(s)
