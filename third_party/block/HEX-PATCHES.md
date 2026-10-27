# hex external block compatibility patch

Published block 0.1.6. MIT declared in the published Cargo.toml; this historical archive and upstream 0.1.6 tag contain no separate license text. All published files are retained.

The verified crates.io archive checksum and every original file hash are in
`hex-provenance.json`; `hex-patches.diff` records the sole `src/lib.rs` delta.
This package is excluded from the Hex workspace. Do not edit registry caches,
format vendor sources, suppress warnings or expand this patch without review.
See `scripts/check-dependency-patches.py` for rationale and evidence.
Run `python3 scripts/check-dependency-patches.py` after any dependency change.

Refresh only from a checksum-verified published archive, review upstream fixes,
retain licensing/provenance, reapply only necessary delta and revalidate.

Pulse2872 adds explicit `"C"` to all ten external declaration/function sites,
including macro templates. Rust's previous implicit external ABI is C; calling
conventions, argument/return types and ownership are unchanged. The guard reverses
both finite deltas and checks the exact original published source hash. The real
source now also compiles with `missing_abi` denied; no lint suppression is added.
