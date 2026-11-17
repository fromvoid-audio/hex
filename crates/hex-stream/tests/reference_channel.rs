use hex_engine::{Audio, StreamSourceKey, StreamedSourceMetadata};
use hex_media::{
    ResampleKernel, ResampleLimits, ResamplePlan, ResampleVersion, VerifiedWavReader,
    VerifiedWavReaderLimits, WavReaderLimits, resample_with_version,
};
use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Project, SINC_CONVERSION_V2};
use hex_stream::{StreamFailureKind, scan_reference_channel_on_control};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Wav(PathBuf);
impl Wav {
    fn create(frames: u32, sparse: bool) -> (Self, Vec<[f32; 2]>) {
        let path = std::env::temp_dir().join(format!(
            "hex-reference-{}-{}.wav",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bytes = frames * 4;
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&192_000u32.to_le_bytes()).unwrap();
        file.write_all(&4u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        let mut pcm = Vec::new();
        if sparse {
            file.set_len(44 + u64::from(bytes)).unwrap();
            let count = u64::from(frames).min(4096);
            let stride = u64::from(frames) / count;
            for index in 0..count {
                file.seek(SeekFrom::Start(44 + index * stride * 4)).unwrap();
                file.write_all(&1000i16.to_le_bytes()).unwrap();
                file.write_all(&12_000i16.to_le_bytes()).unwrap();
                pcm.push([1000.0 / 32768.0, 12_000.0 / 32768.0]);
            }
        } else {
            for index in 0..frames {
                let left = ((index % 97) as i16 - 48) * 250;
                let right = ((index % 71) as i16 - 35) * 700;
                file.write_all(&left.to_le_bytes()).unwrap();
                file.write_all(&right.to_le_bytes()).unwrap();
                pcm.push([f32::from(left) / 32768.0, f32::from(right) / 32768.0]);
            }
        }
        drop(file);
        (Self(path), pcm)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn asset(frames: u32) -> hex_project::AssetId {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Asset(id) = editor
        .execute(Command::AddAsset {
            path: "reference.wav".into(),
            sample_rate: 48_000,
            frames: u64::from(frames),
        })
        .unwrap()
    else {
        panic!("asset")
    };
    id
}
fn setup(
    wav: &Wav,
    frames: u32,
    target_rate: u32,
    version: ResampleVersion,
) -> (VerifiedWavReader, ResamplePlan, StreamedSourceMetadata) {
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(u64::from(frames) * 4 + 44, u64::from(frames), 128, 8192),
        4 * 1024 * 1024,
    );
    let reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
    let kernel = ResampleKernel::with_version(48_000, target_rate, version).unwrap();
    let plan = ResamplePlan::with_kernel(
        kernel,
        u64::from(frames),
        u64::from(frames),
        u64::from(frames) * 2,
        4096,
    )
    .unwrap();
    let conversion = if target_rate == 48_000 {
        IDENTITY_CONVERSION
    } else {
        SINC_CONVERSION_V2
    };
    let meta = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset(frames)),
        48_000,
        target_rate,
        u64::from(frames),
        plan.output_frames(),
        conversion,
        *reader.fingerprint(),
    )
    .unwrap();
    (reader, plan, meta)
}
fn expected_channel(frames: &[[f32; 2]]) -> usize {
    let count = frames.len().min(4096);
    let stride = (frames.len() / count).max(1);
    let mut energy = [0.0f64; 2];
    for index in 0..count {
        let frame = frames[index * stride];
        for channel in 0..2 {
            let x = f64::from(frame[channel]);
            energy[channel] += x * x;
        }
    }
    usize::from(energy[1] > energy[0])
}

#[test]
fn converted_reference_matches_resident_and_refuses_wrong_inputs() {
    let (wav, pcm) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 44_100, ResampleVersion::V2);
    let converted = resample_with_version(
        &Audio::new(48_000, pcm).unwrap(),
        44_100,
        ResampleLimits {
            max_source_frames: 8192,
            max_output_frames: 16384,
        },
        ResampleVersion::V2,
    )
    .unwrap();
    let mut source = vec![[0.0; 2]; 8192];
    let mut output = vec![[0.0; 2]; 4096];
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    assert_eq!(result.metadata(), &meta);
    assert_eq!(result.channel(), expected_channel(converted.frames()));
    let reads_before_refusals = reader.raw_page_reads();
    let derived = meta
        .with_plain_pitch(100, hex_project::WarpResampler::V2)
        .unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &derived,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::UnknownSource)
    );
    let wrong = StreamedSourceMetadata::new(
        meta.key(),
        48_000,
        44_100,
        8192,
        plan.output_frames(),
        SINC_CONVERSION_V2,
        [0; 32],
    )
    .unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &wrong,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::SourceChanged)
    );
    let wrong_kernel = ResampleKernel::with_version(48_000, 44_100, ResampleVersion::V1).unwrap();
    let wrong_plan = ResamplePlan::with_kernel(wrong_kernel, 8192, 8192, 16384, 4096).unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &wrong_plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::UnknownSource)
    );
    assert_eq!(
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut [], &mut output, || {
            false
        }),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut source, &mut [], || {
            false
        }),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(reader.raw_page_reads(), reads_before_refusals);
    let mut calls = 0;
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || {
                calls += 1;
                calls >= 3
            }
        ),
        Err(StreamFailureKind::Cancelled)
    );
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        )
        .unwrap(),
        result
    );
    let reads_before_small_scratch = reader.raw_page_reads();
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source[..1],
            &mut output,
            || false
        ),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(reader.raw_page_reads(), reads_before_small_scratch);
    let file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.set_len(44 + 8192 * 4 - 1).unwrap();
    drop(file);
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::SourceChanged)
    );
}

#[test]
fn dense_scan_uses_multiple_small_conversion_windows_in_order() {
    let (wav, _) = Wav::create(17, false);
    let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    let mut pcm = Vec::new();
    for index in 0..17 {
        let (left, right) = if index % 2 == 0 {
            (20_000i16, 0i16)
        } else {
            (0, 20_000)
        };
        file.write_all(&left.to_le_bytes()).unwrap();
        file.write_all(&right.to_le_bytes()).unwrap();
        pcm.push([f32::from(left) / 32768.0, f32::from(right) / 32768.0]);
    }
    drop(file);
    let (mut reader, plan, meta) = setup(&wav, 17, 48_000, ResampleVersion::V1);
    let mut source = [[0.0; 2]; 4];
    let mut output = [[0.0; 2]; 4];
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    assert_eq!(result.channel(), expected_channel(&pcm));
    assert_eq!(result.channel(), 0);
}

#[test]
fn sparse_twenty_million_frame_source_uses_only_bounded_samples() {
    let frames = 20_000_000;
    let (wav, samples) = Wav::create(frames, true);
    let (mut reader, plan, meta) = setup(&wav, frames, 48_000, ResampleVersion::V1);
    let mut source = [[0.0; 2]; 16];
    let mut output = [[0.0; 2]; 16];
    let reads_before = reader.raw_page_reads();
    let started = std::time::Instant::now();
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    eprintln!(
        "reference_scan frames={frames} rate=48000 source_scratch=16 output_scratch=16 samples=4096 elapsed_ms={} verified_page_reads={}",
        started.elapsed().as_millis(),
        reader.raw_page_reads() - reads_before
    );
    assert_eq!(result.channel(), expected_channel(&samples));
    assert_eq!(result.channel(), 1);
    let mut still_owned = [[0.0; 2]; 1];
    assert_eq!(reader.read_frames_into(0, &mut still_owned).unwrap(), 1);
    assert_eq!(still_owned[0], samples[0]);
}
