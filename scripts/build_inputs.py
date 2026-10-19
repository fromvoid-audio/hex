"""Content identities for guarded Cargo cache reuse, never artifact certificates."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat


def fingerprint(root: Path, env: dict, metadata: dict, tools: dict, *, paths=None) -> str:
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
    field(json.dumps({k: v for k, v in env.items() if
                     k.startswith(("CARGO_", "RUST", "CC", "CXX", "AR", "LD", "PKG_CONFIG"))
                     or k in {"PATH", "HOST", "TARGET", "SDKROOT", "DEVELOPER_DIR", "MACOSX_DEPLOYMENT_TARGET"}}, sort_keys=True))
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
                 "scripts/local-ci.py", "scripts/build_inputs.py", "scripts/ci_plan.py"):
        observe(root / name)
    config_dirs = {p / ".cargo" for p in root.resolve().parents} | {root / ".cargo"}
    # Build observers run with a member crate as cwd. Include every observed
    # ancestor, even when its configuration paths are currently absent.
    for directory in workspace_roots:
        config_dirs.update(p / ".cargo" for p in (directory, *directory.parents))
    cargo_home = env.get("CARGO_HOME") or str(Path(env.get("HOME", "~")) / ".cargo")
    config_dirs.add(Path(cargo_home))
    for directory in sorted(config_dirs):
        for name in ("config", "config.toml"):
            observe(directory / name)
    return digest.hexdigest()
