# project tuning and native freqs

Requested 17 October 2026. Pulse goal 4 owns the complete outcome. The reference
is the user's **Freqs** VST, `/Users/portalsurfer/dev/freqs`, repository
`PORTALSURFER/freqs`; its editor is GPUI. This work adapts original user-owned
scale/analysis code and behavior, without embedding the VST or its host runtime.

## reference identity

Committed reference: `f90669b0b193a55d049fc1cd7d30d7127233edca` (v0.3.4).
The inspected working tree also contains unpublished editor, preset-library and
shared-state changes. They are part of this inventory, not changes made here.
The following SHA-256 identities bind the inspected contents; the ignored local
`artifacts/tuning-reference/` retains the manifest and unpublished patch. Freqs
source and personal presets remain untouched. No third-party license grant is
inferred from its private repository.

| reference file | inspected sha-256 |
| --- | --- |
| `README.md` | `b123016b3589db87edc0727d991f6cf0a69dd1d6750f03a292290c73d7568b53` |
| `docs/PROJECT.md` | `dafc5335fcc35dc7c920fbeba609e7d03fa184da1f125d26d39ad508f2f521ad` |
| `src/scale.rs` | `1a301975075ed6cd0e99690e316c4746be395b104644bfe37dd279584d428f9d` |
| `src/analysis.rs` | `cbde3d936060539b750fe975f4f701920a7a78e4ab97ae218ae6773e19bdacb8` |
| `src/gui_gpui.rs` | `f3a95d2e0bac32a789274fb041c2e177c1fd1e45cb44684462c98c4bf946d1ae` |
| `src/library.rs` | `64ab92a8c1d9124e249eeee1aedf133f0fad92701319739bec8ff3d7479749c2` |
| `src/shared.rs` | `31040a2a3bcbd024e7bcce5bf8e364f18f4567a909d4b6d968bdeb1650900f68` |
| `src/synth.rs` | `3d96700ed4ecccf7a54136cb78722f075dc7754eeb24647683b95bbb7378ea6e` |
| `src/mts.rs` | `be8737e4f8cb8bcf6325e15bdb8529ff169349c46063ff014ca95f1234a2dfab` |
| `src/vst3/controller.rs` | `d9056be4afa76749ba3066372324e824ee55fb6e931f695a94aa8fa5c2381cf3` |
| `src/vst3/synth_audio.rs` | `3669c8ce4a99157dfc181b2b5d2afa6c9cec8a3bed6c6bf21b0af13c3cb0e96e` |

## finite feature acceptance

The table is the requested editor scope. A model or popup alone does not satisfy
it. Source implementation and automated checks are recorded below; every native
interaction/listening item still needs the finite Mac acceptance pass.

| feature | reference behavior | owning follow-up / evidence |
| --- | --- | --- |
| entry and editor | top-level `tuning` entry; integrated native editor, useful resizing, focus/shortcuts, lowercase themed controls and bundled typography | #2741 / PR352; source implemented, native acceptance pending |
| nodes | Hz and cents per degree, Enter apply, Escape/blur cancel, arrow stepping and Shift fine, scrolling, context insert/remove; root retained, minimum three | scale math in #2739; native interactions implemented in #2741; Mac acceptance pending |
| pitch line | drag/fine, selected frequency/cents readout, double-click insert-and-drag/remove, note numbers and density-aware cents, held/recent MIDI highlights | #2741 implemented; native acceptance pending |
| pitch space | cohesion/root-affinity variations, selectable point, dice regeneration, live Reach drag | harmonic analysis in #2739; variation/editor #2741 implemented; native acceptance pending |
| lattice and statistics | circular harmonic relationships, selected pair, root affinity, whole-scale and three-note relationship metrics | analysis in #2739; native display #2741 implemented; native acceptance pending |
| reference | root degree/key/Hz, repeat 600–2400 cents, ratio snapping and Shift-repeat snapping | model in #2739; native bindings #2741 implemented; native acceptance pending |
| scale structure | 3–48 notes, typed/step count and visible errors, adjacent MIDI keys advance degrees and wrap periods | model/math #2739; runtime #2740/editor #2741 implemented; native acceptance pending |
| settings | equal divisions, just major, snap selected node | scale math #2739; native actions #2741 implemented; native acceptance pending |
| history | canonical undo/redo, Cmd-Z/Cmd-Shift-Z, complete drag or insert gesture is one step | discrete project history #2739; grouped editor gestures #2741 implemented; native acceptance pending |
| local library | eight named slots, save/load/rename/duplicate/import/export, FREQS1/2/3 compatibility; names remain library metadata | bounded portable codec #2739; background file operations/UI #2741 implemented; native acceptance pending |
| audition | triangle/square/saw, level, attack, release; live pitch follows tuning with finite voice/note-off ownership | project-saved settings #2739; audio/UI #2741 implemented; native acceptance pending |
| save project | working scale and audition settings travel with project, independent of local library; malformed data refuses safely | #2739 |
| native playback | one table for native pitched instruments, Arrangement, Session, preview and export; explicit fixed-Hz/held-voice policy | #2740 / PR351 implemented and focused-tested; native listening pending |
| MTS-ESP | master status/client count, safe ownership and lifecycle; compatible clients only | #2742 / PR353 implemented and fixture-tested; installed-client/signing acceptance pending |

## canonical data contract

`hex-tuning` is std-only control-side scale math, analysis, portable preset codec
and validated fixed-size values. `hex-project` owns one `ProjectTuning`, snapshot
and typed `SetTuning` command. Existing transactional history prepares candidates
before committing. `hex-storage` current schema 31 requires enabled state, the complete scale,
audition settings and the complete device-asset manifest. The superseded alpha
schema 30 is explicitly unsupported. Existing readers through schema 29 retain
their disabled A440 12-TET defaults; no new historical conversion is added. Local slot names and process-global MTS state are not serialized.

Untuned projects preserve the legacy equal-temperament formula. Opening
the editor must not enable tuning, change the scale or dirty the project. The
Freqs seven-degree major scale (key 60 at 220 Hz) remains a deliberate selection,
not a migration default. Enabled scales support 3–48 ordered degrees with at least
one cent separation including the cyclic period seam, root keys 0–127, root 10–20,000 Hz and repeat 600–2400 cents.
Portable exports use round-trip-safe float text for root Hz, period and all degree
values; import/export preserves the authored float bits. Project JSON enables
correctly rounded float parsing so reopening preserves arbitrary valid root Hz.
Wrapped insertion and
count edits retain the root and validate the complete cyclic scale.
Portable input is capped at 8192 bytes before parsing, including inactive degree
values. Sample-rate/Nyquist admission remains an instrument preparation concern;
a valid authored table is not proof every instrument can render its highest key.

## runtime and MTS boundary decision

Native DSP must consume control-prepared immutable project tuning directly. MTS
is an external compatibility transport; missing MTS cannot disable native scale
editing/persistence or silently restore equal temperament. Runtime cache identity,
held/released voice transfer and fixed oscillator semantics require #2740's tests
before tuned playback is claimed. The native runtime in #2740 prepares both
Arrangement and Session from the effective table, with history and independent
pitch/PCM tests. Unsupported resource-backed providers still refuse explicitly
before publication. Offline export never registers an MTS master.

The selected architecture for #2742 is one desktop-owned publisher behind an
explicitly audited, pinned safe native dependency. Workspace unsafe remains
forbidden. Raw dynamic loading, ABI, owned registration and unloading stay within
that dependency; creating an excluded crate merely to evade lint is insufficient.
A helper process changes connectivity when MTS IPC is disabled and adds an
unnecessary protocol. Freqs's raw loader is not copied unchanged: ODDSound's
low-level `MTS_RegisterMaster` takes a pointer argument, whereas the inspected
Freqs loader declares no arguments. Verify against the pinned upstream wrapper
and exact installed library when implementing; do not exercise the suspect call.

One explicit active project owns broadcasting, independently of popup lifetime or
window focus. Generations/revisions reject stale work. Canonical, native-applied
and MTS-published revisions remain distinct: asynchronous external publication is
not sample-atomic with graph admission. Missing/incompatible library and another
master are visible states. Do not take over, reinitialize, reset another master's
table, install privileged libraries silently, or deregister a registration not
owned by this broker. Acquisition republishes the complete current table; disable,
project replacement and shutdown cannot be lost behind coalesced edits. Native
callback work has no MTS calls, locks, allocations, logging or final drops.

Hex currently has no production third-party plugin host. Compatible external MTS
clients may follow the broadcast in the configured IPC domain; unsupported
instruments do not automatically retune. External held-note response depends on
the client. Standalone macOS library-validation requirements must be verified
against the actual signed build in #2742; no signing policy is silently changed.

Primary sources: [ODDSound integration](https://github.com/ODDSound/MTS-ESP/blob/main/README.md),
[pinned master wrapper](https://github.com/ODDSound/MTS-ESP/blob/f214739b8832e7f297cb9970d0c0efbf783f1462/Master/libMTSMaster.cpp),
[master lifecycle contract](https://github.com/ODDSound/MTS-ESP/blob/main/Master/libMTSMaster.h).
These define transport ownership, not Hex native musical behavior.

## validation and delivery

Save and Export compare their captured project generation/revision before reading
the working scale or writing a file. Stale requests preserve existing slots and
create no export. Pitch-line dragging includes the first degree; protected-root
removal remains refused. Audition covers MIDI key127 and consumes each callback
refusal notification once, so successful edits are not overwritten by an old error.

Slot replacement retains staged same-directory rename. Create-new exports and
duplicates never replace a destination: where hard links are unavailable, an
exclusive file creation writes the validated bounded preset. A failed write can
leave an incomplete newly created file, reported explicitly and retained rather
than deleted. Filesystems without directory-sync support cannot promise that
extra durability step. Existing files and racing destinations stay intact.

The runtime continuation contract, scoped memory changes and evidence boundaries
are in [native runtime](project-tuning-runtime.md). The complete feature inventory
above remains authoritative; a passing runtime test is not editor/MTS completion.

#2739 covers model/math/history/persistence only. Native editor, audio, MTS and
optimized user-test delivery remain required goal work. No user-facing tuning
completion, listening pass, compatible-client pass, physical deadline or plugin
hosting claim follows from these source tests. Full exact-candidate local CI and
focused PR review precede merge; later source changes require fresh evidence.
The queued [finite native acceptance](tuning-native-acceptance.md) records the
Mac fixture commands, launcher-owned delivery and user/client checklist.
The complete feature uses repository-owned optimized stable-path delivery with
exact source/build/CI provenance and a finite user interaction/listening list.

### Dedicated native reference audition

The tuning window owns a transient reference-tone input independent of selected
tracks and the project-tuning enable switch. Its root focus accepts the existing
computer-keyboard piano mapping, with z/x octave and c/v velocity controls. Text
and numeric focus do not play notes. Key release works after modifier changes;
window deactivation, closing tuning, project replacement, Stop and retirement
cancel the lease. A canceled lease cannot restart from autorepeat.

The engine mixes a fixed-capacity PolyBLEP triangle/square/saw synthesizer after
normal playback. It borrows prepared 128-key frequencies, preserves phase and
envelopes through supported graph retargets, and refuses unsupported Nyquist or
voice cases explicitly. This is a live monitor, excluded from offline export.
Held/recent pitch-line highlights observe the synthesizer's actual held-key mask.
Grouped tuning previews and their cancel/commit publications retain a matching
keyboard/audition activation, and preparation refusal precedes canonical history
commit. Opening or auditioning does not enable tuning or dirty the project.

Cloud source evidence: the real desktop worker-to-audio bridge test rendered
nonzero PCM without a selected track; the callback retarget test checked an
independent waveform oracle with zero total/net callback allocations; GPUI tests
covered root/numeric focus, modified key release and close cancellation. Native
Mac listening, focus and physical deadline acceptance remain pending.

The pitch line also projects admitted Arrangement and Session MIDI gates from the
exact audio-applied prepared revision and route snapshot, including notes omitted
from the 32-note drawing preview. It queries a reusable fixed buffer on the UI
side at most about30 times per second, uses the applied source while edits are
pending, and keeps a400ms recent marker. Stop, faults, unavailable source/route
identity and project replacement clear held playback gates. These are MIDI gate
observations at the reported transport cursor, not synth loudness/release-tail or
physical hardware-input telemetry. Reference audition retains its actual held mask.
