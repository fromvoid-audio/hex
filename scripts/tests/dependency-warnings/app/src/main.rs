hex_warning_macros::passthrough! { fn answer() -> u32 { 42 } }
fn main() {
    assert_eq!(answer(), 42);
}
