# engineering health

Snapshot: 5 November 2026, Linux x86_64, Rust/Cargo 1.94.0. The runtime goal is
active; no scheduled task was created. Reconcile the checkpoint with live Pulse
and Git after an interruption.

Latest cycle: [subframe tempo test partitions](https://pulse.iocore.dev/#/issues/5034),
[PR #2090](https://github.com/PORTALSURFER/hex/pull/2090). Nine independent rate/mode
cases retain all 27 combinations, the same fixtures, complete history and full PCM
oracles. The formatted check body matches the original. At matching baseline
`d78d2fb484e74fa062fdfdd3bfc1ca0fd8a715f4` and candidate
`0cc77317ca3f29c486f6543e61925332c85c4b28`:

- Old case median: 2.942 s across three isolated runs. Largest new isolated case:
  0.666 s; selected default-parallel matrix median: 1.740 s.
- Serial matrix median: 3.190 s. Full library: 11.825 s before and 11.947 s after.
  These results do not establish less total work or faster whole-suite execution.
- Contended cases can still exceed 1 s. The broader unit-budget acceptance remains
  open; this is a test scheduling improvement, not application CPU evidence.

Current-base verification at `bafdd2ac8a0f9f90c513e7ad01ff052c00aec6b8` over
`c38297e419cfa6e0139e0971870b3a5f34c4361f`: all nine matrix cases passed; the full
Session library passed 481 tests with three existing manual profiles ignored in
12.141 s. Strict owner all-target Clippy passed in default and additive-feature
configurations. Canonical PR/Pulse records own final guard and integration evidence.

The [Sampler readiness experiment](https://pulse.iocore.dev/#/issues/3851) was
retired after inconclusive paired timings; the original fixture was restored.
The [Session audit](https://pulse.iocore.dev/#/issues/3850) refreshes historical slow
cases and preserves meaningful rate/mode, large-source and history coverage.

The [private-project CPU report](https://pulse.iocore.dev/#/issues/4965) still needs
its matched optimized Mac/project baseline. Headless checks establish neither
native UI/listening acceptance nor physical deadlines or application speed.
Previous verified outcomes and exact provenance remain in
[measurements](measurements.jsonl), Git history and canonical PRs/Pulse. Ignored
raw logs remain executor-local; essential evidence also belongs in the live records.
