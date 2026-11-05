# development workflow

1. Read the live Pulse ticket and canonical PR. Reuse its branch; preserve unrelated
   or held source and keep one writer per area.
2. Define one finite boundary change and observable acceptance. Name affected
   crate/module owners, responsibilities, permitted dependencies and public typed
   command/query/resource APIs in the PR or Pulse. Inspect their actual consumers.
3. Implement against source-owned contracts, using visibility, types, dependency
   checks and contract tests to enforce isolation. Internal devices use existing
   host APIs; missing capabilities are explicit bounded host-contract changes,
   checked with affected consumers. Keep API/lifetime/failure docs beside the code.
4. Run owning regressions, affected contract/consumer checks and relevant feature
   configurations, plus proportionate formatting/lint/build checks. Preserve
   lifecycle, malformed-input, realtime/allocation, current-format persistence and
   Undo/Redo coverage where relevant. Inspect the final diff.
5. Record the exact candidate/base, check commands/results and reasons for scope
   and omissions in the canonical PR. Broaden for concrete impact or unresolved
   risk; explain unknown selection and resolve or conservatively check that area.
   A small diff or passing mock alone is insufficient evidence of isolation.
6. Review and merge the verified candidate under AGENTS.md's standing authorization;
   confirm the actual merge and update Pulse. Recheck affected inputs after conflict
   resolution or material repairs. Keep native acceptance separate.

## Checking toolchain and hashing inputs

Use `rustc --version` from the checkout to confirm the
[toolchain pin](../rust-toolchain.toml). Inspect resolved workspace members with
`cargo metadata --locked --no-deps --format-version=1` to check that their
`rust_version` values match the workspace declaration.

When reviewing SHA-256 dependency changes, inspect production and build edges
separately from compatibility-test dependencies:

```sh
cargo tree --workspace --locked --edges normal,build --invert sha2@0.10.9
cargo tree --workspace --locked --edges normal,build --invert sha2@0.11.0
```

Hex-owned production and build dependencies use `sha2` 0.11.0. The GPUI HTTP
client retains an independent 0.10.9 edge. The media tests also retain 0.10.9 to
verify digest compatibility, so a tree with development edges includes that
additional path.

## Choosing checks

A merge, core path, policy edit or next module does not alone require a full suite.
Use the existing runner when its selection fits the affected boundary:

```sh
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
```

The selector covers affected packages and a reviewed compressor boundary; see
[local-ci.md](local-ci.md). A reviewed change-based command set may qualify under
AGENTS.md without a runner merge certificate. Record its coverage and unresolved
limits honestly. Do not rewrite runner reports, weaken assertions or claim a
partial quick/edit report alone qualifies. A failed, interrupted, stale or
incomplete required check set cannot qualify.

Examples of scope reasoning:

- Device DSP behind unchanged host contracts: owning deterministic render/error
  regressions, affected real-host contract tests, applicable feature builds and
  package lint/format checks. Expand if timing, resources or latency cross owners.
- A resource or command contract change: owner tests plus affected consumers,
  lifecycle and malformed-input cases; add allocation or persistence/history
  checks when those paths change. A mock consumer alone cannot qualify the boundary.
- Prose-only policy/workflow change: review links, paths, examples and contradictions.
  If executable selection changes too, check selector fixtures and affected plans.

Transition one coherent verified boundary per PR, spanning necessary owners and
consumers, including required current-format persistence/history changes. Reuse
prior evidence only while relevant
source, contracts, configurations, toolchain and dependency inputs are unchanged;
record the original provenance and why it still applies. Recheck changed consumers.
Do not repeat full audits for each slice or next module. Qualify selector changes
once against their selection risks, including unknown paths, dependencies and
feature coverage. Tests do not mathematically guarantee unrelated code.

Keep one persistent checkout/target per executor and serialize target use with the
repository tools. Retain raw logs/summary and immutable source references with the
canonical PR; do not recursively copy historical evidence into new archives.
A comprehensive run remains available when justified by concrete breadth or risk.

Native UI/listening acceptance is separate from source checks and merge. For a
user-test app follow [test-builds.md](test-builds.md); use an optimized candidate,
finite checklist and exact included revisions. Never force-quit a user's project.

For optional end-user release-note preparation, see `python3 scripts/changelog.py
--help`. Generation is explicit; ordinary CI validates retained notes offline.
