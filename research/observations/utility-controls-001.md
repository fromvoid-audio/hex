# Utility bounded static analysis

- Reference: installed Live 12 Suite 12.4.6 (2026-09-10_0de5c8fa9a), rechecked
  read-only on 9 October 2026. Universal executable SHA-256:
  `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
- Existing arm64 slice SHA-256:
  `08cd55064ef7f30ace2bab68327f84ea7184eb1220d98082da19fbbf1b6db780`.
- Tools: Apple `nm -arch arm64 -nm`; Homebrew Ghidra 12.1.4/OpenJDK 21.0.12.1;
  language `AARCH64:LE:64:AppleSilicon:default`.
- Questions: locate Utility DC enable and bass-mono cutoff control paths; identify
  the boundary between parameter updates and DSP before audible comparisons.
- Method: identify named `OStereoGainProcessor` callback symbols; process the
  existing private Ghidra project with `-noanalysis`, `-max-cpu 2`, and the original
  selected-function helper's 30-second decompile timeout. No import repetition,
  whole-program analysis, reference modification, user set or factory content.
- Project: ignored `research/private/live-12.4.6-arm64/live-12.4.6-arm64.gpr`,
  program `Live.arm64`. Both runs reported script export success and project save.
- Exports and logs remain in ignored private storage; no pseudocode, executable
  or analysis database is distributed or copied into Hex implementation.

## observations, inference and unknowns

The named DC callback entry is `0x1021a46ec`. Its first instruction compares the
incoming scalar with zero. Ghidra inferred a 596-byte discontiguous body including
shared paths, with warnings about PIC call-to-branch rewriting and unreachable
blocks. The recovered control flow indicates a boolean enable update, an apparent
history clear on an off-to-on transition in one path, and separate legacy
processing selections. Types, field meanings, branch applicability and function
boundaries have not been independently established. The apparent history clear
is a hypothesis for a toggle experiment, not an accepted DSP/reset specification.

The bass-frequency callback is `0x1021a4578`. Ghidra inferred a 148-byte body
including a shared helper at `0x104982954`. The recovered update appears to
retain the supplied frequency and derive a coefficient from frequency divided
by a stored scalar, with an angular bound. This supports looking for sample-rate
dependent preparation; it does not prove that the scalar is the active sample
rate, establish the filter order/response, or describe where/how samples use the
coefficient. No recovered polynomial or other implementation code was copied.

Confidence is high for reference identity, architecture, named entries and
successful bounded exports; medium for the control-path interpretation; low for
reset-state meanings; unconfirmed for filter response, phase, smoothing, mode
order, frequency display law or audible parity. There was no sample rate, block
size, alignment policy or numerical tolerance because no audio render occurred.

Private export hashes:

- `utility-dc-thunk-001.txt`:
  `2a7cd1d3bc8c8092d88552c5f62831e06e25d806536225ab0e03c17af5c59988`
- `utility-bass-frequency-001.txt`:
  `12fa1ab5d48cb58a89fad052aea09b659bba06e14f19814156be0b5174d8f8e6`
