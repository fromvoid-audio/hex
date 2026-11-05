# engineering continuation

This compact workspace implements the user's 5 November 2026 continuation request.
Start with [the mission](goal.md), [checkpoint](state.json), [ranked index](backlog.md)
and [health snapshot](health.md). Keep facts and limitations timestamped; a snapshot
does not establish current health. [Measurements](measurements.jsonl) retain exact
commands, inputs, platform, elapsed wall time and log locations. Commit useful
outcomes through focused PRs; retain raw logs under the existing ignored `artifacts/`.
Those local logs need separate artifact retention to survive executor replacement;
canonical PRs and Pulse notes must contain the essential results and provenance.

## owners and existing tools

Read [AGENTS.md](../AGENTS.md), [README.md](../README.md),
[development workflow](../docs/development-workflow.md),
[local checks](../docs/local-ci.md) and [cloud setup](../docs/cloud-environment.md).
Architecture is owned by crate entry-point and module doc comments: `hex-project`
validates edits/history, `hex-session` coordinates prepared state, `hex-engine`
renders it, `hex-audio` owns callback handoff, `hex-storage` persists projects,
and `hex-ui` presents typed commands and queries. Follow actual contracts and
consumers before changing any owner. Do not copy those contracts here.

Use the existing profiling examples under `crates/*/examples/` and recorded
workloads under `research/`; select a relevant one only after identifying a real
performance problem. Timing a guard or build is not an application benchmark.
The local CI runner already retains machine-readable summaries and numbered logs
under `artifacts/local-ci/`. Use them for engineering health; no new dashboard,
test runner, database or dependency is needed.

## activation and continuation

The user supplies this mission manually through `/goal`. A supplied goal instruction
is also followed as task scope; native runtime activation requires runtime evidence.
See the checkpoint for the observed status. To activate it in a supporting Codex
interface, submit this instruction (or paste the complete file after `/goal`):

```text
/goal Read .engineering/goal.md and adopt its complete Continuous Engineering Improvement mission, including every core responsibility, operating-loop step, evidence requirement, autonomy constraint, and Hex execution/recovery rule. Begin by reading .engineering/README.md and .engineering/state.json, reconcile them with live Pulse and current Git/PR state, and continue one valuable verified improvement at a time. Preserve the full goal across sessions. Do not create scheduled tasks.
```

Repository files alone cannot activate or verify a native goal. The runtime has
now supplied an automatic goal continuation; the checkpoint records that evidence. No scheduler is configured or required by this workspace; goal
activation alone does not guarantee unattended execution or recovery after a
runtime stops. Resume the goal in an available authorized Codex runtime when needed.

## safe recovery protocol

1. Inspect `git status --short`, `git worktree list`, the checkpoint's live Pulse
   issue/PR and exact published head. Claims are advisory; do not overwrite WIP.
   If the prior PR merged, reconcile its actual merge and choose new work instead
   of replaying it. If an issue is blocked, read its reason before acting.
2. Fetch upstream. Use a dedicated branch/worktree based on current upstream or
   the inspected existing task branch. Never reset another checkout, force-push,
   delete someone else's target or retire an active branch. Avoid overlapping file
   ownership; choose independent work when another agent is editing the owner.
3. Audit and prioritize from evidence. Refresh/deduplicate the small backlog index
   against live Pulse each cycle; retire obsolete references with a reason. Keep
   only a few actionable leads. Do not resume explicitly paused work by accident.
4. Complete one bounded improvement using existing owner tests and tools. Serialize
   Cargo-heavy checks per executor; use the repository runner's target locks and
   custody rules. Refresh the upstream base before integration and revalidate
   changed inputs/conflicts. Follow AGENTS.md's PR workflow and check policy.
5. Record exact source/base, commands, results, measured benefit, omissions, blocker
   and one next action in the canonical PR/Pulse note; update the checkpoint pointer
   and health snapshot at coherent milestones. Preserve failed experiments too.
   Old evidence qualifies only unchanged relevant inputs. Interrupted, zero-match,
   failed or stale checks never count as passed.

If runs overlap on different executors, a local filesystem lock cannot coordinate
them. Inspect the same canonical Pulse issue/PR before editing and stop or choose
another owner on a conflicting published head; do not treat a Pulse claim as an
exclusive distributed lock. Leave uncertain private work untouched.
