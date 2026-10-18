# user-paused native binding checkpoint

The user paused all local work on17 October2026, then authorized checkpoint
pushes and primary-dispatched cloud continuation. No automatic local resume,
build, test, merge or installation is authorized by this checkpoint.

Canonical W06 #2675 / PR291, branch pulse/task-2675-g1-d8ec864b09c6.
Implementation checkpoint3c45ccc54f295a821ec8a0cbd96293dc952bb438 is pushed.
It is based on unmerged W05 7ab8fa4b624168abb48b154c0691ca35fa787da3;
it has not integrated W04/fades/catalog fixes or the later W05 repair.

The retained actual same-update finalization reproducer fails at
8ac4e1494c228b52bd5726785bcaf2933eb144d0 (5pass/1fail, queued Begin reopens
a finalized drag). Exact positive e009db34d15bc7a33367dc5ecd1163896e17cc4d
passes6/6 with0ignored in13.58seconds and a later genuine drag still works.
Raw logs and SHA256 checksums are adjacent. The earlier raw-phase/retirement
regressions and all other Mac artifacts remain in the local verification
checkout; they are not implicitly accessible from cloud.

The implementation extracts bindings/gesture adapters, validates owner/emitter/
generation/type/descriptor, holds exact numeric gesture leases, revokes them
on retirement, preserves failed drafts/barriers, and fences completed queued
effects until the same installed binding reopens. It adds no second edit model.
Read-only source review accepted it. Full exact integrated local CI and native
acceptance are still outstanding; these focused passes are not merge evidence.
Integrate current canonical W05/main, preserve both contracts, rerun meaningful
regressions and full final checks. Do not duplicate PR291.
