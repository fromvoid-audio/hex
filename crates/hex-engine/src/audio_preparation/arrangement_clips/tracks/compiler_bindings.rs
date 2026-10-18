//! Nonowning correlation of actual compiler output with existing reserved rows.
//! Observations do not confer constructor provenance or authorize root adoption.
use super::*;
use crate::compile::PreparedAudioSourceRef;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CompilerClipOutcome {
    #[default]
    Unseen,
    Resolved,
    Omitted,
    ResidentOriginal(usize),
    StreamedOriginal(usize),
    Transformed(usize),
}
pub(crate) struct CompilerSourceBindings<'a> {
    tracks: &'a mut TrackAssembly,
    returns: &'a mut super::super::returns::ReturnAssembly,
    bank: &'a OwnedAudioInputs,
}
impl<'a> CompilerSourceBindings<'a> {
    pub(super) fn new(
        tracks: &'a mut TrackAssembly,
        returns: &'a mut super::super::returns::ReturnAssembly,
        project: &'a hex_project::Project,
        bank: &'a OwnedAudioInputs,
    ) -> Result<Self, AudioStorageError> {
        if tracks.phase != Phase::Installed
            || tracks.compiler_started
            || tracks.active != project.tracks().len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Whole-row freshness preflight before any cursor/outcome changes. No
        // identity is caller asserted and no owning input is cloned or taken.
        for (ordinal, (row, track)) in tracks.rows[..tracks.active]
            .iter()
            .zip(project.tracks())
            .enumerate()
        {
            if row.id != Some(track.id())
                || row.ordinal != ordinal
                || row.compiler_cursor != 0
                || row.compiler_output != 0
                || row.compiler_complete
                || row.pairs.len() != track.clips().len()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            for (pair, clip) in row.pairs.iter().zip(track.clips()) {
                let (asset, _) = bank
                    .sources
                    .get(pair.origin.source)
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if pair.origin.track != track.id()
                    || pair.origin.clip != clip.id()
                    || *asset != clip.asset()
                    || pair.compiled != CompilerClipOutcome::Unseen
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
        }
        // This route correlates existing in-place owners, not merely matching
        // asset IDs. Reserved rows and externally moved roots have no such
        // local ownership observation and remain outside this route.
        tracks.preflight_root_move()?;
        tracks.validate_root_sources(&[], bank)?;
        returns.compiler_preflight(project)?;
        // One attempt starts before any compiler owner birth or source lookup,
        // including an error on the very first lookup or an empty track set.
        tracks.compiler_started = true;
        Ok(Self {
            tracks,
            returns,
            bank,
        })
    }
    pub(crate) fn resolved(
        &mut self,
        track: usize,
        clip: usize,
        source: &PreparedAudioSourceRef<'_>,
    ) -> Result<(), AudioStorageError> {
        let row = self
            .tracks
            .rows
            .get_mut(track)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let pair = row
            .pairs
            .get_mut(clip)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if row.compiler_cursor != clip || pair.compiled != CompilerClipOutcome::Unseen {
            return Err(AudioStorageError::SourceMismatch);
        }
        let (_, actual) = self
            .bank
            .sources
            .get(pair.origin.source)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let matches = match (&actual.kind, source) {
            (SourceKind::Resident(original), PreparedAudioSourceRef::Resident(resolved)) => {
                original.shares_pcm_with(resolved)
            }
            (SourceKind::Streamed(original), PreparedAudioSourceRef::Streamed(resolved)) => {
                Arc::ptr_eq(original.arc(), resolved)
            }
            _ => false,
        };
        if !matches {
            return Err(AudioStorageError::SourceMismatch);
        }
        pair.compiled = CompilerClipOutcome::Resolved;
        Ok(())
    }
    pub(crate) fn produced(
        &mut self,
        track: usize,
        clip: usize,
        output_index: usize,
        output: Option<&Clip>,
    ) -> Result<(), AudioStorageError> {
        let row = self
            .tracks
            .rows
            .get_mut(track)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let pair = row
            .pairs
            .get_mut(clip)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if row.compiler_cursor != clip
            || row.compiler_output != output_index
            || pair.compiled != CompilerClipOutcome::Resolved
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let (_, original) = self
            .bank
            .sources
            .get(pair.origin.source)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let outcome = match output {
            None => CompilerClipOutcome::Omitted,
            Some(output) => match (
                &original.kind,
                &output.audio,
                output
                    .metadata
                    .as_deref()
                    .and_then(|metadata| metadata.streamed.as_ref()),
            ) {
                (SourceKind::Resident(original), Some(audio), None)
                    if original.shares_pcm_with(audio) =>
                {
                    CompilerClipOutcome::ResidentOriginal(output_index)
                }
                (SourceKind::Streamed(original), None, Some(stream))
                    if Arc::ptr_eq(original.arc(), stream) =>
                {
                    CompilerClipOutcome::StreamedOriginal(output_index)
                }
                (_, Some(_), None) => CompilerClipOutcome::Transformed(output_index),
                _ => return Err(AudioStorageError::SourceMismatch),
            },
        };
        // Input bounds cap both increments. These are scalar records only;
        // transformed owners remain unqualified until their own actual route.
        row.compiler_cursor += 1;
        row.compiler_output += usize::from(output.is_some());
        pair.compiled = outcome;
        Ok(())
    }
    pub(crate) fn finish_track(
        &mut self,
        track: usize,
        output: &Track,
    ) -> Result<(), AudioStorageError> {
        let row = self
            .tracks
            .rows
            .get_mut(track)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if row.compiler_complete
            || output.track_id != row.id
            || row.compiler_cursor != row.pairs.len()
            || row.compiler_output != output.clips.len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Called only after the common compiler moved the whole Track into its
        // actual retained Project. Earlier per-clip outcomes are provisional.
        row.compiler_complete = true;
        Ok(())
    }
    pub(crate) fn returned(
        &mut self,
        index: usize,
        source: &hex_project::ReturnTrack,
        output: &crate::ReturnBus,
        rate: u32,
    ) -> Result<(), AudioStorageError> {
        self.returns.compiler_observe(index, source, output, rate)
    }
}
