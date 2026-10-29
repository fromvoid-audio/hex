//! Read-only SDK conversions share the same single controller operation/page
//! mailbox. No synthetic parameter edit or guessed unit mapping is involved.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
// One fixed SDK string stays inline: mailbox/request copies must not add heap
// owners or allocations. Its bounded size is included in constructor backing.
#[allow(clippy::large_enum_variant)]
pub enum ParameterConversion {
    Format { id: u32, normalized: f64 },
    Parse { id: u32, text: ParameterText },
    FromPlain { id: u32, plain: ParameterPlainValue },
}
impl ParameterConversion {
    pub fn id(self) -> u32 {
        match self {
            Self::Format { id, .. } | Self::Parse { id, .. } | Self::FromPlain { id, .. } => id,
        }
    }
    pub(super) fn validate(self) -> Result<()> {
        if self.id() > i32::MAX as u32 {
            return Err(Error::Protocol("controller conversion id exceeds profile"));
        }
        let valid = match self {
            Self::Format { normalized, .. } => {
                normalized.is_finite() && (0.0..=1.0).contains(&normalized)
            }
            Self::FromPlain {
                plain: ParameterPlainValue::Finite(value),
                ..
            } => value.is_finite(),
            Self::FromPlain {
                plain: ParameterPlainValue::Unavailable,
                ..
            } => false,
            _ => true,
        };
        if !valid {
            return Err(Error::Protocol("invalid controller conversion input"));
        }
        Ok(())
    }
    pub(super) fn write(self, out: &mut [u8]) {
        let (kind, value, text) = match self {
            Self::Format { normalized, .. } => (1, normalized, ParameterText::default()),
            Self::Parse { text, .. } => (2, 0.0, text),
            Self::FromPlain {
                plain: ParameterPlainValue::Finite(value),
                ..
            } => (3, value, ParameterText::default()),
            Self::FromPlain {
                plain: ParameterPlainValue::NegativeInfinity,
                ..
            } => (4, 0.0, ParameterText::default()),
            Self::FromPlain { .. } => (5, 0.0, ParameterText::default()), // validate excludes Unavailable.
        };
        out[0] = kind;
        out[1..5].copy_from_slice(&self.id().to_le_bytes());
        out[5..13].copy_from_slice(&value.to_bits().to_le_bytes());
        for (unit, bytes) in text.units.iter().zip(out[13..].chunks_exact_mut(2)) {
            bytes.copy_from_slice(&unit.to_le_bytes());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterConversionStatus {
    Converted,
    Refused,
    Unsupported,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterConversionResult {
    pub revision: u64,
    pub total: u16,
    pub id: u32,
    pub status: ParameterConversionStatus,
    pub normalized: Option<f64>,
    pub plain: ParameterPlainValue,
    pub display: Option<ParameterText>,
}
impl ParameterConversionResult {
    pub(super) fn read(
        reader: &mut Reader<'_>,
        query: ParameterConversion,
        revision: u64,
        total: u16,
    ) -> Result<Self> {
        let id = u32::from_le_bytes(reader.array()?);
        let status = match reader.byte()? {
            0 => ParameterConversionStatus::Converted,
            1 => ParameterConversionStatus::Refused,
            2 => ParameterConversionStatus::Unsupported,
            3 => ParameterConversionStatus::Missing,
            _ => return Err(Error::Protocol("invalid controller conversion status")),
        };
        let bits = u64::from_le_bytes(reader.array()?);
        let normalized = f64::from_bits(bits);
        let plain = ParameterPlainValue::read(reader)?;
        let formatted = reader.byte()?;
        let display = ParameterText::read(reader)?;
        if id != query.id() || formatted > 1 || (formatted == 0 && !display.as_utf16().is_empty()) {
            return Err(Error::Protocol(
                "controller conversion acknowledgement differs",
            ));
        }
        let accepted = status == ParameterConversionStatus::Converted;
        if accepted {
            if total == 0 || !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
                return Err(Error::Protocol("invalid converted controller value"));
            }
            if let ParameterConversion::Format {
                normalized: requested,
                ..
            } = query
                && normalized.to_bits() != requested.to_bits()
            {
                return Err(Error::Protocol("formatted normalized value differs"));
            }
        } else if bits != 0 || plain != ParameterPlainValue::Unavailable || formatted != 0 {
            return Err(Error::Protocol("refused conversion returned a value"));
        }
        Ok(Self {
            revision,
            total,
            id,
            status,
            normalized: accepted.then_some(normalized),
            plain,
            display: (formatted == 1).then_some(display),
        })
    }
}
