# research evidence

This folder contains recorded reference observations and reproducible measurements.
Requirements, proposed experiments, Hex implementation status and acceptance
checklists belong in [Pulse](https://pulse.iocore.dev/), not in research notes.
Code descriptions belong in source doc comments.

- `catalog/`: immutable inspected source identities and hardware/OS measurements.
- `observations/`: actual observations, their confidence and limits,
  plus raw measured logs/CSV/manifests. An old observation is not current acceptance.
- `fixtures/`: original executable experiment inputs and comparison helpers.
- `private/`: ignored private captures and local artifacts; never commit them.

For a new experiment, select one question and use original/licensed inputs. Record
exact source hashes, tools, host/rate/block settings where applicable,
fixture hashes, reproduction actions, one changed variable, measured output and
units, numerical tolerance/alignment, and unresolved questions. Keep observation,
inference and unknowns distinct. A manual description or screenshot
does not establish sample-level behavior. Do not hide timing defects by alignment.

Keep raw outputs beside their existing manifest; do not copy historical archives
into new packets. Preserve original media, user sets and licenses. Do not
distribute proprietary code/assets. Implement independently.
