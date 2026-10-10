# compact original Operator notes

Pulse2171 separated immutable oscillator routing and four prepared envelopes
from pitch steps and velocity. Pulse2183 adds four waveform selectors and
bounded harmonic-cap metadata. The current `PreparedOperatorConfiguration`
occupies592 bytes and owns no per-context allocations. Each
`PreparedOperatorNote` is48 bytes, with four phase increments, one velocity
coefficient and four packed cap indices. The caller must sample a note against
its preparing configuration. Native context identity/ownership belongs to
Pulse2169, not this standalone DSP API.

Configuration preparation validates rate and uses already validated oscillator,
routing and envelope types. Each note validates pitch, velocity and every
oscillator's Nyquist limit, including inaudible operators. Rendering borrows the
configuration and compact note; it neither copies the graph per sample nor
allocates, initializes tables, replays envelopes or owns reference counts. This
is verified by source inspection here; native callback probes are separate.

The shared private oscillator loop preserves arithmetic ordering, static
envelope shaping and registered FM4's41-word/344-byte analytic ABI. Standalone
FM4 remains328 bytes per prepared note. All-sine Operator configurations retain
the prior sampling route and exact PCM. Non-sine shapes use a process-wide bank
whose table data is245,880 bytes; the fixed OnceLock cell occupies24 bytes on this64-bit target, including
the16-byte Box descriptor (245,904 bytes total before allocator bookkeeping). Each
non-sine context holds only a static reference. For100 notes sharing one
configuration, the shared configuration and compact-note storage is5392 bytes; this excludes
host note records, banks, references and graph/retirement storage.

All66 device tests pass. Exact-bit comparisons cover arbitrary/reverse acyclic
routes, three rates, pitch/velocity pairs, zero/early/long gates, release ends,
large ages and reverse query order. Independent envelope/FM oracles remain
passing; the control codec's four-operator PCM oracle also exercises the compact
path. This does not establish full Operator behavior or Live numerical parity.

## short release measurements

Apple M5 Pro, macOS26.6.2. Three interleaved old/new standalone FM4 runs, then
three standalone/compact Operator comparisons:48kHz,100 held voices, four-stage
chain,128 frames,100 warmups,2000 timed blocks, Instant wall time/black_box.
Another agent paused Cargo/CPU during timing; other system load was uncontrolled.
Baseline is PR69 b6f6e379b581c9b93f71268c33a79ebc44602dd0. Binary/source
hashes and all nine logs live under ignored
`artifacts/profiles/operator-compact-notes-20261010/`.

Legacy FM4 total medians were0.348s before and0.355s after (about2% higher in a
short noisy sample); per-note size and exact PCM stayed unchanged. Current
standalone/compact Operator medians were0.499s/0.501s. Every Operator comparison
checked all268800 rendered frames outside timing and found exact PCM equality.
These measurements establish a bounded memory reduction without a demonstrated
speed gain. They are not physical device deadlines or large-project acceptance.
Reproduce with `cargo run --release -p hex-devices --example profile_fm4` and
`cargo run --release -p hex-devices --example profile_operator_context`.

Pulse2172 adds the registered shared-context adapter and exercises it through
Arrangement, Session, callback and export paths; Pulse2183 adds registered
mixed-waveform rendering and generic choice/state migration. The current shared
configuration is592 bytes and compact notes are48 bytes. The standalone timing
measurements above predate the waveform work and must not be treated as its
performance evidence. Mutable feedback, FM sideband aliasing quality, remaining
Operator behavior and native listening remain open. See the
[waveform contract](operator-waveforms.md) for correctness evidence and the
reproducible profile command. Full local CI is recorded in the focused PR and
Pulse ticket.
