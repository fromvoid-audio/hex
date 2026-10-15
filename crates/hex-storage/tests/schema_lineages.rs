use hex_project::{Command, Editor, Project};
use hex_storage::{decode, encode, encode_schema20};
use serde_json::{Value, json};

const MAIN19: &[u8] = include_bytes!("fixtures/lineages/main19.json");
const FOUNDATION: [&[u8]; 4] = [
    include_bytes!("fixtures/lineages/foundation19.json"),
    include_bytes!("fixtures/lineages/foundation20.json"),
    include_bytes!("fixtures/lineages/foundation21.json"),
    include_bytes!("fixtures/lineages/foundation22.json"),
];
fn value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn devices(project: &Project) -> Vec<(i32, i32)> {
    project
        .tracks()
        .iter()
        .flat_map(|t| t.devices().iter().chain(t.instrument()))
        .chain(project.return_tracks().iter().flat_map(|t| t.devices()))
        .map(|d| {
            (
                d.state().input_gain_millidb(),
                d.state().output_gain_millidb(),
            )
        })
        .collect()
}
#[test]
fn exact_published_lineages_migrate_edit_and_reopen_without_data_loss() {
    for original in std::iter::once(MAIN19).chain(FOUNDATION) {
        let project = decode(original).unwrap();
        let encoded = encode(&project).unwrap();
        let doc = value(&encoded);
        assert_eq!(doc["version"], 23);
        assert_eq!(decode(&encoded).unwrap(), project);
        assert_eq!(
            doc["tracks"][0]["devices"][0]["payload"],
            json!([0, 255, 42])
        );
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::MasterGain { gain: 0.25 }).unwrap();
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
        assert!(editor.undo());
        assert_eq!(*editor.project(), project);
        assert!(editor.redo());
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
    }
    let main = decode(MAIN19).unwrap();
    assert_eq!(
        devices(&main),
        [(3000, -9500), (-60000, 24000), (12000, -60000)]
    );
    assert!(main.import_provenance().is_none());
    for original in FOUNDATION {
        let project = decode(original).unwrap();
        assert_eq!(
            project.import_provenance().unwrap().original_bytes(),
            &[255, 0, 42]
        );
        assert!(devices(&project).iter().all(|&(input, _)| input == 0));
    }
    assert_eq!(
        devices(&decode(FOUNDATION[3]).unwrap()),
        [(0, -9500), (0, 24000), (0, -60000)]
    );
}
#[test]
fn main17_to19_defaults_are_preserved_and_hybrid_shapes_refuse() {
    for version in 17..=19 {
        let mut doc = value(MAIN19);
        doc["version"] = version.into();
        assert_eq!(
            devices(&decode(&bytes(&doc)).unwrap()),
            [(3000, -9500), (-60000, 24000), (12000, -60000)]
        );
        doc["tracks"][0]["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove("input_gain_millidb");
        doc["tracks"][0]["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove("output_gain_millidb");
        assert_eq!(devices(&decode(&bytes(&doc)).unwrap())[0], (0, 0));
    }
    for source in [MAIN19, FOUNDATION[0]] {
        for field in ["routing", "import_provenance"] {
            let mut hybrid = value(source);
            if hybrid.get(field).is_some() {
                hybrid.as_object_mut().unwrap().remove(field);
            } else {
                hybrid[field] = if field == "routing" {
                    json!([])
                } else {
                    Value::Null
                };
            }
            assert!(decode(&bytes(&hybrid)).is_err());
        }
    }
    // Legacy gain18 is a subset of the main gain lineage, never an ambiguity.
    let mut old = value(MAIN19);
    old["version"] = 18.into();
    old["tracks"][0]["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("input_gain_millidb");
    assert_eq!(devices(&decode(&bytes(&old)).unwrap())[0], (0, -9500));
}
#[test]
fn union23_requires_both_gains_and_refuses_invalid_duplicate_and_future_fields() {
    let base = value(&encode(&decode(MAIN19).unwrap()).unwrap());
    for field in ["input_gain_millidb", "output_gain_millidb"] {
        for bad in [
            json!(-60001),
            json!(24001),
            json!(0.5),
            Value::Null,
            json!("0"),
        ] {
            let mut doc = base.clone();
            doc["tracks"][0]["devices"][0][field] = bad;
            assert!(decode(&bytes(&doc)).is_err());
        }
        let mut doc = base.clone();
        doc["tracks"][0]["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode(&bytes(&doc)).is_err());
        let text = String::from_utf8(bytes(&base)).unwrap();
        let existing = format!("\"{field}\":{}", base["tracks"][0]["devices"][0][field]);
        let duplicate = text.replace(&existing, &format!("{existing},\"{field}\":0"));
        assert_ne!(text, duplicate);
        assert!(decode(duplicate.as_bytes()).is_err());
    }
    for field in ["routing", "import_provenance", "loop_region"] {
        let mut doc = base.clone();
        doc.as_object_mut().unwrap().remove(field);
        assert!(decode(&bytes(&doc)).is_err());
    }
    let mut doc = base.clone();
    doc["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_envelope");
    assert!(decode(&bytes(&doc)).is_err());
    let mut doc = base.clone();
    doc["tracks"][0]["devices"][0]["unknown"] = true.into();
    assert!(decode(&bytes(&doc)).is_err());
    let mut doc = base;
    doc["version"] = 24.into();
    assert!(decode(&bytes(&doc)).is_err());
    // Output-only22 must never silently accept input gains by widening its DTO.
    let mut doc = value(FOUNDATION[3]);
    doc["tracks"][0]["devices"][0]["input_gain_millidb"] = 0.into();
    assert!(decode(&bytes(&doc)).is_err());
}
#[test]
fn input_only_gain_and_legacy_encoder_refusals_preserve_project_and_destination() {
    let mut snapshot = decode(FOUNDATION[0]).unwrap().snapshot();
    snapshot.tracks[0].devices[0].state = snapshot.tracks[0].devices[0]
        .state
        .clone()
        .with_input_gain_millidb(3000)
        .unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    assert!(encode_schema20(&project).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-schema-lineage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("project.hex");
    hex_storage::save_new(&path, &project).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(hex_storage::save_new(&path, &Project::new(48000).unwrap()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(hex_storage::load(&path).unwrap(), project);
    std::fs::remove_dir_all(dir).unwrap();
}
