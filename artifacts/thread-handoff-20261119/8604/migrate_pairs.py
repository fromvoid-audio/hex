from pathlib import Path
import re
root=Path('artifacts/8604/source')
changed=[]
for p in root.rglob('*.rs'):
    s=p.read_text(); old=s
    # Replace captured range pairs; these become one exact geometry witness.
    s=re.sub(r'(pub\(super\) )?pitch_low: u8,\s*\n\s*(?:pub\(super\) )?pitch_rows: u8,',r'\1pitch: crate::editor::PitchViewport,',s)
    def pair(m):
        a,b=m[1],m[2]
        if a.endswith('.piano_pitch_low') and b == a[:-len('low')]+'rows':
            return 'pitch: '+a[:-len('_low')]+','
        if a.endswith('.pitch_low') and b == a[:-len('low')]+'rows':
            return 'pitch: '+a[:-len('_low')]+','
        return f'pitch: crate::editor::PitchViewport::from_rows({a}, {b}).unwrap(),'
    # Literal constructors are test fixtures except the few listed adapters;
    # adapters will be repaired explicitly before this artifact is a candidate.
    s=re.sub(r'pitch_low:\s*([^,\n]+),\s*\n\s*pitch_rows:\s*([^,\n]+),',pair,s)
    s=re.sub(r'pitch_low,\s*\n\s*pitch_rows,', 'pitch,', s)
    # Collapse exact capture identity checks before rewriting numeric readers.
    s=re.sub(r'([\w.]+)\.pitch_low == ([\w.]+)\.pitch_low\s*\n\s*&& \1\.pitch_rows == \2\.pitch_rows',r'\1.pitch == \2.pitch',s)
    s=re.sub(r'self\.piano_pitch_low == g\.context\.pitch_low\s*\n\s*&& self\.piano_pitch_rows == g\.context\.pitch_rows', 'self.piano_pitch == g.context.pitch',s)
    if s != old:
        p.write_text(s); changed.append(str(p.relative_to(root)))
print(f'{len(changed)} artifact files have pair-carrier migration edits; not yet a compilable product patch.')
for p in changed: print(p)
# Single state declaration + stable known default, no second authoritative pair.
p=root/'crates/hex-ui/src/editor/mod.rs';s=p.read_text().replace('mod midi_scene;', 'mod midi_scene;\nmod midi_pitch_viewport;\npub(super) use midi_pitch_viewport::PitchViewport;')
s=s.replace('pub(in crate::editor) piano_pitch_low: u8,\n    piano_pitch_rows: u8,','pub(in crate::editor) piano_pitch: PitchViewport,')
s=s.replace('piano_pitch_low: 48,\n            piano_pitch_rows: 24,','piano_pitch: PitchViewport::DEFAULT,')
p.write_text(s)
p=root/'crates/hex-ui/tests/piano_roll_unit/editor.rs';s=p.read_text();s='#[path = "../../src/editor/midi_pitch_viewport.rs"]\nmod midi_pitch_viewport;\npub(super) use midi_pitch_viewport::PitchViewport;\n'+s;p.write_text(s)
