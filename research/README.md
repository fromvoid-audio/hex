# research evidence

This folder contains recorded reference observations and reproducible measurements.
Requirements, proposed experiments, Hex implementation status and acceptance
checklists belong in [Pulse](https://pulse.iocore.dev/), not in research notes.
Code descriptions belong in source doc comments.

- `catalog/`: immutable inspected reference/source identities. Recheck the installed
  build with `sh scripts/inventory-live.sh` before a new Live study.
- `observations/`: actual visual/static observations, their confidence and limits,
  plus raw measured logs/CSV/manifests. An old observation is not current acceptance.
- `fixtures/`: original executable experiment inputs and comparison helpers.
- `tools/`: the bounded Ghidra helper and its invocation instructions.
- `private/`: ignored proprietary captures, binaries and analysis databases;
  preserve these locally and never commit them.

For a new experiment, select one question and use original/licensed inputs. Record
exact reference/source hashes, tools, host/rate/block settings where applicable,
fixture hashes, reproduction actions, one changed variable, measured output and
units, numerical tolerance/alignment, and unresolved questions. Keep observation,
inference and unknowns distinct. A manual description, screenshot or decompilation
does not establish sample-level behavior. Do not hide timing defects by alignment.

Keep raw outputs beside their existing manifest; do not copy historical archives
into new packets. Preserve original media, user sets and licenses. Do not study
activation/licensing or distribute proprietary code/assets. Implement independently;
approximate decompiler output is evidence, never application source to paste.
