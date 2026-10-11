# external xattr 0.2.3 compatibility patch

Baseline: published crates.io xattr 0.2.3, archive SHA-256
`6d1526bbe5aaeb5eb06885f4d987bcdfa5e23187055de9b83fe00156a821fabc`.
The archive was verified before extraction. Upstream repository:
https://github.com/Stebalien/xattr. All original archive files, MIT/Apache-2.0
licenses and VCS metadata are retained unchanged except `src/sys/mod.rs`.
Published VCS metadata is descriptive; the archive checksum identifies the baseline.

The only source delta uses libc::ENODATA for the missing-attribute sentinel on
Linux and Android. macOS, FreeBSD and NetBSD continue to use libc::ENOATTR;
unsupported targets retain the upstream behavior. No syscall or unsafe block
changes. The patch remains an excluded external dependency under the existing
GPUI → zed-async-tar → xattr boundary, never a Hex workspace crate.

`hex-provenance.json` records original and patched hashes; `hex-patches.diff`
records the exact delta. Run `python3 scripts/check-xattr-compat.py` for the
native public API regression, or add `--target x86_64-unknown-linux-gnu` for a
Linux compile regression on macOS with the Rust target installed. Cross-target
compilation does not execute Linux syscalls or establish full desktop support.
When refreshing dependencies, review whether the parent permits a maintained
xattr release before dropping this patch; do not edit the Cargo registry cache.
