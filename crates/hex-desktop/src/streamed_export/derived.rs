//! Cancellation checks and derived-source (granular/repitch) projection for
//! offline streamed export, bounce, and seek preroll.
use hex_export::{ExportControl, ExportError};
use hex_session::VerifiedAudioFile;
use hex_stream::SourceDefinition;
use std::{
    borrow::Cow,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) trait Cancellation {
    fn is_cancelled(&self) -> bool;
}

impl Cancellation for ExportControl {
    fn is_cancelled(&self) -> bool {
        ExportControl::is_cancelled(self)
    }
}

impl Cancellation for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}

pub(super) fn check(control: &impl Cancellation) -> Result<(), ExportError> {
    if control.is_cancelled() {
        Err(ExportError::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn media(error: impl std::fmt::Display) -> ExportError {
    ExportError::Media(error.to_string())
}

/// Source facts for derived pages. An offline bounce can also borrow its
/// isolated Project when there is no retained requirement to look up.
#[derive(Clone, Copy)]
pub(super) enum DerivedCapture<'a> {
    Canonical(&'a hex_session::service::CanonicalAudioSourceSnapshot),
    Captured(&'a [hex_engine::audio_transform::StreamedGranularRequirement]),
    Bounce(&'a hex_session::StreamedBounceRequest<'a>),
}

impl DerivedCapture<'_> {
    pub(super) fn granular_root(
        &self,
        expected: &hex_engine::StreamedSourceMetadata,
        file: &VerifiedAudioFile,
        control: &impl Cancellation,
    ) -> Result<hex_engine::audio_transform::StreamedGranularRoot, ExportError> {
        let raw = file
            .source_metadata(expected.key().asset(), expected.key().generation())
            .map_err(ExportError::Render)?;
        if !expected.is_granular_of(&raw) {
            return Err(ExportError::Invalid("granular export source facts differ"));
        }
        let captured = match self {
            Self::Canonical(source) => {
                for result in source.required_granular_roots(expected.key().generation()) {
                    check(control)?;
                    let (candidate, root) = result.map_err(ExportError::Render)?;
                    if candidate == file && root.metadata() == expected {
                        return Ok(root);
                    }
                }
                &[]
            }
            Self::Captured(requirements) => *requirements,
            Self::Bounce(request) => {
                let key = expected.key();
                for result in request.projected_granular_roots(key.asset(), key.generation()) {
                    check(control)?;
                    let root = result.map_err(ExportError::Render)?;
                    if root.metadata() == expected {
                        return Ok(root);
                    }
                }
                request.granulars()
            }
        };
        for requirement in captured {
            check(control)?;
            if requirement
                .metadata_for(&raw)
                .is_ok_and(|metadata| &metadata == expected)
            {
                return requirement
                    .try_owned_root(&raw, &AtomicBool::new(false))
                    .map_err(ExportError::Render);
            }
        }
        Err(ExportError::Invalid(
            "verified granular requirement missing or differs",
        ))
    }

    pub(super) fn repitch_definition(
        &self,
        metadata: &hex_engine::StreamedSourceMetadata,
        raw: hex_engine::StreamedSourceMetadata,
        file: &VerifiedAudioFile,
        retained: &[hex_engine::audio_transform::StreamedRepitchRequirement],
        available: usize,
        control: &impl Cancellation,
    ) -> Result<(SourceDefinition, usize), ExportError> {
        let matches = |requirement: &hex_engine::audio_transform::StreamedRepitchRequirement| {
            requirement
                .metadata_for(&raw)
                .is_ok_and(|expected| &expected == metadata)
        };
        let mut requirement = retained
            .iter()
            .find(|requirement| matches(requirement))
            .map(Cow::Borrowed);
        let budget_error = ExportError::Invalid("repitch map reservation exceeded");
        if requirement.is_none()
            && let Self::Bounce(request) = self
            && let Some(projected) =
                request.projected_repitch_requirements(raw.key().asset(), raw.key().generation())
        {
            let workspace = hex_engine::audio_transform::native_metadata_workspace_bytes()
                .ok_or(ExportError::Invalid("repitch workspace overflow"))?;
            if workspace > available {
                return Err(budget_error);
            }
            check(control)?;
            for candidate in projected {
                check(control)?;
                let candidate = candidate.map_err(ExportError::Render)?;
                if matches(&candidate) {
                    requirement = Some(Cow::Owned(candidate));
                    break;
                }
            }
        }
        let requirement = requirement.ok_or(ExportError::Invalid(
            "verified repitch requirement missing or differs",
        ))?;
        let bytes = requirement
            .map()
            .backing_bytes()
            .ok_or(ExportError::Invalid("repitch map size overflow"))?;
        // A projected recipe and the independent constructor's map coexist
        // during the copy. Borrowed recipes are charged by their caller.
        let projected_bytes = match &requirement {
            Cow::Borrowed(_) => 0,
            Cow::Owned(requirement) => requirement
                .retained_bytes()
                .ok_or(ExportError::Invalid("repitch map size overflow"))?,
        };
        if bytes
            .checked_add(projected_bytes)
            .is_none_or(|peak| peak > available)
        {
            return Err(budget_error);
        }
        let map = requirement.try_owned_map().map_err(ExportError::Render)?;
        check(control)?;
        let definition =
            SourceDefinition::new(file.path().to_path_buf(), raw, file.reader_limits())
                .with_repitch(map, requirement.law())
                .map_err(ExportError::Render)?;
        Ok((definition, bytes))
    }
}
