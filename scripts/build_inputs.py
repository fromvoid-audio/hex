"""Content identities for guarded Cargo cache reuse, never artifact certificates."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time


# Pinned cc1.6 target_envs accepts plain, raw/normalized target suffixes,
# and HOST_/TARGET_ spellings. Keep native build flags in the same certificate.
NATIVE_INPUTS = ("CC", "CXX", "AR", "RANLIB", "NVCC", "CFLAGS", "CPPFLAGS", "CXXFLAGS",
                 "ARFLAGS", "RANLIBFLAGS", "LDFLAGS", "CXXSTDLIB", "CXXSTDLIB_STATIC")
NATIVE_CONTROLS = {"CRATE_CC_NO_DEFAULTS", "CROSS_COMPILE", "WASM_MUSL_SYSROOT", "WASI_SYSROOT",
                   "PAUTHTEST_SYSROOT", "PAUTHTEST_RESOURCE_DIR",
                   "SYSROOT", "CLIPPY_ARGS"}


def semantic_environment_key(name: str) -> bool:
    if name == "CARGO_TARGET_DIR":
        return False
    native = name[5:] if name.startswith("HOST_") else name[7:] if name.startswith("TARGET_") else name
    return (name.startswith(("CARGO_", "RUST", "CC", "CXX", "AR", "LD", "PKG_CONFIG"))
            or name in {"PATH", "HOST", "TARGET", "SDKROOT", "DEVELOPER_DIR", "MACOSX_DEPLOYMENT_TARGET"}
            or name in NATIVE_CONTROLS
            or name in {"HEX_RELEASE_LABEL", "HEX_RELEASE_CHANGELOG"}
            or any(native == key or native.startswith(key + "_") for key in NATIVE_INPUTS))


def fingerprint(root: Path, env: dict, metadata: dict, tools: dict, *, paths=None, compiler_only=False) -> str:
    digest = hashlib.sha256()

    def field(value):
        raw = value if isinstance(value, bytes) else str(value).encode()
        digest.update(len(raw).to_bytes(8, "big"))
        digest.update(raw)

    def observe(path: Path, ancestors=()):
        field(path)
        try:
            info = path.lstat()
        except FileNotFoundError:
            field("missing")
            return
        field(info.st_mode)
        if path.is_symlink():
            field(os.readlink(path))
            target = path.resolve(strict=True)
            if target in ancestors:
                raise ValueError("dependency symlink cycle")
            observe(target, (*ancestors, target))
        elif stat.S_ISREG(info.st_mode):
            field(path.read_bytes())
        elif stat.S_ISDIR(info.st_mode):
            # Presence of an empty directory and every directory-symlink target
            # participates. Sorting also detects membership additions/removals.
            for child in sorted(path.iterdir()):
                if child.name != ".git" and child.resolve() != Path(metadata["target_directory"]).resolve():
                    observe(child, ancestors)
        else:
            raise ValueError("unsupported dependency input: " + str(path))

    field("hex-build-inputs-v1")
    if paths is not None:
        for path in sorted(paths):
            observe(path)
        return digest.hexdigest()
    field(json.dumps(tools, sort_keys=True))
    # Cargo may execute an override rather than the default PATH compiler.
    # Bind its actual bytes/mode/resolved path, not merely the env spelling.
    search = os.pathsep.join(str((root / p).resolve()) if not Path(p).is_absolute() else p
                             for p in env.get("PATH", os.defpath).split(os.pathsep))
    for name in ("RUSTC", "CARGO_BUILD_RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                 "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
                 "RUSTDOC", "CARGO_BUILD_RUSTDOC"):
        selected = env.get(name)
        if not selected:
            continue
        if os.sep in selected:
            path = Path(selected)
            if not path.is_absolute():
                path = root / path
        else:
            resolved = shutil.which(selected, path=search)
            if resolved is None:
                raise ValueError("selected Cargo executable is unavailable: " + name)
            path = Path(resolved)
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError("selected Cargo executable is not executable: " + name)
        field(name)
        observe(path)
    if compiler_only:
        return digest.hexdigest()
    field(json.dumps({k: v for k, v in env.items() if semantic_environment_key(k)}, sort_keys=True))
    if env.get("HEX_RELEASE_CHANGELOG"):
        # External frozen release notes are build inputs, even with unchanged
        # path/mtime. Full CI and guarded optimized delivery must agree on bytes.
        observe(Path(env["HEX_RELEASE_CHANGELOG"]))
    members = set(metadata["workspace_members"])
    workspace_roots = {Path(p["manifest_path"]).parent.resolve() for p in metadata["packages"] if p["id"] in members}
    declared_paths = {Path(d["path"]).resolve() for p in metadata["packages"] for d in p.get("dependencies", []) if d.get("path")}
    for path in sorted(declared_paths - workspace_roots):
        observe(path)
    for package in sorted(metadata["packages"], key=lambda p: p["id"]):
        manifest = Path(package["manifest_path"])
        if package.get("source") is None and package["id"] not in members:
            observe(manifest.parent)
        elif package["id"] in members:
            observe(manifest)
            # Build scripts may read helper files; include complete owning
            # build-support directories while ordinary crate src stays incremental.
            observe(manifest.parent / "build.rs")
            observe(manifest.parent / "build_support")
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml",
                 "scripts/local-ci.py", "scripts/build_inputs.py", "scripts/ci_plan.py",
                 "scripts/ci_target_storage.py"):
        observe(root / name)
    config_dirs = {p / ".cargo" for p in root.resolve().parents} | {root / ".cargo"}
    # Build observers run with a member crate as cwd. Include every observed
    # ancestor, even when its configuration paths are currently absent.
    for directory in workspace_roots:
        config_dirs.update(p / ".cargo" for p in (directory, *directory.parents))
    cargo_home = env.get("CARGO_HOME") or str(Path(env.get("HOME", "~")) / ".cargo")
    home = Path(cargo_home)
    config_dirs.add(home if home.is_absolute() else root / home)
    for directory in sorted(config_dirs):
        for name in ("config", "config.toml"):
            config = directory / name
            if config.is_file():
                # Without a full Cargo TOML interpreter, do not claim selected
                # compiler identity from arbitrary quoted/escaped config keys.
                # Explicit executable env overrides above are content-bound.
                text = config.read_text().lower()
                if any(token in text for token in ("rustc", "rustdoc", "\\u")):
                    raise ValueError("compiler/wrapper selection in Cargo config is unsupported; use explicit executable environment")
            observe(config)
    return digest.hexdigest()


def workspace_snapshot(root: Path, env: dict, metadata: dict) -> dict:
    """Compact package content witnesses; timestamps are freshness hints, never identities."""
    members = set(metadata["workspace_members"])
    packages = []
    for package in metadata["packages"]:
        if package["id"] in members:
            directory = Path(package["manifest_path"]).parent.resolve().relative_to(root.resolve())
            packages.append({"spec": package["id"], "path": directory.as_posix(), "sha256": hashlib.sha256(),
                             "timestamps": hashlib.sha256()})
    newest_input_ns = 0
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root, env=env)
    for raw in sorted(set(paths.split(b"\0")) - {b""}):
        name = os.fsdecode(raw)
        matches = [p for p in packages if p["path"] == "." or name.startswith(p["path"] + "/")]
        if not matches:
            continue
        package = max(matches, key=lambda p: 0 if p["path"] == "." else len(p["path"]))
        path = root / name
        try:
            info = path.lstat()
        except FileNotFoundError:
            content, mode, stamp = b"<missing>", 0, 0
        else:
            if not stat.S_ISREG(info.st_mode):
                raise ValueError("workspace source input is not a regular file: " + name)
            content, mode, stamp = path.read_bytes(), info.st_mode, info.st_mtime_ns
            newest_input_ns = max(newest_input_ns, stamp)
        for value in (raw, str(mode).encode(), content):
            package["sha256"].update(len(value).to_bytes(8, "big") + value)
        package["timestamps"].update(raw + b"\0" + str(stamp).encode() + b"\0")
    for package in packages:
        package["sha256"] = package["sha256"].hexdigest()
        package["timestamps"] = package["timestamps"].hexdigest()
    return {"packages": sorted(packages, key=lambda p: p["spec"]), "observed_ns": max(time.time_ns(), newest_input_ns)}


def stale_workspace_packages(root: Path, env: dict, previous: dict | None, current: dict) -> list[str]:
    """Only trust normal edits newer than the last completed build observation.

    Git supplies changed paths from the immutable previous clean commit. Missing,
    deleted, backdated or otherwise unproven edits require package invalidation.
    No per-file inventory is persisted in the bounded release-owner document.
    """
    if not previous or not previous.get("checked"):
        return [p["spec"] for p in current["packages"]]
    old = {p["spec"]: p for p in previous["packages"]}
    changed = [p for p in current["packages"] if old.get(p["spec"], {}).get("sha256") != p["sha256"]]
    if not changed:
        return []
    paths = None
    if previous.get("dirty") is False:
        result = subprocess.run(["git", "diff", "--no-renames", "--name-only", "-z", previous["commit"], "--"],
                                cwd=root, env=env, capture_output=True)
        if result.returncode == 0:
            untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=root, env=env)
            paths = set(os.fsdecode(p) for p in (result.stdout + untracked).split(b"\0") if p)
    stale = []
    for package in changed:
        owned = [name for name in (paths or ()) if package["path"] == "." or name.startswith(package["path"] + "/")]
        fresh = bool(owned) and package["timestamps"] != old.get(package["spec"], {}).get("timestamps")
        for name in owned:
            path = root / name
            fresh = fresh and path.is_file() and not path.is_symlink() and path.stat().st_mtime_ns > previous["observed_ns"]
        if not fresh:
            stale.append(package["spec"])
    return stale
