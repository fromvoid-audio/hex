//! Optimized control-side verified-window profile; no callback/device timing.
//! Pass a new output directory. Creates only deterministic, original WAV fixtures
//! there, refusing existing directories. Fixture/open/hash/oracle work is untimed.
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, hint::black_box, path::PathBuf, time::Instant};

const RATE: u32 = 48_000;
const SOURCE_FRAMES: usize = 96_000;
const WARMUP: usize = 8;
const MEASURED: usize = 40;

type Frame = [f32; 2];

fn sample(frame: usize, channel: usize, encoding: u16, bits: u16) -> (i32, f32) {
    match (encoding, bits) {
        (1, 16) => {
            let value = ((frame * 31 + channel * 101) % 65_536) as i32 - 32_768;
            (value, value as f32 / 32_768.0)
        }
        (1, 24) => {
            let value = ((frame * 3571 + channel * 17003) % 16_777_216) as i32 - 8_388_608;
            (value, value as f32 / 8_388_608.0)
        }
        _ => (
            0,
            (frame % 4096) as f32 / 4096.0 - 0.5 - channel as f32 * 0.125,
        ),
    }
}

fn fixture(encoding: u16, bits: u16) -> Result<(Vec<u8>, Vec<Frame>), Box<dyn Error>> {
    let align = 2 * (bits / 8);
    let mut data = Vec::new();
    let mut expected = Vec::new();
    for frame in 0..SOURCE_FRAMES {
        let mut pair = [0.0; 2];
        for (channel, destination) in pair.iter_mut().enumerate() {
            let (integer, value) = sample(frame, channel, encoding, bits);
            *destination = value;
            match (encoding, bits) {
                (1, 16) => data.extend_from_slice(&(integer as i16).to_le_bytes()),
                (1, 24) => data.extend_from_slice(&integer.to_le_bytes()[..3]),
                (3, 32) => data.extend_from_slice(&value.to_le_bytes()),
                (3, 64) => data.extend_from_slice(&f64::from(value).to_le_bytes()),
                _ => return Err("unsupported profile format".into()),
            }
        }
        expected.push(pair);
    }
    let size = u32::try_from(data.len())?;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(
        &size
            .checked_add(36)
            .ok_or("riff size overflow")?
            .to_le_bytes(),
    );
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0");
    wav.extend_from_slice(&encoding.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * u32::from(align)).to_le_bytes());
    wav.extend_from_slice(&align.to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    wav.extend_from_slice(&data);
    Ok((wav, expected))
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let [directory] = arguments.as_slice() else {
        return Err("usage: profile_verified_reads new-output-directory".into());
    };
    let directory = PathBuf::from(directory);
    fs::create_dir(&directory)?;
    for (encoding, bits) in [(1, 16), (1, 24), (3, 32), (3, 64)] {
        let (bytes, expected) = fixture(encoding, bits)?;
        let fingerprint: [u8; 32] = Sha256::digest(&bytes).into();
        let path = directory.join(format!("source-{encoding}-{bits}.wav"));
        fs::write(&path, &bytes)?;
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(bytes.len() as u64, SOURCE_FRAMES as u64, 128, 32_768),
            4096,
        );
        let mut reader = VerifiedWavReader::open(&path, limits)?;
        if reader.fingerprint() != &fingerprint {
            return Err("source fingerprint mismatch".into());
        }
        for count in [128, 4096, 32_768] {
            let mut output = vec![[42.0; 2]; count];
            let mut timings = Vec::new();
            let mut digest = 0xcbf2_9ce4_8422_2325_u64;
            let crossing = (hex_media::VERIFIED_WAV_PAGE_BYTES - 44) / (2 * usize::from(bits / 8));
            for iteration in 0..WARMUP + MEASURED {
                let start = match iteration % 3 {
                    0 => 0,
                    1 => crossing - 17,
                    _ => SOURCE_FRAMES - count / 2,
                };
                output.fill([42.0; 2]);
                let began = Instant::now();
                let decoded = reader.read_frames_into(start as u64, black_box(&mut output))?;
                let elapsed = began.elapsed().as_nanos();
                let available = count.min(SOURCE_FRAMES - start);
                if decoded != available {
                    return Err("decoded count differs from oracle".into());
                }
                for (offset, actual) in output.iter().enumerate() {
                    let oracle = if offset < available {
                        expected[start + offset]
                    } else {
                        [0.0; 2]
                    };
                    for channel in 0..2 {
                        if actual[channel].to_bits() != oracle[channel].to_bits() {
                            return Err(
                                "verified output differs from independent sample/zero-tail oracle"
                                    .into(),
                            );
                        }
                        digest = (digest ^ u64::from(actual[channel].to_bits()))
                            .wrapping_mul(0x100_0000_01b3);
                    }
                }
                if iteration >= WARMUP {
                    timings.push(elapsed);
                }
            }
            timings.sort_unstable();
            println!(
                "VERIFIED_CSV,{encoding},{bits},{count},{:.3},{:.3},{:.3},{digest:016x}",
                timings[MEASURED / 2] as f64 / 1000.0,
                timings[(MEASURED * 95 / 100) - 1] as f64 / 1000.0,
                timings[MEASURED - 1] as f64 / 1000.0
            );
        }
        let final_fingerprint: [u8; 32] = Sha256::digest(fs::read(&path)?).into();
        if fingerprint != final_fingerprint {
            return Err("profile source changed".into());
        }
        println!("VERIFIED_SOURCE,{encoding},{bits},{fingerprint:02x?}");
    }
    println!(
        "verified read profile: {RATE}hz stereo {SOURCE_FRAMES}frames, {WARMUP}warmup/{MEASURED}measured per format/window; start/64k-page crossing/eof; independent bitwise oracle; open/hash/fixture/oracle excluded; no device deadline claim"
    );
    Ok(())
}
