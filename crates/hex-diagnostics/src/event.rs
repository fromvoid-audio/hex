//! Producer-stamped, bounded control records. Queue sequence is admission order,
//! not a claim that concurrent callers' original actions have a total order.
use crate::MAX_EVENT_BYTES;
use std::fmt::Write;

pub(super) struct Context {
    pub text: String,
    pub original_bytes: usize,
}
impl Context {
    pub fn new(value: &str) -> Self {
        Self {
            text: bounded(value).to_owned(),
            original_bytes: value.len(),
        }
    }
}

#[derive(Debug)]
pub(super) struct Record {
    pub level: crate::Level,
    pub category: crate::Category,
    pub sequence: u64,
    pub unix_nanos: u128,
    pub elapsed_nanos: u128,
    pub original_bytes: usize,
    pub message: String,
}
impl Record {
    pub fn line(&self) -> String {
        let mut line = format!(
            "event level={} category={} unix_nanos={} elapsed_nanos={} sequence={} truncated={} original_bytes={} message=",
            self.level.as_str(),
            self.category.as_str(),
            self.unix_nanos,
            self.elapsed_nanos,
            self.sequence,
            self.original_bytes > self.message.len(),
            self.original_bytes,
        );
        quoted(&mut line, &self.message);
        line.push('\n');
        line
    }
}

pub(super) fn bounded(value: &str) -> &str {
    let mut end = value.len().min(MAX_EVENT_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// JSON string escaping, including terminal controls and literal backslashes.
/// All input is already at most MAX_EVENT_BYTES; worst-case expansion is six.
pub(super) fn quoted(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value.is_control() => {
                // String's fmt::Write implementation cannot fail.
                let _ = write!(output, "\\u{:04x}", u32::from(value));
            }
            value => output.push(value),
        }
    }
    output.push('"');
}
