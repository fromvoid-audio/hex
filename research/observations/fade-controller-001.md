# FADE-CONTROLLER-001 — locate a fade-control callback

Status: static analysis observed; fade behavior unconfirmed.

- Reference: installed Live 12 Suite 12.4.6 (2026-09-10_0de5c8fa9a).
- Universal executable SHA-256: `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
  Read-only inventory was repeated on 8 October 2026 before this study; unchanged.
- Analysis input: arm64 slice made with Apple's `lipo -thin arm64` from the
  unchanged original executable. SHA-256:
  `08cd55064ef7f30ace2bab68327f84ea7184eb1220d98082da19fbbf1b6db780`.
- Host: Apple Silicon/macOS 26.6.2. No audio device, sample rate or block size is
  involved in this static study. No user set, factory asset or audio render used.
- Tools: Homebrew Ghidra 12.1.4 PUBLIC, build 20260921; OpenJDK 21.0.12.1;
  Ghidra loader language `AARCH64:LE:64:AppleSilicon:default`.
  Homebrew's arm64 Tahoe Ghidra bottle metadata identifies SHA-256
  `d2a6222389d18d32d48e55079407c1d877d90f86d692911b0399a6ff5e440a9a`.
- Question: locate the control path for an audio-clip fade-in adjustment as a
  prerequisite for later fade-editing experiments. This is not an audio fade curve
  or render-path measurement.
- Inputs/settings: one selected exported callback symbol referencing
  `AClipFades::FadeInController`; no whole-program auto-analysis; at most two
  headless CPU workers; 30-second per-function decompile timeout.
- Steps: inventory/hash original; extract/hash arm64 input into ignored
  `research/private/live-12.4.6-arm64`; import once with `-noanalysis`; run the
  original `research/tools/HexSelectedFunction.java` at `0x10283f478`; inspect
  entry flow; process that existing project at target `0x10282c184`.
- Private project: `research/private/live-12.4.6-arm64/live-12.4.6-arm64.gpr`,
  program `Live.arm64`. Import and both process runs completed and saved.
- Observed entry: the named callback's four-byte instruction is a direct branch
  to `0x10282c184`. That target begins with stack allocation. Its inferred body
  contains three discontiguous ranges totaling 976 bytes, including shared flows
  near `0x1028290b8` and `0x10282abec`.
- Private output hashes: `fade-controller-thunk-002.txt`:
  `989cccba28d5e2a93d5af8dd45277b5a79dda29358cc4e81a36122792d6ecf72`;
  `fade-controller-target-001.txt`:
  `faf87b36024cc036f9f934db84aaa295ec8b7146ce39607d3ab57da095a39fce`.
- Warnings/limitations: six decompiler comments flag possible PIC call-to-branch
  rewriting, removed unreachable blocks and overlapping globals. Types remain
  inferred; the discontiguous function boundary is not independently validated.
  Linked libraries were identified but not imported/analyzed.
- Confidence: high for the imported architecture, entry address and direct branch;
  medium for identifying a fade-control callback from its exported symbol;
  unconfirmed for controller units, default fade lengths, clamping, curve shape,
  sample-level onset/end behavior and applicability to the audio render path.
- Alignment/tolerance: none yet; an audio experiment must specify these before
  comparison. Warp/fades/gain/device/export settings are not applicable here.
- Independent Rust implementation: none derived from this output. No pseudocode,
  proprietary executable or database was added to application code or public docs.
- Next: establish a user-authored single-parameter fade fixture and discriminate
  control units/curve behavior by rendered output; validate this candidate path
  against that observation before deriving a behavior specification.
