# native streaming workload — pulse2545 / PR240

This create-new workload prepares native acceptance of the connected verified
streaming path in PR232–239, including the reader-window and retirement fixes
in PR271/272. Generation and the explicit headless smoke do not
establish native device deadlines, sustained playback, listening, or the broader
original24/177 acceptance. PR241 maps the subsequent edit/fault exercise in the
[native validation ledger](streaming-native-validation.md).

## Generate and identify

From the exact checked checkout, with the supported exclusive Cargo target:

```sh
cargo run --release -p hex-session --example native_streaming_fixture --locked -- \
  /Users/portalsurfer/dev/hex/artifacts/native-streaming-2545
```

The destination must not exist, even empty. Partial failures preserve their files;
choose a fresh destination after investigating. No existing project/media is
replaced. The manifest records generator Git source and dirty state; only clean
source generation qualifies as exact-source evidence. Keep it with a SHA256
sidecar covering `manifest.json` and both saved `.hex` files (for example Python's
standard-library `hashlib`); WAV SHA256 values are already in the manifest from
actual `VerifiedWavReader` verification. Do not relabel older generated files
when later source changes.

There are 100 separately written original mono PCM16 triangle WAVs, each 120s,
cycling 24k/44.1k/48kHz (34/33/33 sources). The files contain continuous independent
signals with different frequencies/phases. Every source remains within the
existing 8m-frame/64MiB source ceiling. Aggregate source PCM is 925,272,000bytes;
converted aggregate duration exceeds the ordinary8m-frame Resident allowance.
The writer uses 4096-frame blocks and a 16KiB buffered writer, never a whole-source
PCM allocation. Each file is verified and hashed after writing.

`100-44100.hex` and `100-48000.hex` share these relative source paths. Each has 100
Arrangement audio tracks/clips spanning 120s, no instruments/MIDI/devices/Returns,
and two 100-cell Session scenes sharing the same 100 assets. The first starts at 0;
the second starts at 30s and runs to the actual source end. Session/Arrangement
use checked converted durations, rather than interpreting source frames at the
project rate. Track gain 0.04, master 0.5 and source peak 0.25 give a conservative
worst-case summed output peak near 0.5. Start with a comfortable hardware level;
this arithmetic is not a listening result.

## Automated preparation check

Small example tests cover canonical roundtrip, independent hashes, relative paths,
shared Session assets, malformed dimensions and preservation of an existing
destination. One explicit large smoke opens the real generated project through
Session's VerifiedStreaming load, Desktop sink, actual source/domain preparation,
Root publication and callback. Only the native output-open completion is replaced
with the existing allocation-probe constructor fixture:

```sh
HEX_STREAMING_WORKLOAD_PROJECT=/Users/portalsurfer/dev/hex/artifacts/native-streaming-2545/100-44100.hex \
  cargo test -p hex-desktop --features allocation-probe --locked \
  generated_100_track_project -- --ignored --nocapture
```

The smoke requires 100 verified sources/200 cells, actual install/context ACK,
decoded nonzero output, at least 2s of committed clock progress and zero total/net
allocation bytes/counts in its measured callback entries. It is ignored in
ordinary CI to avoid creating or reading a gigabyte fixture on every change.
Verification and worker IO are outside the callback probe. The explicit 120s
control deadline accommodates full-file verification; it is not a physical
callback deadline or realtime performance threshold.

## Optimized native operator checklist

Use the supported stable optimized bundle with exact final full local-CI evidence,
included PR heads and this checklist; see [test-builds.md](test-builds.md).
Do not automate native UI interaction, force-quit, change hardware configuration,
or launch a second review app. Save and quit any running hex before replacement,
then relaunch the stable app to load it. Opening a Resident document by accident
must not be treated as a streaming result.

1. Record installed executable/source/build receipt, project/manifest hashes,
   machine/OS, output device, project rate and observed device rate. Use
   `100-44100.hex` first for a 44.1kHz device. Use file → “open as verified streaming…” to open it. Record loading/buffering/busy/refused/fault status honestly.
2. Once the current revision is actually applied, inspect the transport diagnostic
   tooltip. Record starting applied revision, callback maximumµs, miss counter and
   render-fault flag. Record actual negotiated callback size only if observed;
   a current HAL buffer setting alone does not establish Hex's callback size.
3. Play Arrangement for 60–90s with all 100 tracks. Confirm continuously advancing
   transport and listen for sustained output, gaps, clicks or stalls. Record
   elapsed duration, ending counters, miss delta and faults. Initial buffering is
   distinct from a later dropout; do not reset counters silently.
4. Stop under playback and confirm it responds. Repeat the baseline using the
   48kHz project if the actual selected output configuration supports it; record
   negotiation/refusal rather than claiming an untested rate.
5. Save the finite result record, including failures and unobserved fields. Later
   seek/loop/scene/edit/changed-media exercises belong to PR241's checklist.

Native page/cache occupancy is not exposed by current UI: record **unknown**.
Headless zero-allocation probes are not OS allocation, native stop/dispose, timing
or listening proof. Exact N2 pressure, worker-panic and CPAL stop-failure controls
without an operator UI remain automated/headless evidence only. To exercise
missing/changed media later, generate an independent create-new directory and
operate only on that fixture's media; relative paths make copies independent.
