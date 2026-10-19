from pathlib import Path
p=Path('crates/hex-warp/src/lib.rs');s=p.read_text();a=s.index('    let pitch = 2.0_f64.powf', s.index('pub fn render('));b=s.index('    let base_scratch',a);loop=s[a:b]
method='''    /// Validate the renderer's exact per-segment stretch and pitch capability.
    /// Metadata only; the existing map constructor owns extent/segment validity.
    /// This does not admit PCM, algorithm settings, scratch or render work.
    pub fn validate_stretch(&self, mode: Mode, pitch_cents: i16) -> Result<(), Error> {
        if !(-4_800..=4_800).contains(&pitch_cents) {
            return Err(Error::Invalid("invalid audio transform extent or pitch"));
        }
'''+''.join('    '+line+'\n' for line in loop.replace('request.pitch_cents','pitch_cents').replace('request.mode','mode').replace('request.map.segments()','self.segments()').splitlines())+'''        Ok(())
    }
'''
s=s[:a]+'''    request.map.validate_stretch(request.mode, request.pitch_cents)?;
    let pitch = 2.0_f64.powf(f64::from(request.pitch_cents) / 1_200.0);
'''+s[b:]
needle='    pub fn segments(&self) -> &[MapSegment] {';assert needle in s;s=s.replace(needle,method+needle);p.write_text(s)
p=Path('crates/hex-engine/src/audio_transform/mod.rs');s=p.read_text();a=s.index('        let mode = if domain.warp_enabled {');b=s.index('        let input = hex_warp::Input {',a);mode=s[a:b]
func='''fn render_mode(domain: &AudioClipDomain) -> Result<hex_warp::Mode, Invalid> {
    Ok('''+mode.strip().removeprefix('let mode = ').removesuffix(';')+''')
}

/// Check the existing renderer's projected stretch capability without PCM or
/// preparation/cache ownership. Allocates the same bounded metadata map (at most
/// 8192 segments and 1024 explicit attacks); this is control-side work, not a
/// callback API and not a proof of available render memory or work budget.
pub fn validate_clip_warp_preview(clip: &hex_project::Clip, tempo: &hex_project::TempoMap) -> Result<(), Invalid> {
    let Some(edit) = clip.audio_edit() else { return Ok(()); };
    let (map, _) = mapping::prepare_map(edit, tempo, &AtomicBool::new(false))?;
    map.validate_stretch(render_mode(&edit.domain)?, edit.domain.pitch_cents).map_err(warp_error)
}

'''
s=s[:a]+'''        let mode = render_mode(domain)?;
'''+s[b:]
needle='pub const MAX_CACHED_DERIVED_BYTES:';idx=s.index(needle);s=s[:idx]+func+s[idx:];p.write_text(s)
