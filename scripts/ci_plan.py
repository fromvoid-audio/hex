"""Deterministic local check selection; this module never builds or mutates Git."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote

_PYTHON_SPEC = importlib.util.spec_from_file_location(
    "hex_python_test_plan", Path(__file__).with_name("python_test_plan.py"))
if _PYTHON_SPEC is None or _PYTHON_SPEC.loader is None:
    raise RuntimeError("Python self-test impact policy unavailable")
python_tests = importlib.util.module_from_spec(_PYTHON_SPEC)
_PYTHON_SPEC.loader.exec_module(python_tests)

POLICY_VERSION = 3
UI_PACKAGES = {"hex-ui", "hex-widgets"}
# Opaque state reaches storage through runtime type IDs, not a Cargo dependency.
# Changes here must be reviewed with the real registration/codec consumers.
RUNTIME_CONSUMERS = {"hex-devices": {"hex-storage"}}
TOOLING = {
    "launch", "scripts/launch-restart.py", "scripts/hex-test-build.py",
    "scripts/hex-test-build.json", "scripts/bundle-macos.sh",
    "scripts/ci_plan.py", "scripts/local-ci.py", "scripts/python_test_plan.py", "scripts/ci_checkpoint.py",
}


def git(root: Path, *args: str, env=None) -> bytes:
    return subprocess.check_output(["git", *args], cwd=root, env=env)


def metadata_path(path: str) -> bool:
    """Only contributor prose is metadata; executable policy and datasets are inputs."""
    return (path in {"README.md", "AGENTS.md"}
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


def require_qualifying_inputs(inputs: dict) -> None:
    if any(not metadata_path(name) for name in inputs.get("symlinks", [])):
        raise ValueError("qualifying executable source inputs cannot be symlinks")


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
        owners = [p for p in inventory if path.startswith(str(Path(p["manifest"]).parent) + "/")]
        owner = max(owners, key=lambda p: len(p["manifest"]), default=None)
        if (owner and path.endswith(".rs") and not path.endswith("/build.rs")
                and Path(path).relative_to(Path(owner["manifest"]).parent).parts[0]
                in {"src", "tests", "examples", "benches"}):
            scopes.add("ui" if owner["name"] in UI_PACKAGES else "rust")
            changed.add(owner["name"])
        elif owner and (path == owner["manifest"] or path.endswith("/build.rs")):
            scopes.add("rust")
            changed.add(owner["name"])
            reasons.append("package build/config input: include owner and declared consumers: " + path)
        else:
            scopes.add("full")
            reasons.append("high-risk or unknown input: " + path)
    affected = set(changed)
    while True:
        next_affected = affected | {p["name"] for p in inventory
                                   if any(d["name"] in affected for d in p["dependencies"])}
        next_affected |= {consumer for owner in affected for consumer in RUNTIME_CONSUMERS.get(owner, set())
                          if any(p["name"] == consumer for p in inventory)}
        if next_affected == affected:
            break
        affected = next_affected
    if scopes == {"ui"} and affected - UI_PACKAGES - {"hex-desktop"}:
        scopes.add("rust")
        reasons.append("UI has additional consumers: include their package configurations")
    if scopes == {"ui"} and any(set(p["features"]) - ({"ui-test-support"} if p["name"] == "hex-ui" else set())
                                for p in inventory if p["name"] in UI_PACKAGES):
        scopes.add("rust")
        reasons.append("additional UI features require affected-package configurations")
    scope = ("full" if "full" in scopes else "rust" if "rust" in scopes
             or "ui" in scopes and "tooling" in scopes else
             "metadata" if not scopes else next(iter(scopes)))
    if not reasons:
        reasons.append("known " + scope + " inputs; prose does not widen executable scope")
    return scope, sorted(affected), reasons


def boundary_selection(root: Path, paths: list[str], inventory: list[dict], affected: list[str]) -> dict:
    """Narrow only reviewed runtime relationships; Cargo closure remains the lint boundary.

    Compressor state/DSP uses the unchanged Lattice host interfaces. Its real
    registry, session revision, callback/allocation, persistence and panel tests
    cover the dynamic path that Cargo edges cannot name. Other device modules,
    registration, host APIs, macros and build inputs retain package coverage.
    This is a reviewed ownership rule, not inference from diff size or signatures.
    """
    executable = [p for p in paths if not metadata_path(p)]
    names = {p["name"] for p in inventory}
    missing = {consumer for owner in affected for consumer in RUNTIME_CONSUMERS.get(owner, set())} - names
    if missing:
        raise ValueError("reviewed runtime consumers missing from Cargo inventory: " + ", ".join(sorted(missing)))
    compressor = {"crates/hex-devices/src/compressor.rs", "crates/hex-devices/src/compressor/tests.rs"}
    known_consumers = {"hex-devices", "hex-engine", "hex-audio", "hex-ui", "hex-storage",
                       "hex-desktop", "hex-cli", "hex-export", "hex-session", "hex-media", "hex-stream"}
    reviewed_features = {"hex-engine": {"instrument-test-support"}, "hex-export": {"instrument-test-support"},
                         "hex-audio": {"allocation-probe"}, "hex-desktop": {"allocation-probe"},
                         "hex-ui": {"ui-test-support"}, "hex-session": {"source-overview-test-support"},
                         "hex-stream": {"allocation-probe"}}
    narrow = bool(executable) and set(executable) <= compressor and set(affected) <= known_consumers
    narrow = narrow and all(set(p["features"]) <= reviewed_features.get(p["name"], set())
                            for p in inventory if p["name"] in affected)
    if narrow:
        # Macro exports/includes can introduce consumers not represented by the
        # reviewed runtime relationship. Never try to parse Rust with a regex.
        narrow = all((root / p).is_file() and not any(token in (root / p).read_text()
                     for token in ("macro_rules!", "macro_export", "include!", "include_str!", "include_bytes!", "#[path"))
                     for p in compressor)
    tests = []
    if narrow:
        tests = [
            {"package": "hex-devices", "lib": True, "filter": "compressor::"},
            {"package": "hex-engine", "lib": True, "filter": "compressor"},
            {"package": "hex-engine", "test": "device_chain", "filter": ""},
            {"package": "hex-storage", "lib": True, "filter": "compressor"},
            {"package": "hex-audio", "lib": True, "filter": "compressor", "features": "allocation-probe"},
            {"package": "hex-audio", "test": "device_comparison", "filter": "", "features": "allocation-probe"},
            {"package": "hex-audio", "test": "allocation_probe_calibration", "filter": "", "features": "allocation-probe"},
            {"package": "hex-ui", "lib": True, "filter": "compressor", "features": "ui-test-support"},
        ]
    return {"kind": "compressor" if narrow else "packages", "tests": tests,
            "test_packages": [] if narrow else affected,
            "lint_packages": affected,
            "excluded_behavior_packages": sorted(set(affected) - {t["package"] for t in tests}) if narrow else [],
            "excluded_configurations": (["unrelated instrument/export and whole UI/audio suites: compressor-only implementation; unchanged shared host/registration/build inputs"] if narrow else []),
            "excluded_packages": sorted({p["name"] for p in inventory} - set(affected)),
            "rationale": ("unchanged host interfaces/registration; real compressor host, history, allocation, save/reopen and UI consumers; other device behavior omitted; all Cargo consumers linted"
                          if narrow else "owner plus transitive normal/dev/build/optional/target and reviewed runtime consumers; full package tests and relevant feature configurations; no module isolation assumed")}


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
    inputs = runtime_inputs(root)
    if profile in {"merge", "full", "release"}:
        require_qualifying_inputs(inputs)
    scope, affected, reasons = classify(paths, inventory)
    if profile in {"full", "release"}:
        scope = "full"
        reasons.append("explicit full audit/release request")
    result = {"policy_version": POLICY_VERSION, "profile": profile,
              "base": base_sha, "source": source_sha,
              "tree": git(root, "rev-parse", source_sha + "^{tree}").decode().strip(),
              "paths": paths, "scope": scope, "reasons": reasons,
              "affected_packages": affected, "cargo_inventory": inventory,
              "runtime_inputs": inputs,
              "python_tests": python_tests.select(
                  profile, [path for path in paths if not metadata_path(path)],
                  inventory, UI_PACKAGES, python_tests.available_modules(root)),
              "qualifies_merge": profile in {"merge", "full", "release"}}
    if scope == "rust":
        result["boundary"] = boundary_selection(root, paths, inventory, affected)
    result["plan_sha256"] = hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
    return result


def reuse_checks(root: Path, plan: dict, report: Path, env=None) -> dict:
    """Reuse runtime checks only for an ordered, prose-only continuation."""
    if plan["profile"] != "merge":
        raise ValueError("runtime reuse requires an explicit merge profile")
    if python_tests.native_acceptance_inputs(os.environ if env is None else env):
        raise ValueError("native/external acceptance inputs require fresh checks; runtime reuse refused")
    if plan["runtime_inputs"].get("symlinks"):
        raise ValueError("runtime continuation cannot certify external repository symlink targets")
    raw = report.read_bytes()
    evidence = json.loads(raw)
    if evidence.get("native_acceptance_opt_in") is not False:
        raise ValueError("native/external acceptance opt-in is present or unknown; runtime reuse refused")
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
                       "source": previous, "dependency_inputs": dependency,
                       "native_acceptance_opt_in": False}
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
