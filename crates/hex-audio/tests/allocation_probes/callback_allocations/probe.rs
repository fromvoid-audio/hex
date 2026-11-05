//! Shared callback allocation and exact PCM assertions.
use allocation_counter::{AllocationInfo, measure};
use hex_audio::prepared;
use hex_engine::Frame;

pub(crate) fn measured_block(
    callback: &mut prepared::Callback,
    output: &mut [Frame],
) -> AllocationInfo {
    let mut render_failed = false;
    let info = measure(|| {
        callback.begin_block();
        render_failed = callback.render(output).is_err();
    });
    assert!(!render_failed, "callback render returned an error");
    assert_eq!(info.count_total, 0, "callback allocated");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes");
    assert_eq!(
        info.count_current, 0,
        "callback changed live allocation count"
    );
    assert_eq!(
        info.bytes_current, 0,
        "callback changed live allocation bytes"
    );
    info
}

pub(crate) fn measured_shutdown(
    callback: &mut prepared::Callback,
    output: &mut [Frame],
) -> AllocationInfo {
    measured_block(callback, output)
}

pub(crate) fn compare_output(actual: &[Frame], expected: &[Frame]) {
    assert_eq!(actual, expected);
}
