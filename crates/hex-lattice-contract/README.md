# hex-lattice-contract

## Purpose

Bounded, toolkit-independent metadata and opaque state contracts shared by
authored native devices, projects, storage, and the DSP lattice. Parameter display
text is nonempty lowercase text without control characters, limited to 128 UTF-8
bytes per label/unit/choice; selectors expose at most 256 choices. Both native
registries enforce these descriptors before admission.

## Dependencies

Rust standard library only.

## Does not own

DSP, device implementations, project editing, serialization formats, UI, plugin
hosting, or runtime device registries.
