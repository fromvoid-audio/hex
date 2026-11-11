use super::*;
use crate::{ResidentNativeReservation, Session};
use hex_engine::AudioStorageLedgerReservation;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use hex_project::{
    Command, Created, DeviceComparison, DeviceModulation, DeviceUnitClass, DeviceVariant,
    PortableDeviceNode,
};
use std::sync::atomic::AtomicU64;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hex-preset-original-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn session(rate: u32) -> Session {
    Session::new(rate, 8, 1024)
        .unwrap()
        .with_native_reservation(ResidentNativeReservation {
            storage: AudioStorageLedgerReservation {
                data_rows: 128,
                handle_rows: 16,
                root_rows: 8,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            },
            max_scan_frames: 1024,
        })
        .unwrap()
}

fn expected(rate: u32, frames: &[Frame]) -> PortableDeviceAsset {
    PortableDeviceAsset {
        source_rate: rate,
        source_frames: frames.len() as u64,
        facts: original_pcm_facts_on_control(rate, frames, 1024, &AtomicBool::new(false)).unwrap(),
    }
}

fn write_original(path: &Path, rate: u32, frames: &[Frame], cancel: &AtomicBool) {
    let mut remaining = frames;
    hex_export::write_float_wav_new(path, rate, frames.len() as u64, cancel, |block| {
        let (read, tail) = remaining.split_at(block.len());
        block.copy_from_slice(read);
        remaining = tail;
        Ok(())
    })
    .unwrap();
}

#[test]
fn retained_active_and_inactive_originals_import_in_another_project_at_original_rate() {
    let directory = TestDirectory::new();
    let cancel = AtomicBool::new(false);
    let authored = [
        (8000, [[0.0, 0.25], [0.3, -0.5], [0.8, 0.125]]),
        (11025, [[0.3, 0.15], [-0.2, 0.6], [-0.7, 0.5]]),
    ];
    let manifest = authored.map(|(rate, frames)| expected(rate, &frames));
    let mut source = session(16000);
    let source_ids = authored.map(|(rate, frames)| {
        let path = directory.0.join(format!("input-{rate}.wav"));
        write_original(&path, rate, &frames, &cancel);
        let decoded = hex_media::decode_wav_file(&path, Limits::new(1024, 1024)).unwrap();
        let (Created::Asset(id), _) = source.import_device_asset(decoded, &cancel, None).unwrap()
        else {
            panic!("native source import");
        };
        // Retention uses the verified original resident PCM, even after the
        // original external filename no longer resolves.
        fs::remove_file(path).unwrap();
        id
    });
    let state = |key| {
        NativeDeviceState::new(
            DeviceTypeId::from_static("test.preset.original"),
            1,
            vec![1],
        )
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(key).unwrap(),
        }])
        .unwrap()
    };
    let unit = PortableDeviceUnit::new(
        DeviceUnitClass::Instrument,
        vec![PortableDeviceNode {
            parent: None,
            state: state(1),
            comparison: Some(DeviceComparison::new(DeviceVariant::A, state(2))),
            bypassed: false,
            modulation: DeviceModulation::default(),
        }],
        manifest.to_vec(),
    )
    .unwrap();
    let store = OriginalStore::new(&directory.0, 1024).unwrap();
    let before = source.project().clone();
    store.retain_unit(&source, &unit, &cancel).unwrap();
    store.retain_unit(&source, &unit, &cancel).unwrap();
    assert_eq!(source.project(), &before);
    assert_eq!(fs::read_dir(&store.directory).unwrap().count(), 2);

    let mut destination = session(22050);
    destination
        .editor
        .execute(Command::AddMidiTrack {
            name: "different identity space".into(),
        })
        .unwrap();
    let reopened = OriginalStore::new(&directory.0, 1024).unwrap();
    for ((entry, (rate, frames)), source_id) in manifest.into_iter().zip(authored).zip(source_ids) {
        let decoded = reopened.load(entry, &cancel).unwrap();
        assert_eq!(decoded.audio().frames(), frames);
        assert_eq!(decoded.sample_rate(), rate);
        let (Created::Asset(id), _) = destination
            .import_device_asset(decoded, &cancel, None)
            .unwrap()
        else {
            panic!("destination original import");
        };
        assert_ne!(id, source_id);
        let imported = destination.project().asset(id).unwrap();
        assert_eq!(imported.source_sample_rate(), rate);
        assert_eq!(
            destination
                .native_assets
                .original_source_facts_on_control(id)
                .unwrap(),
            entry.facts
        );
    }
}

#[test]
fn missing_or_corrupted_originals_refuse_without_replacing_the_retained_file() {
    let directory = TestDirectory::new();
    let cancel = AtomicBool::new(false);
    let store = OriginalStore::new(&directory.0, 8).unwrap();
    let frames = [[0.125, -0.5], [0.0, 0.5]];
    let entry = expected(48000, &frames);
    assert!(
        matches!(store.load(entry, &cancel), Err(Error::File(error)) if error.kind() == io::ErrorKind::NotFound)
    );
    assert!(store.retain(entry, 44100, &frames, &cancel).is_err());
    assert!(!store.directory.exists());

    let path = store.retain(entry, 48000, &frames, &cancel).unwrap();
    let mut corrupted = fs::read(&path).unwrap();
    *corrupted.last_mut().unwrap() ^= 1;
    fs::write(&path, &corrupted).unwrap();
    assert!(matches!(
        store.load(entry, &cancel),
        Err(Error::Invalid(
            "preset original audio content does not match"
        ))
    ));
    assert!(store.retain(entry, 48000, &frames, &cancel).is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupted);
    cancel.store(true, Ordering::Release);
    assert!(matches!(store.load(entry, &cancel), Err(Error::Cancelled)));
}
