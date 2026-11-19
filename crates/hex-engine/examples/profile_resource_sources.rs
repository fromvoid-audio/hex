//! Complete control-side original resource catalog hashing, not DSP/device timing.
//! Real supported input-bank custody and public scope identities are exercised.
use hex_engine::{
    AudioInputStorageAndObligations, AudioStorageReservation, Frame, OwnedAudioInputs,
    OwnedAudioSource,
    native_device_resources::{
        DeviceResourceCatalog, OriginalResourceSource, ResourceCatalogReservation,
    },
};
use hex_lattice::DeviceResources;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use hex_project::{Command, Created, Editor, Project};
use sha2::{Digest, Sha256};
use std::{error::Error, hint::black_box, sync::atomic::AtomicBool, time::Instant};

const RATE: u32 = 48_000;
const WARMUP: usize = 8;
const MEASURED: usize = 40;

struct Fixture {
    bank: AudioInputStorageAndObligations,
    expected: Vec<OriginalResourceSource>,
    hashes: Vec<[u8; 32]>,
    states: Vec<NativeDeviceState>,
}

fn reference(frames: &[Frame]) -> [u8; 32] {
    // Independent legacy v1 protocol: explicit per-sample little-endian updates.
    let mut hash = Sha256::new();
    hash.update(b"hex.device-resource.original-stereo.v1\0");
    hash.update(RATE.to_le_bytes());
    hash.update((frames.len() as u64).to_le_bytes());
    for frame in frames {
        for sample in frame {
            hash.update(sample.to_bits().to_le_bytes());
        }
    }
    hash.finalize().into()
}

fn fixture(source_count: usize, frames: usize) -> Result<Fixture, Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let mut sources = Vec::new();
    let mut expected = Vec::new();
    let mut hashes = Vec::new();
    let mut states = Vec::new();
    for source in 0..source_count {
        let Created::Asset(asset) = editor.execute(Command::AddAsset {
            path: format!("generated/resource-{source}.wav").into(),
            sample_rate: RATE,
            frames: frames as u64,
        })?
        else {
            return Err("expected profile asset".into());
        };
        let pcm: Vec<Frame> = (0..frames)
            .map(|i| {
                [
                    if i % 257 == 0 {
                        -0.0
                    } else {
                        ((i * 13 + source * 31) % 8192) as f32 / 8192.0 - 0.5
                    },
                    ((i * 19 + source * 101) % 4096) as f32 / 4096.0 - 0.5,
                ]
            })
            .collect();
        hashes.push(reference(&pcm));
        sources.push((
            asset,
            OwnedAudioSource::try_resident(RATE, pcm)
                .map_err(|e| format!("supported original constructor refused: {:?}", e.reason))?,
        ));
        expected.push(OriginalResourceSource {
            asset,
            sample_rate: RATE,
            frames: frames as u64,
        });
        states.push(
            NativeDeviceState::new(
                DeviceTypeId::from_static("test.resource-profile"),
                1,
                vec![],
            )?
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1)?,
                asset: DeviceAssetKey::new(asset.value())?,
            }])?,
        );
    }
    let reservation = AudioStorageReservation {
        allocation_items: 256,
        handle_obligations: 256,
        max_prepared_data_bytes: 64 * 1024 * 1024,
        max_construction_data_bytes: 128 * 1024 * 1024,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    // Generated benchmark sources are reclaimed on control on any refusal.
    let bank = OwnedAudioInputs::try_new(sources, reservation)
        .map_err(|e| format!("profile input admission refused: {:?}", e.reason))?
        .prepare_input_storage_on_control()
        .map_err(|e| format!("profile input storage refused: {:?}", e.reason))?;
    Ok(Fixture {
        bank,
        expected,
        hashes,
        states,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    for (sources, frames) in [(1, 1024), (1, 48_000), (16, 48_000)] {
        let fixture = fixture(sources, frames)?;
        let reservation = ResourceCatalogReservation {
            metadata_bytes: 4 * 1024 * 1024,
            hash_frames: (sources * frames) as u64,
        };
        let cancelled = AtomicBool::new(false);
        let mut timings = Vec::new();
        let mut digest = 0xcbf2_9ce4_8422_2325_u64;
        for iteration in 0..WARMUP + MEASURED {
            let began = Instant::now();
            let catalog = DeviceResourceCatalog::prepare_on_control(
                black_box(&fixture.bank),
                &fixture.expected,
                reservation,
                &cancelled,
            )
            .map_err(|e| format!("catalog preparation refused: {e:?}"))?;
            let elapsed = began.elapsed().as_nanos();
            for (state, expected) in fixture.states.iter().zip(&fixture.hashes) {
                let scope = catalog
                    .prepare_scope_on_control(state, 4096)
                    .map_err(|e| format!("profile scope refused: {e:?}"))?;
                let view = catalog
                    .resources(&fixture.bank, &scope)
                    .map_err(|e| format!("profile bank view refused: {e:?}"))?;
                let actual = view.get(0).ok_or("profile resource missing")?;
                if actual.sample_rate != RATE
                    || actual.pcm.frames() != frames
                    || &actual.identity.content != expected
                {
                    return Err(
                        "actual public resource identity differs from scalar protocol".into(),
                    );
                }
                for byte in actual.identity.content {
                    digest = (digest ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
                }
            }
            if iteration >= WARMUP {
                timings.push(elapsed);
            }
            // Scope verification and catalog reclamation stay outside timing.
        }
        timings.sort_unstable();
        println!(
            "RESOURCE_CSV,{sources},{frames},{:.3},{:.3},{:.3},{digest:016x}",
            timings[MEASURED / 2] as f64 / 1000.0,
            timings[(MEASURED * 95 / 100) - 1] as f64 / 1000.0,
            timings[MEASURED - 1] as f64 / 1000.0
        );
    }
    println!(
        "resource hash profile: supported-controls=completed; {RATE}hz stereo; {WARMUP}warmup/{MEASURED}measure; complete catalog construction; source/reference/scope/reclaim excluded; no callback deadline claim"
    );
    Ok(())
}
