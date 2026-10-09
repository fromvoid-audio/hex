# Reverse-engineering workflow

## Evidence pipeline

1. Inventory the exact reference build and architecture; keep binary analysis
   databases in ignored `research/private/`.
2. Select a small question tied to an implementation requirement. Use the manual
   for vocabulary and observable experiments for semantics.
3. Create a minimal user-authored Live set and deterministic source audio/MIDI.
   Change one parameter per experiment and retain baseline plus variant.
4. Record exact actions, versions, audio settings, expected discriminating result,
   outputs, alignment policy, and numerical tolerances.
5. For ambiguous behavior, trace the relevant binary path with a decompiler.
   Save addresses together with binary hash and architecture; addresses alone
   cannot survive updates. Decompiled code is approximate recovered structure,
   not the original source, and function names/ownership may remain uncertain.
6. Write a behavior specification that separates observation, inference, and
   unknowns. Implement it independently in Rust and add a regression fixture.
7. Compare results and report remaining mismatches. Do not hide a timing defect
   through automatic alignment or relax tolerances without a reason.

Limit analysis to the selected audio/workflow questions. Activation, licensing,
DRM, credential extraction, and patching the installed app are outside this
project's current work. Do not commit Ableton binaries, decompiler exports,
factory content, license material, or user projects containing private data.

## First experiment matrix

| ID | Question | Input/variation | Measure |
| --- | --- | --- | --- |
| CLIP-001 | Unwarped clip start/end | Impulse, trim by one sample | First/last nonzero frame |
| CLIP-002 | Loop boundary | Short asymmetric stereo buffer | Repeated frame sequence |
| MIX-001 | Track gain law | Known low-level tone, several dB values | Amplitude ratio |
| TRANSPORT-001 | Stop/restart and seek | Short loop, documented actions | Cursor and first emitted frame |
| ARRANGE-001 | Overlap policy | Two identifiable clips on one track | Which signal survives |

Start with warp off, fades off, no devices, no normalization/dither, matching
sample rates. Explicitly record these controls rather than assuming defaults.
Interactive transport experiments need event traces or recorded capture, not
only offline exports. Define each fixture's acceptable error before comparing.

## Observation template

Copy `research/observations/template.md` for each completed experiment.
Do not mark a behavior confirmed until the supporting artifacts are available.

## Bounded headless study now available

Ghidra 12.1.4/OpenJDK 21.0.12.1 are installed through Homebrew. Use the original
[research helper](../../research/tools/README.md) for a selected entry and keep
all outputs in ignored private storage. The first
[fade-controller study](../../research/observations/fade-controller-001.md)
records actual import/decompile success and unresolved boundaries/types. Inspect
script success markers, artifacts and warnings; headless process exit alone is
not proof of a successful script or a correct decompilation. Do not enable full
analysis or expand the selected function scope merely to get a cleaner-looking
result. Observable fixtures remain required before behavior is confirmed.
