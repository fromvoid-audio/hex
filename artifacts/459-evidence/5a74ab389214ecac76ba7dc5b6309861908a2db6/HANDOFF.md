# SAME2857 / PR459 owner handoff

Source 5a74ab389214ecac76ba7dc5b6309861908a2db6/treeda3d9f931b4dce970d3c52c50f52fed7c70a57a2.
Expected before 2640b0eeaee97f06d4d929345ff129fb2f489d41/tree82cbed32986e067eed771fe6c5ed8f93ca395303.
Owner source-fix/459-vendored-target-identity; three ordinary commits after2640's
already-recorded one main integration. Apply ONLY three-file delta,457insert11delete.
Canonical459 remains plan5b4/open/draft/unmerged/InProgress. No duplicate PR,
canonical overwrite or main chase. Preserve current413 expected-owner/CAS/recovery
hunks when composing local-ci.py; never replace whole files from this old base.

## Actual RED
Diagnostic e2a8805d829162ac992ca6043916c645d01509ea adds only actual private
Cargo fixture. First run terminal1/0.646s/1executed0skip. Two Git worktrees;
Sample.old_field becomes new_field, consumer changed; vendor source mtime10s
older than cached artifact. Existing guarded workspace-only clean reuses vendor:
actual rustc E0560 says no new_field, suggests old_field. NOT a fake Cargo or
artifact-absence wrapper. A debug/release passed/output8. Before failed Bsuccess
assertion, unrelated external-helper rlib AND release bytes checked unchanged.
Raw compiler log, source/mtime/commit/target records and fingerprints preserved
before temporary fixture cleanup. No shared Cargo target mutation.

## Repair
After existing target lock AND current target/repository/workspace ownership
checks, full locked Cargo metadata resolves nonworkspace local path packages,
including registry patches. Streaming hashes include sorted package-relative
names, file bytes, executable bits, manifests/build scripts/assets. Relative
package root/name/version keys survive worktree changes; outside roots keep
actual absolute identity. Existing dev owner marker adds hashes; malformed
identity/inventory fails before mutation. Changed/unrecorded packages ONLY append
to selective dev clean; Cargo rebuilds dependent consumers. Valid old marker with
missing provenance invalidates local packages once, without adopting unknown or
foreign targets. Name collisions refuse because Cargo ignores ID qualifiers.
Unchanged registry/local dependencies, release artifacts and logs preserved.
End hash checks under lock mark changing source stale, also outside Git inventory;
initial hashes retained so next run invalidates. Failed clean preserves old marker.

No new owner/service/cache framework, workflow authority, whole-target clean,
source touching or release/security changes. Hash excludes only .git and actual
Cargo target. Directory symlinks/special/unreadable/changing files refuse; regular
file links hash link spelling plus target content. This is package provenance,
not a sandbox for arbitrary external build-script/environment inputs. Current
five local packages have no directory symlinks. No dependency source/lock changed.
Full metadata needs available registry manifests: initial explicitly offline
profile refused uncached arbitrary1.5.0, normal configured resolution succeeded.
That setup limit is distinct from product E0560 and final passing fixtures.

## Exact final checks
Nine-step clean PASS24.317s, stale=false/start=end;
content e701443453b5f16a628cd84b34a673a6af8bd37db93a2e8abe6abc78b5e79660;
summary SHA25664db3d89c432ff3fe052503f55ee33d20e741d23fbf2c627ad90e92ee9c96496.
ALL script suite231total:229PASS/2existing macOS-native signature skips/0fail.
Local runner42 cases include real two-worktree and SAME-worktree older-mtime
public field changes: actual outputs8→18→28; unchanged reuse no cleanup; unrelated
external rlib/release hashes unchanged. Nine new negative/state controls cover
cleanup failure/prior owner/recovery, one-time missing provenance, source mutation
stale, clean-name collision, malformed hashes, directory symlink, lock contention,
unknown ownership before dependency resolution, invalid metadata before mutation.
Original real-handoff test remains byte-identical. Python3.9 grammar, five source
guards, fmt/diff PASS. No actual systemPython3.9 runtime claim. Prior a36 clean9
PASS23.462/230total and dirty positive runs retain their own source scope.
First unactivated unittest skipped two realCargo cases; final activated run ran
them. No test skip introduced. All original diagnostics retained.
Read-only actual checkout profile5packages274files8427738bytes/4.027954s includes
full metadata resolution/initial acquisition; no target claim/build/clean. This
corrects note28642's arithmetic typo294files; actual JSON274. No broad performance
claim from this one measured source profile. Only Python/scripts/docs changed.

## Integration/platform gaps
Sole merger owns actual main composition/full21/fresh review/merge. Preserve
canonical459 and precise413 recovery changes. Raw PORTALSURFER assignee/requested
individual review read back. Canonical still empty intake plan, so reviewer
request is routing, not approval of this owner patch. No independent review or
merge inferred. Pulse remainsInProgress. Historical2144/PR60 and EQ source and
negative evidence remain frozen, not relabeled current qualification.
No new native app/UI/audio acceptance needed solely for cache logic. If Root
requests source-specific Mac verification: ALL local runner42cases, configured
Cargo/Git, both real fixtures must execute; private targets only, zero-match or
unexpected skip fails. No production target cleanup/app operations/unchanged
native suites. No selfmerge/hostedCI/newworker/model override/userdata operation.
All owned jobs terminal. Immutable source/evidence refs are source-only artifacts.
