//! Finite Arrangement clip-volume owners, prepared once per track. Rendering
//! resolves one row per note span; no callback scan or per-frame beat conversion.
use super::*;
use hex_project::{ClipId, MidiClipVolumePoint, NoteId, Project, Track};
mod identity;
pub(crate) use identity::ArrangementVolumeIdentity;
// Independent control-side bounds for prepared factor arrays and their curve
// metadata; these are not callback deadlines or exclusive-allocation proofs.
const MAX_VOICE_GAIN_PREPARATION_BYTES: usize = 64 * 1024 * 1024;

fn voice_identities(
    clip: &hex_project::MidiClip,
    source: MidiSourcePlayback,
) -> Result<Vec<hex_project::MidiVoiceGainIdentity>, Invalid> {
    let count = clip
        .notes()
        .iter()
        .filter(|note| note.active() && !note.voice_gain().layers().is_empty())
        .count();
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve arrangement voice gain identities"))?;
    if identities.capacity() > count {
        return Err(Invalid(
            "arrangement voice gain identity scratch exceeds its quote",
        ));
    }
    for note in clip.notes().iter().filter(|note| note.active()) {
        let Some(identity) = note.voice_gain().identity() else {
            continue;
        };
        if source
            .note_occurrences(note.start(), note.duration())
            .map_err(|error| Invalid(error.0))?
            .count()
            != 0
        {
            identities.push(identity);
        }
    }
    identities.sort_unstable();
    identities.dedup();
    Ok(identities)
}

/// Conservative pre-construction bounds for the two retained owner families,
/// the largest per-clip identity scratch, one old growable program buffer,
/// and one projected-curve temporary. No PCM or callback custody is implied.
pub(crate) struct VoicePreparationQuote {
    pub(crate) retained: usize,
    pub(crate) workspace: usize,
    pub(crate) data_rows: usize,
}
impl VoicePreparationQuote {
    fn from_counts(
        rows: usize,
        programs: usize,
        laws: usize,
        max_candidates: usize,
    ) -> Option<Self> {
        if rows == 0 {
            if programs != 0 || laws != 0 {
                return None;
            }
            return Some(Self {
                retained: 0,
                // An outer clip-volume program may still cause preparation to
                // scan a gain-bearing clip with no playable note occurrences.
                workspace: max_candidates
                    .checked_mul(std::mem::size_of::<hex_project::MidiVoiceGainIdentity>())?,
                data_rows: 0,
            });
        }
        if programs == 0 || programs > rows {
            return None;
        }
        // Vec::try_reserve's pinned amortized growth keeps final capacity below
        // twice its populated length, with a four-element minimum.
        let program_slots = programs.checked_mul(2)?.max(4);
        let program_buffer =
            program_slots.checked_mul(std::mem::size_of::<super::voice::PreparedVoiceGain>())?;
        let retained = rows
            .checked_mul(2)?
            .checked_mul(std::mem::size_of::<PreparedArrangementVoiceRow>())?
            .checked_add(program_buffer)?
            .checked_add(
                programs
                    .checked_mul(std::mem::size_of::<super::voice::PreparedVoiceGainIdentity>())?,
            )?
            .checked_add(laws)?;
        let workspace = max_candidates
            .checked_mul(std::mem::size_of::<hex_project::MidiVoiceGainIdentity>())?
            .checked_add(program_buffer)?
            .checked_add(
                (hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP + 3)
                    .checked_mul(std::mem::size_of::<MidiClipVolumePoint>())?,
            )?;
        let data_rows = programs.checked_mul(4)?.checked_add(4)?;
        Some(Self {
            retained,
            workspace,
            data_rows,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedArrangementVolumes {
    rows: Vec<PreparedArrangementClipVolume>,
    voice_rows: Vec<PreparedArrangementVoiceRow>,
    voice_programs: Vec<super::voice::PreparedVoiceGain>,
    metadata_bytes: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedArrangementVoiceRow {
    clip: ClipId,
    note: NoteId,
    start: BeatTime,
    source: MidiSourcePlayback,
    program: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct PreparedArrangementClipVolume {
    pub(crate) clip: ClipId,
    pub(crate) start: BeatTime,
    pub(crate) start_frame: u64,
    pub(crate) program: PreparedMidiClipVolume,
    pub(crate) endpoint: Option<VolumeEndpoint>,
}
impl PreparedArrangementVolumes {
    /// Count actual note rows and distinct source laws without constructing
    /// gain programs. Each clip has its own source geometry, so sharing is
    /// deliberately limited to a clip even when another clip shares its law.
    pub(crate) fn voice_preparation_quote(track: &Track) -> Result<VoicePreparationQuote, Invalid> {
        let mut rows = 0usize;
        let mut programs = 0usize;
        let mut laws = 0usize;
        let mut max_candidates = 0usize;
        for clip in track.midi_clips() {
            if !clip.active() || !clip.source().may_have_voice_gains() {
                continue;
            }
            let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
            let mut candidates = 0usize;
            for note in clip.notes().iter().filter(|note| note.active()) {
                if note.voice_gain().layers().is_empty() {
                    continue;
                }
                candidates = candidates
                    .checked_add(1)
                    .ok_or(Invalid("voice gain quote overflow"))?;
                if source
                    .note_occurrences(note.start(), note.duration())
                    .map_err(|error| Invalid(error.0))?
                    .count()
                    != 0
                {
                    rows = rows
                        .checked_add(1)
                        .ok_or(Invalid("voice gain quote overflow"))?;
                }
            }
            max_candidates = max_candidates.max(candidates);
            for identity in voice_identities(clip, source)? {
                programs = programs
                    .checked_add(1)
                    .ok_or(Invalid("voice gain quote overflow"))?;
                laws = laws
                    .checked_add(
                        super::voice::retained_law_quote(identity.layers())
                            .ok_or(Invalid("voice gain quote overflow"))?,
                    )
                    .ok_or(Invalid("voice gain quote overflow"))?;
            }
        }
        VoicePreparationQuote::from_counts(rows, programs, laws, max_candidates)
            .ok_or(Invalid("voice gain quote overflow"))
    }
    #[cfg(test)]
    pub(crate) fn test_voice<T: MidiTempoBreaks + ?Sized>(
        clip: ClipId,
        note: NoteId,
        start: BeatTime,
        source: MidiSourcePlayback,
        gain: &hex_project::MidiVoiceGain,
        timing: &T,
    ) -> Result<Self, Invalid> {
        let program = super::voice::PreparedVoiceGain::prepare(gain, source, timing)?;
        let metadata_bytes = program.retained_control_metadata_bytes();
        Ok(Self {
            rows: Vec::new(),
            voice_rows: vec![PreparedArrangementVoiceRow {
                clip,
                note,
                start,
                source,
                program: 0,
            }],
            voice_programs: vec![program],
            metadata_bytes,
        })
    }
    pub(crate) fn preparation_identity(&self) -> Result<ArrangementVolumeIdentity, Invalid> {
        ArrangementVolumeIdentity::prepare(self)
    }
    /// `track` must be the actual canonical owner in `project`; timing is its
    /// borrowed effective clock, never a newly prepared/cloned map. Empty curves
    /// are unity and need no rows, backing allocation, or curve metadata lease.
    pub(crate) fn prepare<T: MidiTempoBreaks + ?Sized>(
        project: &Project,
        track: &Track,
        timing: &T,
    ) -> Result<Option<Self>, Invalid> {
        if !project
            .track(track.id())
            .is_some_and(|owner| std::ptr::eq(owner, track))
            || timing.sample_rate() != project.sample_rate()
        {
            return Err(Invalid("clip volume arrangement owner/timing mismatch"));
        }
        let mut count = 0usize;
        let mut voice_count = 0usize;
        let mut points = 0usize;
        let mut metadata_bytes = 0usize;
        for clip in track.midi_clips() {
            if clip.active() && clip.source().may_have_voice_gains() {
                let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
                for note in clip
                    .notes()
                    .iter()
                    .filter(|note| note.active() && !note.voice_gain().layers().is_empty())
                {
                    if source
                        .note_occurrences(note.start(), note.duration())
                        .map_err(|error| Invalid(error.0))?
                        .count()
                        != 0
                    {
                        voice_count = voice_count
                            .checked_add(1)
                            .filter(|count| {
                                *count <= crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES
                            })
                            .ok_or(Invalid("arrangement voice gain row limit reached"))?;
                    }
                }
            }
            if clip.volume().is_unity() {
                continue;
            }
            count = count
                .checked_add(1)
                .filter(|n| *n <= hex_project::MAX_CLIPS)
                .ok_or(Invalid("arrangement clip volume row limit reached"))?;
            points = points
                .checked_add(clip.volume().points().len())
                .filter(|n| *n <= hex_project::MAX_MIDI_VOLUME_POINTS)
                .ok_or(Invalid("arrangement clip volume point limit reached"))?;
            metadata_bytes = metadata_bytes
                .checked_add(
                    clip.volume()
                        .retained_control_metadata_bytes()
                        .ok_or(Invalid("arrangement clip volume metadata size overflow"))?,
                )
                .ok_or(Invalid("arrangement clip volume metadata size overflow"))?;
        }
        if count == 0 && voice_count == 0 {
            return Ok(None);
        }
        check_budget(count, metadata_bytes)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve arrangement clip volume rows"))?;
        check_budget(rows.capacity(), metadata_bytes)?;
        let mut voice_rows = Vec::new();
        voice_rows
            .try_reserve_exact(voice_count)
            .map_err(|_| Invalid("could not reserve arrangement voice gain rows"))?;
        if voice_rows.capacity() > voice_count {
            return Err(Invalid("arrangement voice gain rows exceed their quote"));
        }
        let mut voice_programs = Vec::new();
        let mut voice_program_bytes = 0usize;
        for clip in track.midi_clips() {
            if clip.active() && clip.source().may_have_voice_gains() {
                let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
                let shared = voice_identities(clip, source)?;
                let program_start = voice_programs.len();
                voice_programs
                    .try_reserve(shared.len())
                    .map_err(|_| Invalid("could not reserve arrangement voice gain programs"))?;
                let program_limit = program_start
                    .checked_add(shared.len())
                    .and_then(|n| n.checked_mul(2))
                    .map(|n| if n == 0 { 0 } else { n.max(4) })
                    .ok_or(Invalid("arrangement voice gain program quote overflow"))?;
                if voice_programs.capacity() > program_limit {
                    return Err(Invalid(
                        "arrangement voice gain programs exceed their quote",
                    ));
                }
                for identity in &shared {
                    let prepared = super::voice::PreparedVoiceGain::prepare_layers(
                        identity.layers(),
                        source,
                        timing,
                    )?;
                    voice_program_bytes = voice_program_bytes
                        .checked_add(
                            prepared
                                .allocated_program_bytes()
                                .ok_or(Invalid("arrangement voice gain size overflow"))?,
                        )
                        .filter(|bytes| *bytes <= MAX_VOICE_GAIN_PREPARATION_BYTES)
                        .ok_or(Invalid("arrangement voice gain program limit reached"))?;
                    metadata_bytes = metadata_bytes
                        .checked_add(prepared.retained_control_metadata_bytes())
                        .filter(|bytes| *bytes <= MAX_VOICE_GAIN_PREPARATION_BYTES)
                        .ok_or(Invalid("arrangement voice gain metadata limit reached"))?;
                    voice_programs.push(prepared);
                }
                for note in clip.notes().iter().filter(|note| note.active()) {
                    let Some(identity) = note.voice_gain().identity() else {
                        continue;
                    };
                    if source
                        .note_occurrences(note.start(), note.duration())
                        .map_err(|error| Invalid(error.0))?
                        .count()
                        == 0
                    {
                        continue;
                    }
                    let program = program_start
                        .checked_add(
                            shared
                                .binary_search(&identity)
                                .map_err(|_| Invalid("arrangement voice gain identity missing"))?,
                        )
                        .ok_or(Invalid("arrangement voice gain program index overflow"))?;
                    voice_rows.push(PreparedArrangementVoiceRow {
                        clip: clip.id(),
                        note: note.id(),
                        start: clip.start(),
                        source,
                        program,
                    });
                }
            }
            if clip.volume().is_unity() {
                continue;
            }
            let program = PreparedMidiClipVolume::prepare(
                clip.volume(),
                clip.source_playback().map_err(|error| Invalid(error.0))?,
                timing,
            )?;
            let endpoint = program.finite_endpoint(
                timing,
                VolumePlacement::Arrangement {
                    start: clip.start(),
                },
            )?;
            let start_frame = timing
                .beat_to_frame(clip.start())
                .map_err(|_| Invalid("arrangement clip volume start mapping is unavailable"))?;
            if endpoint.is_some_and(|end| end.frame < start_frame) {
                return Err(Invalid("arrangement clip volume end precedes start"));
            }
            rows.push(PreparedArrangementClipVolume {
                clip: clip.id(),
                start: clip.start(),
                start_frame,
                program,
                endpoint,
            });
        }
        rows.sort_unstable_by_key(|row| row.clip);
        voice_rows.sort_unstable_by_key(|row| (row.clip, row.note));
        if voice_rows
            .windows(2)
            .any(|pair| (pair[0].clip, pair[0].note) == (pair[1].clip, pair[1].note))
        {
            return Err(Invalid("duplicate arrangement voice gain identity"));
        }
        if rows.windows(2).any(|pair| pair[0].clip == pair[1].clip) {
            return Err(Invalid("duplicate arrangement clip volume identity"));
        }
        Ok(Some(Self {
            rows,
            voice_rows,
            voice_programs,
            metadata_bytes,
        }))
    }
    pub(crate) fn clip(&self, clip: ClipId) -> Option<&PreparedArrangementClipVolume> {
        self.rows
            .binary_search_by_key(&clip, |row| row.clip)
            .ok()
            .and_then(|index| self.rows.get(index))
    }
    pub(crate) fn voice(
        &self,
        clip: ClipId,
        note: NoteId,
    ) -> Option<(
        &PreparedArrangementVoiceRow,
        &super::voice::PreparedVoiceGain,
    )> {
        let index = self
            .voice_rows
            .binary_search_by_key(&(clip, note), |row| (row.clip, row.note))
            .ok()?;
        let row = self.voice_rows.get(index)?;
        Some((row, self.voice_programs.get(row.program)?))
    }
    pub(crate) fn has_voice_gains(&self) -> bool {
        !self.voice_rows.is_empty()
    }
    /// Actual Vec backing for the existing root ledger's inspection. Borrowing
    /// it conveys no declaration, allocation identity, or closed-storage proof.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> &Vec<PreparedArrangementClipVolume> {
        &self.rows
    }
    pub(crate) fn allocated_row_bytes(&self) -> Result<usize, Invalid> {
        self.rows
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedArrangementClipVolume>())
            .and_then(|n| {
                n.checked_add(
                    self.voice_rows
                        .capacity()
                        .checked_mul(std::mem::size_of::<PreparedArrangementVoiceRow>())?,
                )
            })
            .and_then(|n| {
                n.checked_add(
                    self.voice_programs
                        .capacity()
                        .checked_mul(std::mem::size_of::<super::voice::PreparedVoiceGain>())?,
                )
            })
            .and_then(|n| {
                self.voice_programs.iter().try_fold(n, |n, program| {
                    n.checked_add(program.allocated_program_bytes()?)
                })
            })
            .ok_or(Invalid("arrangement clip volume row size overflow"))
    }
    /// Conservative curve Arc header/payload count, charged once per row even
    /// if canonical owners share a curve. Inline/Vec rows are accounted separately.
    pub(crate) fn retained_control_metadata_bytes(&self) -> usize {
        self.metadata_bytes
    }
}
impl PreparedArrangementVoiceRow {
    pub(crate) fn start(&self) -> BeatTime {
        self.start
    }
    pub(crate) fn pass_end<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        onset: BeatTime,
    ) -> Result<u64, Invalid> {
        let elapsed = onset
            .raw()
            .checked_sub(self.start.raw())
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain onset precedes clip"))?;
        let end = self
            .source
            .occurrence_gain_pass_end(elapsed)
            .map_err(|error| Invalid(error.0))?;
        let end = self
            .start
            .raw()
            .checked_add(end.raw())
            .map(BeatTime::from_raw)
            .ok_or(Invalid("voice gain end beat overflow"))?;
        timing
            .beat_to_frame(end)
            .map_err(|_| Invalid("voice gain end exceeds frame range"))
    }
}
impl PreparedArrangementClipVolume {
    /// Use the same prepared clock as this row. Outgoing tails retain this
    /// owner's evaluated endpoint scalar, including through later handoffs.
    pub(crate) fn segment_or_hold<T: MidiTempoBreaks + ?Sized>(
        &self,
        timing: &T,
        frame: u64,
        frames: usize,
    ) -> Result<VolumeRamp, Invalid> {
        if frames == 0 || frames > VOLUME_QUERY_FRAMES || frame.checked_add(frames as u64).is_none()
        {
            return Err(Invalid(
                "arrangement clip volume query exceeds frame capacity",
            ));
        }
        if frame < self.start_frame {
            return Err(Invalid("arrangement clip volume query precedes clip"));
        }
        let endpoint = self
            .endpoint
            .ok_or(Invalid("clip volume source window is silent"))?;
        if frame >= endpoint.frame {
            return Ok(VolumeRamp {
                start_frame: endpoint.frame,
                end_frame: u64::MAX,
                start_gain: endpoint.gain,
                end_gain: endpoint.gain,
                next_boundary: None,
            });
        }
        self.program
            .segment_at(
                timing,
                VolumePlacement::Arrangement { start: self.start },
                frame,
                frames,
            )?
            .ok_or(Invalid(
                "arrangement clip volume active segment is unavailable",
            ))
    }
}
fn check_budget(rows: usize, metadata: usize) -> Result<(), Invalid> {
    // The model's global row/point limits bound this control owner. Metadata
    // includes conservative Arc headers/alignment, not just authored payload.
    let overhead = 2usize
        .checked_mul(std::mem::size_of::<usize>())
        .and_then(|n| {
            n.checked_add(
                std::mem::align_of::<usize>().max(std::mem::align_of::<MidiClipVolumePoint>()) - 1,
            )
        })
        .ok_or(Invalid("arrangement clip volume metadata budget overflow"))?;
    let max_metadata = hex_project::MAX_MIDI_VOLUME_POINTS
        .checked_mul(std::mem::size_of::<MidiClipVolumePoint>())
        .and_then(|n| n.checked_add(hex_project::MAX_CLIPS.checked_mul(overhead)?))
        .ok_or(Invalid("arrangement clip volume metadata budget overflow"))?;
    let row_bytes = rows
        .checked_mul(std::mem::size_of::<PreparedArrangementClipVolume>())
        .ok_or(Invalid("arrangement clip volume row size overflow"))?;
    row_bytes
        .checked_add(metadata)
        .ok_or(Invalid("arrangement clip volume total size overflow"))?;
    if rows > hex_project::MAX_CLIPS || metadata > max_metadata {
        return Err(Invalid(
            "arrangement clip volume exceeds bounded metadata budget",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;
