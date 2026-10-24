//! Control-side Mac discovery admission and wire types; never loads a plugin.
//! Process/signal ownership belongs only to the standalone scanner executable.
pub mod build_identity;
pub mod identity;
pub mod instance_package;
pub mod macho;
pub mod package;
pub mod protocol;
pub mod report;
pub mod validation;
