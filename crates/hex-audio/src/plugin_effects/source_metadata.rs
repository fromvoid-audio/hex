//! Copied source description for another independent instance. This stays on
//! control with the unique worker owner, never in callback configuration.
use super::*;
use hex_plugin_host::protocol::Configuration as NativeConfiguration;

/// Original checked paths/class/binary digests, with no native or admission
/// authority. A subsequent preparation must inspect the actual files again.
/// Copies are fallible and bounded to the existing native configuration limits.
#[derive(Debug, PartialEq, Eq)]
pub struct EffectSourceMetadata {
    module: String,
    executable: String,
    cid: [u8; 16],
    executable_sha256: String,
    info_sha256: String,
}
impl EffectSourceMetadata {
    /// Two existing 4096-byte configuration paths and two lower-hex digests.
    /// Inline metadata and allocator overhead are separate from this DATA cap.
    pub const MAX_DATA_BYTES: usize = 2 * 4096 + 128;

    pub fn selection(&self) -> EffectSelection<'_> {
        EffectSelection {
            module: std::path::Path::new(&self.module),
            executable: &self.executable,
            cid: self.cid,
            executable_sha256: &self.executable_sha256,
            info_sha256: &self.info_sha256,
        }
    }
    pub(super) fn required_data_bytes(config: &NativeConfiguration) -> Option<usize> {
        config.validate().ok()?;
        config
            .bundle
            .len()
            .checked_add(config.executable.len())?
            .checked_add(128)
    }
    pub(super) fn from_configuration(config: &NativeConfiguration) -> Result<Self, Error> {
        let expected = Self::required_data_bytes(config)
            .ok_or(Error::Protocol("native source metadata bounds differ"))?;
        let source = Self {
            module: copy_text(&config.bundle)?,
            executable: copy_text(&config.executable)?,
            cid: config.identity.class.bytes(),
            executable_sha256: digest_text(config.identity.binary.executable_sha256)?,
            info_sha256: digest_text(config.identity.binary.info_sha256)?,
        };
        if source.data_bytes() != Some(expected) {
            return Err(Error::Protocol("native source metadata capacity differs"));
        }
        Ok(source)
    }
    pub(crate) fn try_copy_on_control(&self) -> Result<Self, Error> {
        Ok(Self {
            module: copy_text(&self.module)?,
            executable: copy_text(&self.executable)?,
            cid: self.cid,
            executable_sha256: copy_text(&self.executable_sha256)?,
            info_sha256: copy_text(&self.info_sha256)?,
        })
    }
    /// Actual retained String capacities, excluding this inline value and
    /// allocator overhead. Callers account their own temporary copied value.
    pub fn data_bytes(&self) -> Option<usize> {
        self.module
            .capacity()
            .checked_add(self.executable.capacity())?
            .checked_add(self.executable_sha256.capacity())?
            .checked_add(self.info_sha256.capacity())
    }
}
fn copy_text(text: &str) -> Result<String, Error> {
    if text.len() > 4096 {
        return Err(Error::Protocol("native source metadata text exceeds limit"));
    }
    let mut copy = String::new();
    copy.try_reserve_exact(text.len())
        .map_err(|_| Error::Protocol("native source metadata allocation refused"))?;
    if copy.capacity() != text.len() {
        return Err(Error::Protocol("native source metadata capacity differs"));
    }
    copy.push_str(text);
    Ok(copy)
}
fn digest_text(bytes: [u8; 32]) -> Result<String, Error> {
    let mut text = String::new();
    text.try_reserve_exact(64)
        .map_err(|_| Error::Protocol("native source digest allocation refused"))?;
    if text.capacity() != 64 {
        return Err(Error::Protocol("native source digest capacity differs"));
    }
    for byte in bytes {
        let hex = b"0123456789abcdef";
        text.push(char::from(hex[usize::from(byte >> 4)]));
        text.push(char::from(hex[usize::from(byte & 15)]));
    }
    Ok(text)
}
