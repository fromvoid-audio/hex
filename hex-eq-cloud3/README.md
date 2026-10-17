# Hex EQ cloud3 — immutable evidence only

All six original canonical source PRs342/343/347/344/348/349 are merged. This orphan
Git commit transports original receipts/logs and one finite later-cohort native
request; it is not source, a source PR, an app, a deployment or native approval.
Do not merge or check out this evidence tree over a source/user worktree.

Read NATIVE-REQUEST.md and SOURCE-SEQUENCE.json. All77 available original CI
receipts (including failed/interrupted/scoped runs) and their logs are retained
unchanged under receipts/<scope>/<run>. The manifest lists every original path,
SHA256, byte count and report status; partial/failed runs remain nonqualifying.
The historical worker profile retains its original scope and outlier limits.

PR349 head b63424fde3e88915f747f8223bb2c9ea183403c3 has its own all20 green
receipt. Actual merge92ecd8d2137c5d270d5d0c77a11dad58b50daf15 has a different
complete tree; its already-running pinned verification separately passed all20.
These independent receipts must never be relabeled as one another or as a later
cohort. MERGE-PROOF.json records actual parents/tree/recomputed tree/ancestry.

Gesture2729 is already user accepted; preserve that feature signoff. No claim of
human acceptance of the current merge or the frozen earlier Mac app is made.
Native acceptance of remaining scopes and future combined delivery qualification
belong to the coordinator. Leave the closed frozen dfa21f5a app untouched.

Fetch the unique evidence tag from the handoff, verify its commit against the
supplied full immutable Git OID, then use git archive to extract to a NEW task-local
evidence directory. Read the files there; source stays at the chosen qualified
commit. For example, after fetching the tag and verifying its OID:

    git archive <verified-evidence-commit> hex-eq-cloud3 | tar -x -C <new-evidence-dir>

No hosted CI, duplicate source PR or Mac job is created by this transport.
