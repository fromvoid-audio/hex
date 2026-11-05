//! Bounded controller observations for generic controls. Page indices address
//! enumeration only; raw SDK IDs identify parameters. A page grants no edit,
//! source custody or publication authority. Values describe a main-lane read,
//! not a sample-exact processing or final audio-delivery acknowledgement.
use super::*;
mod conversion;
pub use conversion::{ParameterConversion, ParameterConversionResult, ParameterConversionStatus};

pub const PARAMETER_PAGE_SIZE: usize = 4;
/// Inventory bound shared with the isolated native controller host. A single
/// page and the processing edit/feedback queues keep their own fixed limits.
pub const MAX_CONTROLLER_PARAMETERS: u16 = 576;
const TEXT_UNITS: usize = 128;
const PARAMETER_FLAGS: u32 = 0x0001_801f;

/// Valid, terminated SDK UTF-16, copied without SDK pointers or unbounded text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParameterText {
    units: [u16; TEXT_UNITS],
    len: u8,
}
impl Default for ParameterText {
    fn default() -> Self {
        Self {
            units: [0; TEXT_UNITS],
            len: 0,
        }
    }
}
impl ParameterText {
    /// Bounded user text for SDK parsing; preserve spelling/case and refuse
    /// truncation or embedded terminators before any request is admitted.
    pub fn from_text(text: &str) -> Result<Self> {
        let mut result = Self::default();
        for unit in text.encode_utf16() {
            if unit == 0 || usize::from(result.len) == TEXT_UNITS - 1 {
                return Err(Error::Protocol(
                    "controller text exceeds its bound or contains a terminator",
                ));
            }
            result.units[usize::from(result.len)] = unit;
            result.len += 1;
        }
        Ok(result)
    }
    pub fn as_utf16(&self) -> &[u16] {
        &self.units[..usize::from(self.len)]
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let mut result = Self::default();
        for unit in &mut result.units {
            *unit = u16::from_le_bytes(reader.array()?);
        }
        let end = result
            .units
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(Error::Protocol("unterminated controller text"))?;
        if result.units[end..].iter().any(|unit| *unit != 0)
            || char::decode_utf16(result.units[..end].iter().copied())
                .any(|character| character.is_err())
        {
            return Err(Error::Protocol("invalid controller text"));
        }
        result.len = end as u8; // The terminating zero bounds end to127.
        Ok(result)
    }
}

/// Infinite plain values (for example minus-infinity dB) remain distinct from
/// malformed numeric data and an unavailable conversion. UI sliders still use
/// finite normalized values; these variants never become numeric input bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ParameterPlainValue {
    Finite(f64),
    NegativeInfinity,
    PositiveInfinity,
    #[default]
    Unavailable,
}
impl ParameterPlainValue {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let kind = reader.byte()?;
        let bits = u64::from_le_bytes(reader.array()?);
        let value = f64::from_bits(bits);
        match kind {
            0 if value.is_finite() => Ok(Self::Finite(value)),
            1 if bits == 0 => Ok(Self::NegativeInfinity),
            2 if bits == 0 => Ok(Self::PositiveInfinity),
            3 if bits == 0 => Ok(Self::Unavailable),
            _ => Err(Error::Protocol("invalid controller plain value")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerParameter {
    pub id: u32,
    pub flags: u32,
    pub step_count: u32,
    pub unit_id: i32,
    pub normalized: f64,
    pub default_normalized: f64,
    pub plain: ParameterPlainValue,
    pub default_plain: ParameterPlainValue,
    pub title: ParameterText,
    pub short_title: ParameterText,
    pub units: ParameterText,
    pub display: Option<ParameterText>,
    pub default_display: Option<ParameterText>,
}
impl ControllerParameter {
    /// Display capability only, never authorization to edit a later source.
    /// Preserve unknown flags in observations, but do not expose a writer for
    /// semantics this host does not understand.
    pub fn is_editable(&self) -> bool {
        self.flags & (!PARAMETER_FLAGS | 2 | 16) == 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParameterPageQuery {
    pub first: u16,
    /// Zero starts enumeration; subsequent pages bind the accepted revision.
    pub revision: u64,
    /// A single read-only SDK conversion instead of enumeration. Raw IDs are
    /// stable parameter identity; first must be zero for this query kind.
    pub conversion: Option<ParameterConversion>,
}
impl ParameterPageQuery {
    pub fn validate(self) -> Result<()> {
        if self.first > MAX_CONTROLLER_PARAMETERS || (self.first != 0 && self.revision == 0) {
            return Err(Error::Protocol("invalid controller page query"));
        }
        if let Some(conversion) = self.conversion {
            if self.first != 0 {
                return Err(Error::Protocol("conversion cannot enumerate a page"));
            }
            conversion.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParameterPage {
    pub revision: u64,
    pub total: u16,
    pub first: u16,
    values: [ControllerParameter; PARAMETER_PAGE_SIZE],
    len: u8,
}
impl ParameterPage {
    pub fn as_slice(&self) -> &[ControllerParameter] {
        &self.values[..usize::from(self.len)]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParameterPageProgress {
    Pending,
    /// One bounded control-side allocation; never decoded on the callback.
    Page(Box<ParameterPage>),
    Conversion(Box<ParameterConversionResult>),
    /// Ordinary metadata refresh invalidates enumeration, not the processor.
    Changed {
        revision: u64,
        total: u16,
    },
}

pub fn parameter_page_request(
    config: &Configuration,
    sequence: u64,
    operation: u64,
    query: ParameterPageQuery,
    poll: bool,
) -> Result<[u8; 351]> {
    query.validate()?;
    let mut out = [0; 351];
    out[..72].copy_from_slice(&capture_request(config, sequence, operation)?);
    out[..4].copy_from_slice(if poll { b"HXVN" } else { b"HXVM" });
    out[72..74].copy_from_slice(&query.first.to_le_bytes());
    out[74..82].copy_from_slice(&query.revision.to_le_bytes());
    if let Some(conversion) = query.conversion {
        conversion.write(&mut out[82..]);
    }
    Ok(out)
}

pub fn parameter_page(
    body: &[u8],
    config: &Configuration,
    sequence: u64,
    operation: u64,
    query: ParameterPageQuery,
) -> Result<ParameterPageProgress> {
    config.validate()?;
    query.validate()?;
    let frames = sequence
        .checked_mul(u64::from(config.quantum))
        .ok_or(Error::Protocol("controller page position exhausted"))?;
    let mut reader = Reader::new(body);
    if operation == 0
        || reader.binding()? != config.binding
        || u64::from_le_bytes(reader.array()?) != sequence
        || u64::from_le_bytes(reader.array()?) != frames
        || u64::from_le_bytes(reader.array()?) != operation
    {
        return Err(Error::Protocol("controller page acknowledgement differs"));
    }
    let status = reader.byte()?;
    if status == 0 {
        reader.end()?;
        return Ok(ParameterPageProgress::Pending);
    }
    let revision = u64::from_le_bytes(reader.array()?);
    let total = u16::from_le_bytes(reader.array()?);
    if total > MAX_CONTROLLER_PARAMETERS || (total != 0 && revision == 0) {
        return Err(Error::Protocol("invalid controller page revision"));
    }
    if status == 2 {
        if (query.revision == 0 || query.revision == revision) && query.first <= total {
            return Err(Error::Protocol("controller page did not change"));
        }
        reader.end()?;
        return Ok(ParameterPageProgress::Changed { revision, total });
    }
    if (query.revision != 0 && query.revision != revision) || (status != 1 && status != 3) {
        return Err(Error::Protocol("controller page revision differs"));
    }
    if status == 3 {
        let conversion = query
            .conversion
            .ok_or(Error::Protocol("unexpected controller conversion"))?;
        let result = ParameterConversionResult::read(&mut reader, conversion, revision, total)?;
        reader.end()?;
        return Ok(ParameterPageProgress::Conversion(Box::new(result)));
    }
    if query.conversion.is_some() {
        return Err(Error::Protocol("conversion returned an enumeration"));
    }
    let first = u16::from_le_bytes(reader.array()?);
    let len = reader.byte()?;
    if first != query.first
        || first > total
        || usize::from(len) != PARAMETER_PAGE_SIZE.min(usize::from(total - first))
    {
        return Err(Error::Protocol("controller page extent differs"));
    }
    let mut page = ParameterPage {
        revision,
        total,
        first,
        len,
        ..ParameterPage::default()
    };
    for index in 0..usize::from(len) {
        let id = u32::from_le_bytes(reader.array()?);
        let flags = u32::from_le_bytes(reader.array()?);
        let step_count = u32::from_le_bytes(reader.array()?);
        let unit_id = i32::from_le_bytes(reader.array()?);
        let normalized = f64::from_le_bytes(reader.array()?);
        let default_normalized = f64::from_le_bytes(reader.array()?);
        if id > i32::MAX as u32
            || flags & 3 == 3
            || (flags & 16 != 0 && flags & 2 == 0)
            || step_count > i32::MAX as u32
            || ![normalized, default_normalized]
                .into_iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
            || page.values[..index].iter().any(|value| value.id == id)
        {
            return Err(Error::Protocol("invalid controller parameter"));
        }
        let plain = ParameterPlainValue::read(&mut reader)?;
        let default_plain = ParameterPlainValue::read(&mut reader)?;
        let title = ParameterText::read(&mut reader)?;
        let short_title = ParameterText::read(&mut reader)?;
        let units = ParameterText::read(&mut reader)?;
        let format_flags = reader.byte()?;
        let display = ParameterText::read(&mut reader)?;
        let default_display = ParameterText::read(&mut reader)?;
        if format_flags & !3 != 0
            || (format_flags & 1 == 0 && !display.as_utf16().is_empty())
            || (format_flags & 2 == 0 && !default_display.as_utf16().is_empty())
        {
            return Err(Error::Protocol("invalid controller display conversion"));
        }
        page.values[index] = ControllerParameter {
            id,
            flags,
            step_count,
            unit_id,
            normalized,
            default_normalized,
            plain,
            default_plain,
            title,
            short_title,
            units,
            display: (format_flags & 1 != 0).then_some(display),
            default_display: (format_flags & 2 != 0).then_some(default_display),
        };
    }
    reader.end()?;
    Ok(ParameterPageProgress::Page(Box::new(page)))
}
