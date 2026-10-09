# hex-media

## Purpose

Bounded control-side import of supported RIFF/WAVE audio into validated engine
audio buffers, plus versioned sample-rate conversion. The decoder accepts PCM
16/24/32-bit and IEEE float32, mono or stereo, at sample rates from 8 kHz through
192 kHz. Mono is duplicated to stereo. Conversion uses a centered Hann-windowed
sinc filter and runs only on a control worker; it does not alter the audio
callback. Identity conversion shares the original immutable PCM allocation.

## Dependencies

`hex-engine` provides the validated `Audio` type. `hex-project` owns the
canonical converted-frame-count rule and conversion algorithm version IDs.
`sha2` provides the pinned SHA-256 implementation for source page verification.
File access and WAV parsing otherwise use the Rust standard library.

## Does not own

Project editing or asset IDs, persistence, warp, codecs beyond the listed WAV
encodings, plugin hosting, UI, background worker scheduling, or audio callbacks.
Import and resampling are control-side operations and must not run on the
callback.

Callers provide byte and decoded-frame limits. `decode_wav_file` preserves the
exact source `PathBuf`; it does not normalize case or rewrite the path.

`WavFrameReader` is a separate control-side API for bounded random-access reads
from classic RIFF/WAVE files. Its `WavReaderLimits` independently cap file bytes,
source frames, scanned chunk headers and decoded-window frames. Opening scans and
validates RIFF chunk topology without reading audio payloads; `read_frames_into`
uses fixed-size scratch storage and clears the complete caller buffer if a read
fails. `read_frames` is a fallibly allocated convenience for one bounded window.
The reader owns the opened file descriptor and keeps the caller's path spelling.
It compares descriptor length and modification time before and after each read,
which catches ordinary edits and truncation but is not an immutable content
identity: same-size edits with unchanged or restored timestamps may escape it.

`VerifiedWavReader` adds a control-side SHA-256 fingerprint over every raw byte
in the opened descriptor, including trailing bytes outside RIFF, and an ordered
`VERIFIED_WAV_PAGE_BYTES` raw-page digest manifest. `fingerprint()` exposes the
whole-file digest, while `page_digests()` and `page_manifest_bytes()` expose the
ordered digests and allocated payload size. `VerifiedWavReaderLimits` adds a
manifest-byte ceiling; its actual vector capacity is checked before the
full-file hash pass.
The reader does not retain the full source. On open and each public window read,
the shared WAV parser reads through a one-page adapter that verifies each full
raw page before serving bytes from that same scratch buffer. A read also checks
the requested sample pages against the captured digests, so same-size edits
with restored modification times are detected when those pages are accessed.
Opening hashes the complete file in one pass but is not an atomic filesystem
snapshot: concurrent writes during that pass can produce a hybrid manifest.
The fingerprint is not persisted and no managed copy, sidecar, project
generation, or cache identity is implemented yet. Keep verified reader
creation, reads, hashing and final destruction on control workers.

This is only a bounded source-decoding primitive. It does not change the resident
import path or its limits and does not stream media into Session, Arrangement,
export, or the real-time callback. Production page identity, conversion, cache
ownership and callback-safe delivery remain separate work.

`ResamplePlan` prepares the existing v1 global-phase sinc kernel from source and
target rates, total source frames, separate source/output ceilings and an output
window ceiling. Query `required_source_range` for the exact source span and halo
of a half-open output window, then pass that global-indexed source slice to
`render_window`. The renderer uses caller-owned output storage, preserves the
whole-source zero-padding boundary, and returns bit-identical results across
different partitions. `render_wav_window` reads that span through a
`WavFrameReader` into caller-owned bounded scratch and converts it on the
control side. Neither API allocates during rendering; neither belongs in the
audio callback. Callers that convert many sources at the same rate pair can
prepare one `ResampleKernel` and pass clones to `ResamplePlan::with_kernel`.
Plans then share its immutable table; there is no global cache, and each unique
kernel remains an explicit control-side lifetime decision. The kernel reports
its allocated weight payload through `table_payload_bytes()` (up to the existing
13 MiB cap; Arc metadata is excluded). Keep its final drop off the audio
callback. These APIs do not establish a page cache, background scheduler,
immutable source identity, or production streaming playback.

`SINC_CONVERSION_V1` uses a Nyquist-normalized cutoff `c = 0.94 * min(1,
target_rate / source_rate)`, equivalent to cycles-per-sample cutoff `c / 2`.
The kernel radius is `ceil(32 / ratio)` source frames. Out-of-range samples are
zero-padded without edge renormalization, so boundary transients can be lower
than steady-state gain. These choices are a deterministic initial algorithm,
not a claim of transparent or Ableton-compatible conversion quality. The
windowed-sinc rationale follows Julius O. Smith's [Windowed Sinc
Interpolation](https://www.dsprelated.com/freebooks/pasp/Windowed_Sinc_Interpolation.html);
the cutoff margin, Hann window, support radius and phase-table size above are
Hex's explicit versioned choices.

Run the bounded release microprofile with:

```sh
cargo run --release -p hex-media --example profile_resample
```

One local run on Apple M5 Pro arm64 used generated one-second stereo inputs,
two warmups, and eight measured conversions per pair. Conversion timing excludes
the deterministic output digest. Times are milliseconds; with eight samples,
nearest-rank p95 equals the observed maximum.

| conversion | input → output frames | median | p95 | max |
| --- | ---: | ---: | ---: | ---: |
| 44.1 → 48 khz | 44,100 → 48,000 | 4.945 | 5.666 | 5.666 |
| 48 → 44.1 khz | 48,000 → 44,100 | 3.993 | 4.287 | 4.287 |
| 48 → 16 khz | 48,000 → 16,000 | 4.173 | 4.264 | 4.264 |

These are single-machine microbenchmark observations, not comparative speed,
real-time deadline, or whole-application performance claims. CPU load and thermal
state can affect results; rerun the command for a current local measurement.
