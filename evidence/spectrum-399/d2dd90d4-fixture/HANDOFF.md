# PR399 private spectrum fixture return

Owner d2dd90d4cca1efd2ec1df787b2e7043af1998e08 / tree228cb54b82d40addcd56594a3516e1df2d53bc8a, direct parent e30175bc764af9dbb7d2d67bb5a04b995fd0fb66 / tree4200be25c15e1e877dc402c88a4b006f7e3fa366. Published source4/399-spectrum-fixture-e301. Canonical PR399 not overwritten; sole merger integrates/qualifies.

Only the named private project_spectrum_layers_preserve_all_members_colors_and_clear_replaced_context test changes. Authored Orange plus original hardcoded Orange reproduces the old invalidation assertion failure (1 FAILED/cargo101). Original full UI failure did not report initial hue; this does not retroactively establish it. Final fixture explicitly expects Orange->Orange cache reuse, then asserts actual different old/target Orange->Blue->Orange, corresponding accent/points/invalidation each time. Counts0/1/2/256, all-member color/points/master, unchanged revision reuse, removal, stale and replacement checks retained; all other test functions byte-identical (scope.json).

Candidate ProjectSpectrum suite4 PASS/0failed/0ignored. Strict hex-ui all targets/ui-test-support Clippy and architecture/GPUI/dependency/UI-style/device-layout guards/fmt/diff pass (strict-checks.json). No full UI/full21/native or app operations. No production cache/randomness/painter/controller/Service/Engine change. No product cache defect observed with genuinely different color.

Original merger negative full21/e301 summary10f3c46a8a43866f206617ceaddfe3ca8061abba06a7588744595ac6edc55c2b, steps1-11PASS/12UI913PASS1FAIL3oldignored/13-21NOTRUN, remains separate and authoritative. Source2 palette and three legacy fixtures remain in parent unchanged. Original negative archive source bc7a6366d1f37f2f2c5ba06a1028e56edb0e423e, SHA4e678cb68a83acb13c650005c827fc34c43ae6c775e41808c3b58acae50932fe was described by root; not independently recovered or relabeled here.

Scope is source-ready only; Pulse In Progress until merge. Sole merger owns prospective full CI/review/merge. No new native request; no physical acceptance claim.
