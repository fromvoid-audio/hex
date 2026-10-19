"""Deterministic local check selection; this module never builds or mutates Git."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote

POLICY_VERSION = 1
UI_PACKAGES = {"hex-ui", "hex-widgets"}
TOOLING = {
    "launch", "scripts/launch-restart.py", "scripts/hex-test-build.py",
    "scripts/hex-test-build.json", "scripts/bundle-macos.sh",
}


def git(root: Path, *args: str, env=None) -> bytes:
    return subprocess.check_output(["git", *args], cwd=root, env=env)


def metadata_path(path: str) -> bool:
    """Only prose is metadata. Assets, generated datasets and policy are inputs."""
    return (path in {"README.md", "todo.md"}
            or path.startswith("docs/") and path.endswith(".md"))


def runtime_inputs(root: Path, env=None) -> dict:
    digest = hashlib.sha256()
    paths = sorted(set(git(root, "ls-files", "-z", "--cached", "--others",
                           "--exclude-standard", env=env).decode().rstrip("\0").split("\0")) - {""})
    symlinks = [name for name in paths if (root / name).is_symlink()]
    for name in paths:
        if metadata_path(name):
            continue
        path = root / name
        mode = (0o120000 if path.is_symlink() else 0o100755 if path.is_file() and path.stat().st_mode & 0o111
                else 0o100644 if path.is_file() else 0)
        content = os.readlink(path).encode() if path.is_symlink() else (
            path.read_bytes() if path.is_file() else b"<missing>")
        for value in (name.encode(), str(mode).encode(), content):
            digest.update(len(value).to_bytes(8, "big"))
            digest.update(value)
    return {"version": POLICY_VERSION, "sha256": digest.hexdigest(),
            "symlinks": symlinks,
            "metadata_exclusions": [p for p in paths if metadata_path(p)]}


def changed_paths(root: Path, base: str, source: str) -> list[str]:
    """Both sides of renames, deletions, and untracked inputs participate."""
    paths = set()
    for arguments in ((base, source), (source,)):
        fields = git(root, "diff", "--name-status", "-z", "--find-renames", *arguments).decode().split("\0")
        index = 0
        while index < len(fields) and fields[index]:
            status = fields[index]
            count = 2 if status[0] in "RC" else 1
            paths.update(fields[index + 1:index + 1 + count])
            index += count + 1
    paths.update(git(root, "ls-files", "--others", "--exclude-standard", "-z").decode().split("\0"))
    return sorted(paths - {""})


def cargo_inventory(metadata: dict, root: Path) -> list[dict]:
    members = set(metadata["workspace_members"])
    inventory = []
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        manifest = Path(package["manifest_path"]).resolve().relative_to(root.resolve()).as_posix()
        inventory.append({
            "name": package["name"], "manifest": manifest,
            "features": package["features"],
            "targets": [{k: t[k] for k in ("name", "kind", "required-features") if k in t}
                        for t in package["targets"]],
            # Include dev, build, optional and target-dependent declarations:
            # default resolve.nodes alone omits some required configurations.
            "dependencies": [{k: d.get(k) for k in
                              ("name", "rename", "kind", "optional", "target", "features", "uses_default_features", "path")}
                             for d in package["dependencies"]],
        })
    if len(inventory) != len(members) or not inventory:
        raise ValueError("incomplete workspace Cargo inventory")
    return sorted(inventory, key=lambda item: item["name"])


def classify(paths: list[str], inventory: list[dict]) -> tuple[str, list[str], list[str]]:
    changed = set()
    scopes = set()
    reasons = []
    for path in paths:
        if metadata_path(path):
            continue
        if (path in TOOLING or path.startswith("scripts/tests/test_") and path.endswith(".py")):
            scopes.add("tooling")
            continue
        owner = next((p for p in inventory if path.startswith(str(Path(p["manifest"]).parent) + "/")), None)
        if (owner and owner["name"] in UI_PACKAGES and path.endswith(".rs")
                and not path.endswith("/build.rs")):
            scopes.add("ui")
            changed.add(owner["name"])
        else:
            scopes.add("full")
            reasons.append("high-risk or unknown input: " + path)
    affected = set(changed)
    while True:
        next_affected = affected | {p["name"] for p in inventory
                                   if any(d["name"] in affected for d in p["dependencies"])}
        if next_affected == affected:
            break
        affected = next_affected
    if scopes == {"ui"} and affected - UI_PACKAGES - {"hex-desktop"}:
        scopes.add("full")
        reasons.append("UI reverse dependency extends beyond supported UI/default-desktop configurations")
    scope = ("metadata" if not scopes else next(iter(scopes)) if len(scopes) == 1 else "full")
    if len(scopes) > 1:
        reasons.append("mixed executable scopes require full coverage")
    if not reasons:
        reasons.append("known " + scope + " inputs; prose does not widen executable scope")
    return scope, sorted(affected), reasons


def resolve(root: Path, *, profile: str, base: str, source: str, metadata: dict,
            allow_same_tree_merge: bool = False) -> dict:
    if profile not in {"edit", "merge", "full", "release"}:
        raise ValueError("unknown check profile")
    source_sha = git(root, "rev-parse", "--verify", source + "^{commit}").decode().strip()
    base_sha = git(root, "rev-parse", "--verify", base + "^{commit}").decode().strip()
    head = git(root, "rev-parse", "HEAD").decode().strip()
    if head != source_sha:
        if (not allow_same_tree_merge or git(root, "rev-parse", "HEAD^{tree}")
                != git(root, "rev-parse", source_sha + "^{tree}")):
            raise ValueError("requested source is not the current checkout")
        subprocess.run(["git", "merge-base", "--is-ancestor", source_sha, head], cwd=root, check=True)
    subprocess.run(["git", "merge-base", "--is-ancestor", base_sha, source_sha], cwd=root, check=True)
    paths = changed_paths(root, base_sha, source_sha)
    if not paths:
        raise ValueError("empty scope is not qualifying test evidence; use --profile full for an audit")
    inventory = cargo_inventory(metadata, root)
    scope, affected, reasons = classify(paths, inventory)
    if profile in {"full", "release"}:
        scope = "full"
        reasons.append("explicit full audit/release request")
    result = {"policy_version": POLICY_VERSION, "profile": profile,
              "base": base_sha, "source": source_sha,
              "tree": git(root, "rev-parse", source_sha + "^{tree}").decode().strip(),
              "paths": paths, "scope": scope, "reasons": reasons,
              "affected_packages": affected, "cargo_inventory": inventory,
              "runtime_inputs": runtime_inputs(root),
              "qualifies_merge": profile in {"merge", "full", "release"}}
    result["plan_sha256"] = hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
    return result


def reuse_checks(root: Path, plan: dict, report: Path) -> dict:
    """Reuse runtime checks only for an ordered, prose-only continuation."""
    if plan["profile"] != "merge":
        raise ValueError("runtime reuse requires an explicit merge profile")
    if plan["runtime_inputs"].get("symlinks"):
        raise ValueError("runtime continuation cannot certify external repository symlink targets")
    raw = report.read_bytes()
    evidence = json.loads(raw)
    old_plan = evidence.get("plan")
    if (evidence.get("schema_version") != 2 or evidence.get("mode") not in {"full", "merge", "release"}
            or evidence.get("status") != "passed" or evidence.get("exit_code") != 0
            or evidence.get("stale") is not False or evidence.get("dirty") is not False
            or evidence.get("start_tree") != evidence.get("end_tree")
            or not evidence.get("steps") or any(s.get("status") != "passed" for s in evidence["steps"])
            or old_plan and not old_plan.get("qualifies_merge")
            or evidence.get("runtime_inputs", {}).get("sha256") != plan["runtime_inputs"]["sha256"]):
        raise ValueError("runtime report is incomplete/failed/stale/partial or executable inputs changed")
    previous = evidence["commit"]
    subprocess.run(["git", "merge-base", "--is-ancestor", previous, plan["source"]], cwd=root, check=True)
    delta = changed_paths(root, previous, plan["source"])
    if not delta or any(not metadata_path(p) for p in delta):
        raise ValueError("reused runtime evidence requires a nonempty prose-only continuation")
    dependency = evidence.get("target_cache", {}).get("dependency_inputs")
    if not isinstance(dependency, dict):
        raise ValueError("runtime report lacks guarded toolchain/dependency identity")
    result = dict(plan)
    result["reuse"] = {"report": str(report.resolve()), "sha256": hashlib.sha256(raw).hexdigest(),
                       "source": previous, "dependency_inputs": dependency}
    result["reasons"] = [*plan["reasons"], "runtime inputs identical; metadata continuation checked separately"]
    result.pop("plan_sha256")
    result["plan_sha256"] = hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
    return result


def check_metadata(root: Path, base: str, source: str) -> list[str]:
    """Check changed prose without pretending to automate editorial claims."""
    checked = []
    for name in changed_paths(root, base, source):
        if not metadata_path(name):
            raise ValueError("metadata-only check received executable input: " + name)
        path = root / name
        if not path.exists():  # Deleted prose still participates in the plan.
            continue
        text = path.read_text(encoding="utf-8")
        if any(line.startswith(("<<<<<<< ", ">>>>>>> ")) for line in text.splitlines()):
            raise ValueError("unresolved prose conflict: " + name)
        for target in re.findall(r"\]\(<?([^\s)>]+)>?\)", text):
            if ":" in target or target.startswith(("/", "#")):
                continue
            relative = unquote(target.split("#", 1)[0])
            destination = (path.parent / relative).resolve()
            if not destination.is_relative_to(root.resolve()) or not destination.exists():
                raise ValueError(f"broken repository link in {name}: {target}")
        checked.append(name)
    subprocess.run(["git", "diff", "--check", base, source], cwd=root, check=True)
    return checked


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-metadata", action="store_true", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--source", required=True)
    args = parser.parse_args()
    print(json.dumps({"metadata_checked": check_metadata(Path.cwd(), args.base, args.source),
                      "claim_review": "editorial claims require final diff review"}, indent=2))
