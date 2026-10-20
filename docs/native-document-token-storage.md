# Borrowed document and journal storage

The existing Session worker owns `DocumentHandle` and `Journal`. These are
ordinary inputs to a future whole-Service construction inventory. Selected
inspection of their storage does not qualify compilation, native publication,
retirement, constructor peak, or a complete root summary.

`Journal` retains exactly a `PathBuf` and optional `Vec<u8>`. Its borrowed
accessor exhaustively matches both private fields. Engine observes the actual
path and checkpoint capacities, including spare capacity. The enclosing inline
Journal belongs to the actual worker body. Inspection does no file IO.

`FileToken` remains the private Storage tuple containing `Arc<[u8]>`, with the
same Clone/content-check/load/save/replacement/recovery behavior. Its nominal
borrowed observation exhaustively selects that actual Arc field; it performs
no allocation, cloning, conversion, mutation, or adoption. One audited Session
adapter projects the actual DocumentHandle body, path, token body and token's
borrowed Arc field. Engine checks containment of all those real fields before
observing a token. The public internal interfaces are source-audited; they do
not claim hostile downstream unforgeability. The architecture guard restricts
the accessors, constructor sites and finite Engine projection producers.

The separate family is `storage-file-token-vec-and-copy-slice-arc-v1`.
The only four token births are the two Vec routes in `files.rs`, the replacement
Vec route and recovery's slice-copy route in `replacement.rs`. There is no raw,
custom allocator, mutable Arc or Weak ingress into this nominal family.

The audited std source is Rust 1.94.0 commit
`4a4ef493e3a1488c6e321570238084b38948f6db`,
[library/alloc/src/sync.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/sync.rs),
SHA256 `ce15d00d92133cf1f55cd1905d7fe6b4e23e62090dabff6c68fae2d7fe78fe8f`.
The relevant paths are:

- `From<Vec<T,A>>`: retains the old Vec while `allocate_for_slice_in` allocates
  the Arc, copies elements, then releases the old buffer.
- `From<&[T]>`: `ArcFromSlice` selects the copy specialization for bytes;
  `copy_from_slice` calls `allocate_for_slice`. The ordinary clone path also
  uses a single length-bound slice allocation; no child owner exists in u8.
- Both allocation helpers pass the actual slice `Layout` to
  `allocate_for_layout` and `arcinner_layout_for_value_layout`. The latter
  extends the two-atomic Arc header by the slice layout and pads alignment.

The independent build selector requires the pinned compiler, ordinary native
installation and matching supported Linux x86_64 GNU or macOS aarch64 target.
Ambiguous wrappers/flags, cross targets or unsupported compiler installations
stay Unknown. Its evidence is written to `document-token-arc-build-evidence.txt`;
NativeDeviceState's Vec-only declaration is not reused as this family proof.
This is a version-bound requested-layout declaration, not physical allocator,
RSS, security, or native execution attestation.

Engine observes one shared body/header per actual allocation and one handle per
actual owning Arc field. Repeated observation deduplicates the same field and
body. Distinct equal-content tokens remain separate. A missing alias prevents
closure and Weak observations refuse. Counts and allocation/field equality
serve closure and deduplication only; they cannot adopt a foreign token.

Birth-time preflight remains a separate obligation: the actual old Vec capacity
or recovery-buffer ownership overlaps the new Arc and constructor scratch.
The later visitor cannot reconstruct those dead inputs or retroactively certify
their peak. Full startup/Service/current-history-captured-source/File/queue and
native union, both caps before new compiler/native owners, qualified default
startup and Sampler PCM/ACK/replacement/retirement/export/seek remain open.
