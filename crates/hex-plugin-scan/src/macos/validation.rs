//! Original control-side validation causes. Native revalidation retains static
//! refusals or the original I/O error; it never formats a second heap diagnostic.
//! The enum lives inline in its caller. std-owned I/O backing remains an opaque
//! error lifetime, not guessed DATA bytes. Discovery/report boundaries explicitly
//! convert to their existing presentation strings; that grants no admission.

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Io {
        context: &'static str,
        source: std::io::Error,
    },
}
impl From<&'static str> for Error {
    fn from(value: &'static str) -> Self {
        Self::Invalid(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Invalid(_) => None,
        }
    }
}
// Factory discovery and initial inspection still own String report fields.
// Native source/helper revalidation propagates Error directly instead.
impl From<Error> for String {
    fn from(value: Error) -> Self {
        value.to_string()
    }
}
