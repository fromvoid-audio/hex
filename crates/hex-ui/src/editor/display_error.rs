//! One error value owns its source metadata, so later messages cannot reuse a stale path.
use crate::theme;
use gpui::{Context, IntoElement, Render, SharedString, Window, div, prelude::*, rgb};
use hex_session::service::AudioBatchMemberFailure;
use std::{fmt, ops::Deref};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DisplayError {
    message: String,
    batch_member: Option<AudioBatchMemberFailure>,
}

impl From<String> for DisplayError {
    fn from(message: String) -> Self {
        Self {
            message,
            batch_member: None,
        }
    }
}
impl From<&str> for DisplayError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<AudioBatchMemberFailure> for DisplayError {
    fn from(failure: AudioBatchMemberFailure) -> Self {
        Self {
            message: format!(
                "audio batch refused at file {} · {}: {}",
                failure.member,
                hex_session::service::text::verbatim(failure.path.display()),
                failure.explanation
            ),
            batch_member: Some(failure),
        }
    }
}
impl Deref for DisplayError {
    type Target = str;
    fn deref(&self) -> &str {
        &self.message
    }
}
impl AsRef<str> for DisplayError {
    fn as_ref(&self) -> &str {
        &self.message
    }
}
impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl DisplayError {
    /// Preserve source metadata when diagnostics adds an app-owned explanation.
    pub(super) fn with_context(&self, context: &str) -> Self {
        match &self.batch_member {
            Some(failure) => {
                let mut failure = failure.clone();
                failure.explanation.push_str(" · ");
                failure.explanation.push_str(context);
                failure.into()
            }
            None => format!("{} · {context}", self.message).into(),
        }
    }

    /// Both status and tooltip consume this exact display text. Only the path is source data.
    pub(super) fn display_text(&self) -> SharedString {
        match &self.batch_member {
            Some(failure) => format!(
                "{}{}{}",
                theme::text(format!("audio batch refused at file {} · ", failure.member)),
                failure.path.display(),
                theme::text(format!(": {}", failure.explanation)),
            )
            .into(),
            None => theme::text(&self.message),
        }
    }
}

pub(super) struct ErrorTooltip(pub(super) DisplayError);
impl ErrorTooltip {
    pub(super) fn display_text(&self) -> SharedString {
        self.0.display_text()
    }
}
impl Render for ErrorTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(self.display_text())
    }
}

pub(super) struct RecoveryTooltip {
    pub(super) failure: std::sync::Arc<hex_session::service::RecoveryFailure>,
}
impl RecoveryTooltip {
    pub(super) fn display_text(&self) -> gpui::SharedString {
        use crate::theme;
        // Storage supplies lowercase app wording. Keep its diagnostic as source
        // data: it can include an earlier slot path or OS-owned error spelling.
        let mut detail = format!(
            "{}{}",
            theme::text("automatic recovery: "),
            self.failure.explanation
        );
        if let Some(preserved) = &self.failure.preserved_path {
            detail.push_str(&format!(
                "{}{}",
                theme::text(" · previous journal preserved at "),
                preserved.display()
            ));
        }
        detail.into()
    }
}
impl gpui::Render for RecoveryTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        use crate::theme;
        use gpui::{div, prelude::*, rgb};
        div()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(self.display_text())
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn batch_failure_status_and_tooltip_preserve_source_case_and_bytes() {
        let path = PathBuf::from("/Samples/DrÜms/Kick.WAV");
        let source = AudioBatchMemberFailure {
            member: 2,
            path: path.clone(),
            explanation: "WAV Decode Refused".into(),
        };
        let error = DisplayError::from(source);
        let expected =
            "audio batch refused at file 2 · /Samples/DrÜms/Kick.WAV: wav decode refused";
        assert_eq!(error.display_text().as_bytes(), expected.as_bytes());
        let tooltip = ErrorTooltip(error.clone());
        assert_eq!(tooltip.display_text().as_bytes(), expected.as_bytes());
        assert_eq!(
            error.batch_member.as_ref().unwrap().path.as_os_str(),
            path.as_os_str()
        );
        let contextual = error.with_context("Logging Unavailable");
        assert_eq!(
            contextual.display_text().as_ref(),
            format!("{expected} · logging unavailable")
        );
        let replacement = DisplayError::from("NEXT Failure".to_owned());
        assert_eq!(replacement.display_text().as_ref(), "next failure");
        assert!(replacement.batch_member.is_none());
    }
}
