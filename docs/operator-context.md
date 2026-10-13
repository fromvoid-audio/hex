# compact original Operator notes

## shell-to-detail selection contract

The four shell rows are ordered d, c, b, a to match the compact device layout.
Pressing a row's level, ratio/frequency, or fixed-mode multiplier control selects
that oscillator and returns the detail pane to its oscillator page. The existing
shared control remains mounted under its stable `DeviceId` and `ParameterId`, so
the same pointer press can continue as a drag and create one canonical gesture.
When a focused shell numeric control begins a keyboard adjustment, it selects the
same oscillator before the parameter gesture is dispatched. These transitions
finish other valid numeric drafts, preserve an invalid draft rather than hiding
it, and exclude only the initiating shell control from finalization. They do not
change device state or DSP configuration. Other devices keep their independent
controls and context.

The rendered GPUI regression covers all four row mappings, selector dismissal,
invalid-draft refusal, pointer drag/undo, focused keyboard adjustment, stationary
typed editing and neighboring Utility state. This is source-level UI evidence;
native visual acceptance remains pending.

## Operator Tab navigation

The Operator body forms one Tab group. Tab walks the currently rendered controls
in layout order, and Shift-Tab reverses that order and wraps within the group.
The shell visits d, c, b, a: each oscillator selector followed by its available
controls, while the squares stay visually at the far right. Focusing a square
selects its oscillator context; its thicker focus border remains distinct from
selection. The detail pane contributes its visible envelope, lfo or
oscillator controls and page selectors; the preview contributes its available
controls and routing/global icon. Section containers are not Tab targets: focusing
any child widget reveals its containing row or detail pane's gutter background,
without changing its enable flag. Leaving a group restores its normal background,
even when its oscillator or detail page remains selected. All lfo summary/detail
controls and the routing preset participate; each shared lfo widget is mounted
once, with common controls in the summary and remaining controls in detail. The routing icon remains keyboard-operable with Enter/Space. Hidden or
disabled controls are excluded.
Changing detail pages changes that page's available targets, without changing
sound parameters. Enter/Space activates a focused oscillator or page selector;
routing retains its existing keyboard activation. Numeric controls keep their
typing and adjustment bindings. Graph nodes cycle with Ctrl-Tab/Ctrl-Shift-Tab
so plain Tab can leave the graph.

Traversal finishes a valid canonical typed draft through the canonical edit path
and finishes view-only preview drafts through their local edit path. Invalid
fine/filter/pitch/global previews refuse forward and reverse traversal before
blur, just as invalid canonical drafts do. Focus alone does not author state. Stable
per-device focus handles retire on project or track-owner changes; stale section
activation is rejected. Shared controls outside Operator retain their existing
Tab behavior. Native focus visibility, exact optimized macOS build delivery and
user confirmation remain separate acceptance requirements.

Pulse2171 separated immutable oscillator routing and four prepared envelopes
from pitch steps and velocity. Pulse2183 adds four waveform selectors and
bounded harmonic-cap metadata. The current `PreparedOperatorConfiguration`
occupies744 bytes and owns no per-context allocations. The standalone feedback
retarget kernel adds a checked configuration identity and prepared oscillator
levels for a shared control-side ramp; see the
[feedback contract](operator-feedback.md). Each
`PreparedOperatorNote` is64 bytes, with four phase increments, one velocity
coefficient, four packed cap indices and four prepared amplitude scales. The
caller must sample a note against its preparing configuration. The engine's
prepared note bank stores a revision context reference alongside notes created
by that context; shared-context identity and lease ownership keep that pair
intact through playback and retirement. The sampling adapter therefore validates
bounded payload fields and waveform compatibility but does not recompute the
control-prepared velocity scale for each sample. Native context identity and
ownership belongs to Pulse2169, not this standalone DSP API.

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
configuration, the shared configuration and compact-note storage is7,144 bytes; this excludes
host note records, banks, references and graph/retirement storage.

The current79 device tests pass. Exact-bit comparisons cover arbitrary/reverse acyclic
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
configuration was688 bytes and compact notes were48 bytes before sensitivity. The standalone timing
measurements above predate the waveform work and must not be treated as its
performance evidence. Mutable feedback, FM sideband aliasing quality, remaining
Operator behavior and native listening remain open. See the
[waveform contract](operator-waveforms.md) for correctness evidence and the
reproducible profile command. Full local CI is recorded in the focused PR and
Pulse ticket.

Pulse2205 adds four control-prepared tuning modes to the shared configuration.
Pulse2222 adds four per-oscillator amplitude velocity amounts. Scale
`1 - amount + amount * normalized_velocity` is prepared for each note on the
control side, then multiplies that oscillator's envelope amplitude before it
participates in modulation or carrier routing. This original bounded scale
changes modulator timbre while the existing global velocity multiplier remains
in place. A valid zero scale is retained when amount and velocity produce zero;
the active marker distinguishes it from legacy all-zero reserved words. Zero
amounts skip the new amplitude multiplication, preserving the legacy sampling
route and exact PCM. The generic shared note remains8 u64 words
(64 bytes); its last two Operator-private words now carry four packed f32 scales.
The expanded standalone compact note is64 bytes and its shared configuration
is704 bytes. A three-run release comparison of the existing neutral 100-voice
profile used the same workload on both revisions: 48 kHz, four-stage routing,
128-frame blocks, 100 warmups and 2,000 timed blocks. Before Pulse2222 (base
`93da08a`) the median total was0.697 s standalone /0.678 s compact; after it,
three final runs measured0.673/0.770/0.691 s standalone and
0.785/0.702/0.699 s compact (medians0.691 s /0.702 s). These short runs happened on a busy, noisy host;
they do not establish a reliable speed change or deadline. The full/compact
PCM stayed bit-identical. The measured sizes changed from600/688/48 bytes
(standalone note/context/compact note) to624/704/64 bytes. The after binary
SHA-256 was`2cfe557c75af7be885e0959221f0eef3950b97c7a06fc0961258de569154f098`;
raw logs are in ignored `artifacts/profiles/operator-velocity-after-optimized/`
and the before logs in the baseline worktree's matching profile folder. The
benchmark follows the neutral direct standalone/compact route and does not time
registered shared-payload decoding or its context adapter. Active sensitivity
is covered by the independent full-output oracle and a registered callback
allocation probe, which verifies zero total and net allocation/deallocation but
does not establish timing. No physical callback deadline or Live numerical
parity is claimed.

## focus presentation follow-up — 13 october 2026

Pulse2373 / PR178 follow-up: each oscillator row traverses coarse/frequency,
fine, fixed, level, then its A/B/C/D badge. The badge keeps its absolute position
at the right edge; child order provides both forward and reverse keyboard order.
The center curve/detail display always uses its dark background, including pointer
and keyboard focus. Shell and right-side sections retain their focus highlights.

GPUI emits section focus events after painting. Immediate notification in this
phase updates the stored flag without scheduling another frame. The section
listener now defers the editor notification until the effect cycle completes,
so highlights and context paint without waiting for mouse movement. Rendered
regressions inspect completed scene backgrounds along LFO/filter/pitch/global
traversal and oscillator rows, plus keyboard/pointer focus in the detail display.
They preserve canonical values and existing invalid-draft/owner tests.

Current main still has selection-only oscillator badges. Canonical on/off DSP,
state migration and history remain owned by Pulse2335 / unmerged PR164; focus
changes do not substitute a local enable flag for that implementation. Native
optimized-build review and stable macOS delivery remain separate from Linux tests.
