# finite native tuning acceptance

This is the queued Mac follow-up for goal4 and canonical PR350–353. It is not
permission to start a second local thread. The coordinator assigns the one local
lane; the launcher remains the owner of one combined optimized app delivery.
The final Pulse handoff supplies the exact candidate, four PR heads and cloud
receipts. Do not test an unrecorded working tree or infer native acceptance from
Linux checks.

## prerequisites and source checks

Record the clean checkout's full commit, Rust/Cargo versions, macOS/architecture
and SDK/toolchain. Confirm the candidate contains both schema31 data families,
the merged launcher and signing/provenance changes. Preserve any existing dirty
work rather than resetting it. Use disposable projects and new preset paths;
never convert or rewrite unsupported alpha30 projects.

Run the Mac-specific package checks from the exact checkout:

```sh
cargo test -p hex-mts --locked
cargo test -p hex-storage --test tuning --test device_assets --locked
cargo test -p hex-audio --features allocation-probe --test callback_allocations --locked
CARGO_TARGET_DIR=third_party/mts-esp-master/target cargo test --manifest-path third_party/mts-esp-master/Cargo.toml --locked
CARGO_TARGET_DIR=third_party/mts-esp-master/target cargo clippy --manifest-path third_party/mts-esp-master/Cargo.toml --all-targets --locked -- -D warnings
```

These compiled fixtures exercise the platform loader boundary without touching
the installed library or machine-wide master. Retain commands, source identity,
exit codes and output. Use repository target-ownership rules; do not adopt or
clean another active checkout's cache. Report a platform failure to cloud5 before
changing source; a repair needs fresh exact-source evidence.

Record the bounded keyboard handoff release probe on the actual Mac CPU, with
its own inactive/owned profile target and no concurrent compiler job:

```sh
CARGO_TARGET_DIR=artifacts/tuning-native-profile/target cargo test --release -p hex-engine --lib profile_keyboard_tuning_handoff_copy_cost --locked -- --ignored --nocapture
```

Retain the printed workload, runtime/override sizes and same-tuning/retune
percentiles, plus hardware, toolchain and background-load context. This is a CPU
handoff measurement; physical CoreAudio/listening evidence remains separate.

## launcher-owned delivery

The launcher owner supplies one optimized, signed, stable-path combined bundle
through the current repository delivery tooling, with exact candidate, included
PR heads, checklist and receipt. Preserve the launcher's build-only/test mode
and provenance distinctions: a build-only receipt does not assert that tests ran.
Verify the actual signature/team and receipt against the delivered bundle.

App replacement requires the user to save their work and quit Hex normally.
Never force-quit, discard unsaved data, create competing test-app copies or start
an installation from this checklist. Ordinary user Hex use may continue until
the coordinator schedules that delivery window.

## user interaction and listening

The user performs these interactions while present, with no parallel automated
computer-use session. Record pass/fail and a reproducer for each failed item:

1. Open tuning in a new neutral project. Opening does not enable tuning or dirty
   the project. Check useful resizing, lowercase typography, focus, scroll,
   shortcuts, keyboard accessibility and traffic-light clearance. Record any
   missing accessibility semantics explicitly.
2. Edit degree Hz/cents with Enter, Escape, blur and arrow/fine steps. Insert,
   drag and remove on the pitch line and context menu; exercise the 3/48-degree
   bounds and protected root. One whole gesture is one Undo/Redo step.
3. Set root degree/key/Hz, non-octave period and ratio/Shift snapping; exercise
   equal divisions, just major and selected-note snap. Check pitch-space point
   selection, Reach/dice changes and lattice/pair/whole-scale statistics.
4. Save/load/rename/duplicate among eight local slots using disposable names.
   Use an empty/disposable slot; never overwrite an occupied user preset to
   manufacture acceptance. If none is available, leave that write check pending.
   Save/reopen with a locally pending tuning Change and with a reserved Commit:
   both must persist the latest value. Closing/quitting an acknowledged but
   uncommitted preview must prompt; canceling close must preserve the work.
   Import/export FREQS1/2/3 scratch fixtures, including malformed refusal.
   Save/reopen a tuning scratch project and verify working scale/audition,
   independent of slot names. The storage fixtures above verify both schema31
   data families together. For a reviewed bound-device project fixture, record
   any unsupported resource-processing refusal separately: the file and current
   project must stay intact, with no silent removal of bindings or conversion.
   Resource-backed playback admission remains the native host owner's gate.
   Loading/importing the identical scale must preserve clean state and Redo.
   Start Undo or another canonical edit, then immediately try a new tuning edit:
   it must wait rather than resurrect an older whole tuning value.
5. With no selected track and project tuning disabled, audition triangle, square
   and saw; change level/attack/release and tuning while holding/releasing notes.
   Numeric/text focus must not play notes. Modified key release, deactivation,
   close, Stop and project replacement must leave no stuck note or autorepeat
   restart. Press Stop while reference preparation or publication is pending:
   the delayed candidate must not restart a held tone. Include a refused held
   high note, then retune it into the supported
   range without releasing the physical key: it should become audible while
   already sounding voices keep their phase/envelope. Check held/recent reference
   and Arrangement/Session gate highlights.
6. In sine, FM4 and Operator scratch projects, compare neutral and enabled custom
   tuning in Arrangement, Session and keyboard monitoring. Include root offsets,
   7/19-degree non-octave scales, rapid edits/Undo and held/released voices.
   Listen for unintended restart/clicks; fixed-Hz oscillators remain fixed. Check
   the exported PCM against the same authored pitches. Record audio device,
   sample rate/buffer and any underrun/fault; listening is not a deadline proof.

## installed MTS and compatible client

Record whether the existing compatible library is present at
`/Library/Application Support/MTS-ESP/libMTS.dylib`, its version/identity, the
actual bundle signing flags/entitlements and the client's version/IPC domain.
Do not install privileged software, change entitlements, buy a client or reset
another master. Absence is explicit unavailable evidence and leaves this item
open; fixture success is not an installed-client pass.

1. With an existing foreign master, explicitly enable Hex broadcast: observe
   blocked, with the other master/table unchanged. Do not end the user's other
   session to manufacture a pass. Test reacquisition only when that owner has
   voluntarily released; use explicit retry.
2. In a free domain, explicitly enable the intended Hex project. Confirm active
   status, client count and actual compatible-client frequencies/metadata for
   the custom root/period. Compare a second publication and held-note behavior;
   record client limitations and asynchronous native/MTS revisions separately.
   A transient preview must publish only after its exact native candidate applies;
   a refused nonterminal preview must preserve the previous published table.
   Check Cancel/Commit restoration and canonical commits without an audio output
   separately. Worker preparation alone is not an applied-audio receipt.
3. Change window focus: ownership stays explicit. Close the tuning popup:
   broadcasting continues. Disable, replace the project and quit normally:
   owned registration is released, a different master can acquire, and native
   Hex tuning worked throughout unavailable/blocked states.
4. Capture an actual signed-bundle loader failure verbatim. If library validation
   prevents loading, report the bounded signing decision through the coordinator;
   do not silently weaken signing policy. Retain the delivered bundle's identity.

Report source/build/fixture evidence separately from user interaction, physical
audio/listening and real-client evidence. Keep goal4/native acceptance open for
any unperformed item and relay results to the existing Pulse tickets.
