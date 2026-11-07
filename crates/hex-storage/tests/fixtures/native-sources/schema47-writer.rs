//! Fixture emitted through the frozen schema47 native_sources codec.
//! Codec layout is unchanged from e284d85130dfad6aab54db258528956db8d197b3;
//! only an explicit loop-downgrade refusal was added. Model cycles are None.
//! Temporarily included as a unit harness beside the private codec; metadata
//! only, no referenced media is read or rewritten. Output uses create_new.

#[cfg(test)]
#[test]
fn emit_console_audio_schema47_fixture_once() {
    use hex_project::Editor;
    use std::io::Write;
    let e = Editor::new(decode(include_bytes!("../tests/fixtures/track-groups/schema43-flat-tracks-15957517.json")).unwrap(), 8).unwrap();
    // Existing fixture includes audio, MIDI, stop cells, routing, automation,
    // opaque devices and retained processing; no invented historical layout.
    let value = schema::native_sources::encode(e.project()).unwrap();
    assert_eq!(value["version"], 47);
    let bytes = encode_document(&value).unwrap();
    assert_eq!(decode(&bytes).unwrap(), *e.project());
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open("tests/fixtures/native-sources/schema47-console-one-shot.json").unwrap();
    f.write_all(&bytes).unwrap();
}
