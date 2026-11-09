use super::*;
use hex_lattice_contract::{MAX_DEVICE_TYPE_ID_BYTES, NativeDeviceState};
use hex_project::{DeviceComparison, DeviceVariant};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-device-defaults-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn preset(class: DeviceClass, name: &'static str, value: u8) -> Preset {
    let state = NativeDeviceState::new(DeviceTypeId::from_static(name), 1, vec![value]).unwrap();
    Preset {
        class,
        state,
        comparison: None,
    }
}

#[test]
fn roundtrip_comparison_replacement_and_class_provider_isolation() {
    let root = Directory::new();
    let mut a = preset(DeviceClass::Instrument, "hex.test.one", 1);
    a.comparison = Some(DeviceComparison::new(
        DeviceVariant::B,
        NativeDeviceState::new(a.state.type_id(), 2, vec![9]).unwrap(),
    ));
    let b = preset(DeviceClass::Effect, "hex.test.one", 2);
    let c = preset(DeviceClass::Instrument, "hex.test.two", 3);
    assert!(load(&root.0, a.class, a.state.type_id()).unwrap().is_none());
    for value in [&a, &b, &c] {
        save(&root.0, value).unwrap();
    }
    a.state = NativeDeviceState::new(a.state.type_id(), 3, vec![7]).unwrap();
    save(&root.0, &a).unwrap();
    for value in [&a, &b, &c] {
        assert_eq!(
            load(&root.0, value.class, value.state.type_id())
                .unwrap()
                .as_ref(),
            Some(value)
        );
    }
}

#[test]
fn bounded_provider_paths_are_portable_and_invalid_ids_do_not_touch_disk() {
    let root = Directory::new();
    let name = format!("{}.b", "a".repeat(MAX_DEVICE_TYPE_ID_BYTES - 2));
    let id = DeviceTypeId::new(&name).unwrap();
    let target = path(&root.0, DeviceClass::Effect, id).unwrap();
    assert!(target.starts_with(&root.0));
    assert!(target.file_name().unwrap().len() < 255);
    let maximum = Preset {
        class: DeviceClass::Effect,
        state: NativeDeviceState::new(id, 1, vec![1]).unwrap(),
        comparison: None,
    };
    save(&root.0, &maximum).unwrap();
    assert_eq!(load(&root.0, maximum.class, id).unwrap(), Some(maximum));
    for bad in ["../escape", "con.", "hex/escape", "Upper.Case"] {
        assert!(path(&root.0, DeviceClass::Effect, DeviceTypeId::from_static(bad)).is_err());
    }
    assert!(!root.0.join("escape").exists());
}

#[test]
fn malformed_newer_and_mismatched_defaults_are_never_silently_replaced() {
    let root = Directory::new();
    let value = preset(DeviceClass::Effect, "hex.test.one", 1);
    save(&root.0, &value).unwrap();
    let target = path(&root.0, value.class, value.state.type_id()).unwrap();
    let bytes = encode(&value).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut variants = vec![b"{broken".to_vec()];
    for (key, replacement) in [
        ("schema", serde_json::json!(super::super::PRESET_SCHEMA + 1)),
        ("class", serde_json::json!("instrument")),
        ("type_id", serde_json::json!("hex.test.other")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut changed = original.clone();
        changed[key] = replacement;
        variants.push(serde_json::to_vec(&changed).unwrap());
    }
    for bad in variants {
        fs::write(&target, &bad).unwrap();
        assert!(load(&root.0, value.class, value.state.type_id()).is_err());
        assert!(save(&root.0, &value).is_err());
        assert_eq!(fs::read(&target).unwrap(), bad);
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
fn invalid_pair_and_interrupted_publication_keep_prior_bytes_and_clean_staging() {
    let root = Directory::new();
    let mut value = preset(DeviceClass::Effect, "hex.test.one", 1);
    save(&root.0, &value).unwrap();
    let target = path(&root.0, value.class, value.state.type_id()).unwrap();
    let old = fs::read(&target).unwrap();
    for state in [
        value.state.clone().with_input_gain_millidb(-300).unwrap(),
        value.state.clone().with_output_gain_millidb(600).unwrap(),
    ] {
        let unsupported = Preset {
            state,
            ..value.clone()
        };
        assert!(save(&root.0, &unsupported).is_err());
        assert_eq!(fs::read(&target).unwrap(), old);
    }
    value.comparison = Some(DeviceComparison::new(
        DeviceVariant::A,
        preset(DeviceClass::Effect, "hex.test.other", 2).state,
    ));
    assert!(save(&root.0, &value).is_err());
    assert_eq!(fs::read(&target).unwrap(), old);
    value.comparison = None;
    value.state = NativeDeviceState::new(value.state.type_id(), 2, vec![8]).unwrap();
    let failed = save_using(
        &root.0,
        &value,
        |staged, destination| {
            assert_eq!(fs::read(destination).unwrap(), old);
            assert_eq!(load_preset(staged).unwrap(), value);
            Err(io::Error::from(io::ErrorKind::Interrupted))
        },
        sync_directory,
    );
    assert!(matches!(failed, Err(Error::Io(error)) if error.kind() == io::ErrorKind::Interrupted));
    assert_eq!(fs::read(&target).unwrap(), old);
    assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn postpublication_sync_failure_reports_the_complete_new_default() {
    let root = Directory::new();
    let value = preset(DeviceClass::Effect, "hex.test.one", 4);
    let failed = save_using(
        &root.0,
        &value,
        |from, to| fs::rename(from, to),
        |_| Err(io::Error::from(io::ErrorKind::Other)),
    );
    let Err(Error::PublishedButNotDurable { path: target, .. }) = failed else {
        panic!("must distinguish published state");
    };
    assert_eq!(load_preset(&target).unwrap(), value);
    assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
}
