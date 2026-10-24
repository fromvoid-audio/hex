#!/usr/bin/env python3
"""Local immutable macOS releases, sharing Hex's guarded build and install owners.

This module owns channel/version reservation, frozen release inputs and distribution
archives. release_notes owns recoverable change classification; local-ci owns check
and Cargo custody; hex-test-build owns native identity and same-path installation.
No updater, scheduler, Git version mutation or remote publication is performed.

Candidate directories are a durable state machine: plan -> full CI -> optimized
bundle -> notarization -> verified immutable archive. A failed attempt retains its
inputs and diagnostics and can only resume the same plan. Successful versions and
build numbers are never reassigned. Channel ordering uses numeric identifiers,
never lexical comparison between different channels.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    if spec is None or spec.loader is None:
        raise RuntimeError("release dependency unavailable")
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    spec.loader.exec_module(value)
    return value


delivery = module("hex_release_delivery", "hex-test-build.py")
ci = delivery.ci
notes = module("hex_release_notes", "release_notes.py")
manual = module("hex_release_manual", "manual.py")
Refusal = delivery.DeliveryError
BASE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")


def encode(value):
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def read(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        raise Refusal(f"expected bounded regular release metadata: {path}")
    return json.loads(path.read_bytes())


def immutable(path, raw):
    """Publish create-only bytes; identical retries preserve the original file."""
    if path.exists() or path.is_symlink():
        if path.is_symlink() or path.read_bytes() != raw:
            raise Refusal(f"immutable release input differs: {path}")
        return
    # Link the complete fsynced inode so interruption cannot expose partial JSON.
    descriptor, name = tempfile.mkstemp(prefix=".release-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.link(name, path)
        delivery.sync_directory(path.parent)
    finally:
        Path(name).unlink(missing_ok=True)


@contextmanager
def locked(directory):
    delivery.assert_plain_path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(directory / ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "rb") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise Refusal(f"release operation active: {directory}") from error
        yield


def git(*args):
    return delivery.run(["git", *args], ROOT)


def store_path():
    common = Path(git("rev-parse", "--git-common-dir"))
    if not common.is_absolute():
        common = ROOT / common
    return common.resolve().parent / "artifacts" / "releases"


def source():
    if git("status", "--porcelain", "--untracked-files=normal"):
        raise Refusal("commit the clean candidate before cutting a release")
    return git("rev-parse", "HEAD")


def ready_releases(store):
    result = []
    for path in sorted((store / "candidates").glob("*/release.json")):
        value = read(path)
        if value.get("schema") != 1:
            raise Refusal("unsupported historical release manifest")
        result.append((path, value))
    return result


def publish_channel(path):
    """Advance the local channel index last, without overwriting a newer build."""
    manifest = read(path)
    if (manifest.get("schema") != 1 or manifest.get("status") != "ready"
            or manifest.get("channel") not in {"beta", "nightly", "stable"}
            or type(manifest.get("build")) is not int or manifest["build"] < 1):
        raise Refusal("only a completed release can enter the channel catalog")
    store = path.parent.parent.parent
    with locked(store):
        channels = store / "channels"
        channels.mkdir(exist_ok=True)
        index = channels / (manifest["channel"] + ".json")
        current = read(index) if index.exists() else None
        entry = {"schema": 1, "channel": manifest["channel"], "version": manifest["version"],
                 "build": manifest["build"], "source": manifest["source"],
                 "manifest": str(path), "manifest_sha256": delivery.sha(path),
                 "archive_sha256": manifest["archive_sha256"]}
        if current and current["build"] >= entry["build"]:
            if current["build"] == entry["build"] and current != entry:
                raise Refusal("channel catalog conflicts with immutable release")
            return
        delivery.write_json(index, entry)
        delivery.sync_directory(channels)


def latest_stable(store, through):
    eligible = []
    for path, value in ready_releases(store):
        if value["channel"] == "stable":
            result = subprocess.run(["git", "merge-base", "--is-ancestor", value["source"], through], cwd=ROOT)
            if result.returncode == 0:
                eligible.append((path, value))
            elif result.returncode != 1:
                raise Refusal("cannot resolve stable release ancestry")
    return max(eligible, key=lambda item: item[1]["build"], default=None)


def next_version(channel, base, versions, date):
    if not BASE_VERSION.fullmatch(base):
        raise Refusal("target version must be an explicit major.minor.patch without prerelease metadata")
    if channel == "stable":
        label = base
    else:
        prefix = base + ("-beta." if channel == "beta" else f"-nightly.{date}.")
        sequence = [int(v[len(prefix):]) for v in versions if v.startswith(prefix) and v[len(prefix):].isdigit()]
        label = prefix + str(max(sequence, default=0) + 1)
    if label in versions:
        raise Refusal(f"version already reserved: {label}; resume its candidate or choose a new version")
    return label


def recover_reservations(store):
    """Complete a create-only cut interrupted after its full plan was frozen.

    Caller holds the store lock. Orphaned complete plans reserve their original
    label before a new cut can allocate it. Conflicting existing bytes refuse.
    Partial candidates without a plan only consume their original build number.
    """
    versions = store / "versions"
    versions.mkdir(parents=True, exist_ok=True)
    for path in sorted((store / "candidates").glob("*/plan.json")):
        plan = read(path)
        if (plan.get("schema") != 1 or not isinstance(plan.get("version"), str)
                or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-beta\.[0-9]+|-nightly\.[0-9]{8}\.[0-9]+)?", plan["version"]) is None):
            raise Refusal("invalid orphaned release plan")
        immutable(versions / (plan["version"] + ".json"),
                  encode({"candidate": str(path.parent), "plan_sha256": delivery.sha(path)}))


def markdown(snapshot, version):
    lines = [f"# hex {version}", "", f"source: {snapshot['source']}", ""]
    entries = [entry for batch in snapshot["batches"] for entry in batch["result"]["entries"]]
    for category in ("features", "improvements", "fixes"):
        selected = [entry["text"] for entry in entries if entry["category"] == category]
        if selected:
            lines += [f"## {category}", "", *["- " + text for text in selected], ""]
    if not entries:
        lines += ["no user-facing changes in this release.", ""]
    return "\n".join(lines).encode()


def prepare_notes(store, through, *, base=None, max_changes=512, timeout=300):
    """Retry only typed inference timeouts; retain each failed attempt's evidence."""
    for attempt in range(3):
        try:
            return notes.prepare(ROOT, store, through, base=base, max_changes=max_changes, timeout=timeout)
        except notes.Refusal as error:
            cause = error
            timed_out = False
            for _ in range(8):
                if isinstance(cause, subprocess.TimeoutExpired):
                    timed_out = True
                    break
                cause = cause.__cause__
                if cause is None:
                    break
            if not timed_out or attempt == 2:
                raise
            print(f"notes inference timed out; completed work retained; retry {attempt + 2}/3", file=sys.stderr, flush=True)


def cut(args, store):
    through = source()
    version = args.version or delivery.changelog.workspace_version((ROOT / "Cargo.toml").read_text()).split("-")[0]
    if not BASE_VERSION.fullmatch(version):
        raise Refusal("invalid target version")
    stable = latest_stable(store, through)
    if stable and stable[1]["target"] == version:
        if args.version:
            raise Refusal("target version is already stable; select the next target version")
        major, minor, patch = version.split(".")
        version = f"{major}.{minor}.{int(patch) + 1}"
    base = args.base or (stable[1]["source"] if stable else None)
    if args.channel == "stable" and args.accept_source != through:
        raise Refusal(f"stable requires native acceptance of this exact source; use --accept-source {through}")
    if args.channel == "nightly":
        prior = [v for _, v in ready_releases(store) if v["channel"] == "nightly"]
        if prior and max(prior, key=lambda v: v["build"])["source"] == through:
            raise Refusal("nightly source unchanged; previous immutable snapshot remains current")
    # Generation precedes version reservation and all build/check inputs. Ordinary
    # merges never call this; failure retains the separate recoverable ledger.
    prepare_notes(store / "unreleased", through, base=base, max_changes=args.max_changes,
                  timeout=args.generation_timeout)
    snapshot = notes.snapshot(ROOT, store / "unreleased", through, base=base)
    with locked(store):
        candidates = store / "candidates"
        candidates.mkdir(exist_ok=True)
        versions = store / "versions"
        versions.mkdir(exist_ok=True)
        recover_reservations(store)
        label = next_version(args.channel, version, {p.stem for p in versions.glob("*.json")},
                             datetime.now(timezone.utc).strftime("%Y%m%d"))
        numbers = [int(p.name) for p in candidates.iterdir() if p.is_dir() and p.name.isdigit()]
        number = max(numbers, default=0) + 1
        candidate = candidates / str(number)
        candidate.mkdir()
        # One projection is embedded; full immutable provenance stays beside it.
        projection = {"format": 1, "batches": snapshot["batches"]}
        raw_notes = encode(projection)
        if len(raw_notes) > 1024 * 1024:
            raise Refusal("release notes exceed embedded 1 MiB bound; consolidate before release")
        checklist = args.checklist.read_bytes() if args.checklist else (
            "# native beta checks\n\n"
            "- confirm the displayed release label and source match this package.\n"
            "- resize the window; confirm the version, log and changelog controls remain reachable.\n"
            "- open a copy of an existing project; confirm tracks, devices and media.\n"
            "- play, stop, seek and loop; check audible output and responsiveness.\n"
            "- save as a new project, quit normally and reopen that copy.\n"
            "- open the changelog and compare it with RELEASE-NOTES.md.\n\n"
            "native interaction, listening and physical audio deadlines remain unverified.\n"
            "this package qualifies macOS arm64 only; windows remains unverified.\n").encode()
        if not checklist.strip():
            raise Refusal("finite acceptance checklist is required")
        included = read(args.included) if args.included else [
            {"pr": "https://github.com/PORTALSURFER/hex/pull/" + match[1], "head": git("rev-parse", commit + "^2")}
            for line in git("log", "--first-parent", "--format=%H %s", snapshot["base"] + ".." + through).splitlines()
            for commit in [line.split(" ", 1)[0]]
            for match in [re.search(r"Merge pull request #(\d+)", line)] if match]
        if not isinstance(included, list) or any(
                not isinstance(item, dict)
                or re.fullmatch(r"https://github.com/PORTALSURFER/hex/pull/[1-9][0-9]*", str(item.get("pr"))) is None
                or re.fullmatch(r"[0-9a-f]{40}", str(item.get("head"))) is None for item in included):
            raise Refusal("included revisions must be a list of exact PR URLs and full heads")
        signing = delivery.signing_configuration(read(ROOT / "scripts/hex-test-build.json"))
        profile = os.environ.get("HEX_NOTARY_PROFILE", read(ROOT / "scripts/hex-release.json")["notary_profile"])
        manual.build(ROOT, candidate / "manual", label, through)
        plan = {"schema": 1, "channel": args.channel, "version": label, "target": version,
                "build": number, "source": through, "tree": git("rev-parse", "HEAD^{tree}"),
                "created_at": ci.utc_now(), "platform": "macos-arm64", "identity": delivery.IDENTITY,
                "signing": signing, "notary_profile": profile,
                "notes_sha256": delivery.changelog.digest(raw_notes), "included": included,
                "base": snapshot["base"], "accepted_source": args.accept_source,
                "snapshot_sha256": delivery.changelog.digest(encode(snapshot)),
                "documentation_sha256": delivery.sha(candidate / "manual/documentation.json"),
                "checklist_sha256": delivery.changelog.digest(checklist)}
        for name, raw in (("notes.json", raw_notes), ("snapshot.json", encode(snapshot)),
                          ("RELEASE-NOTES.md", markdown(snapshot, label)), ("TESTING.md", checklist),
                          ("plan.json", encode(plan))):
            immutable(candidate / name, raw)
        immutable(versions / (label + ".json"), encode({"candidate": str(candidate), "plan_sha256": delivery.sha(candidate / "plan.json")}))
    return candidate


def validate_plan(candidate):
    plan = read(candidate / "plan.json")
    if plan.get("schema") != 1 or plan.get("platform") != "macos-arm64":
        raise Refusal("unsupported release plan")
    if (not isinstance(plan.get("version"), str)
            or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-beta\.[0-9]+|-nightly\.[0-9]{8}\.[0-9]+)?", plan["version"]) is None):
        raise Refusal("invalid frozen release version")
    if plan["source"] != source() or plan["tree"] != git("rev-parse", "HEAD^{tree}"):
        raise Refusal("release source changed; restore this exact clean candidate to resume")
    for name, key in (("notes.json", "notes_sha256"), ("snapshot.json", "snapshot_sha256"),
                      ("TESTING.md", "checklist_sha256"), ("manual/documentation.json", "documentation_sha256")):
        if delivery.sha(candidate / name) != plan[key]:
            raise Refusal("frozen release inputs changed: " + name)
    manual.verify(candidate / "manual", plan["version"], plan["source"])
    snap = read(candidate / "snapshot.json")
    if (candidate / "RELEASE-NOTES.md").read_bytes() != markdown(snap, plan["version"]):
        raise Refusal("frozen release prose changed")
    if plan["signing"] != delivery.signing_configuration(read(ROOT / "scripts/hex-test-build.json")):
        raise Refusal("release signing configuration changed")
    store = candidate.parent.parent
    reservation_path = store / "versions" / (plan["version"] + ".json")
    if not reservation_path.exists():
        with locked(store):
            recover_reservations(store)
    reservation = read(reservation_path)
    if reservation != {"candidate": str(candidate), "plan_sha256": delivery.sha(candidate / "plan.json")}:
        raise Refusal("frozen release plan differs from its version reservation")
    return plan


def release_environment(candidate, plan):
    env = {**os.environ, "HEX_RELEASE_LABEL": plan["version"],
           "HEX_RELEASE_CHANGELOG": str(candidate / "notes.json")}
    # Keep debug assertions and every test; optional symbol files account for
    # substantial disk use. The shipped application still uses --release.
    env.setdefault("CARGO_PROFILE_DEV_DEBUG", "0")
    env.setdefault("CARGO_PROFILE_TEST_DEBUG", "0")
    return env


def command(argv, log):
    """Retain diagnostics without dumping successful tool output or credentials."""
    result = subprocess.run(argv, cwd=ROOT, capture_output=True)
    log.write_bytes(result.stdout + b"\n" + result.stderr)
    if result.returncode:
        raise Refusal(f"{argv[0]} failed (exit {result.returncode}); see {log}")
    return result.stdout


def preflight(candidate, plan):
    if sys.platform != "darwin" or os.uname().machine != "arm64":
        raise Refusal("distribution packaging currently supports native macOS arm64")
    command(["xcrun", "notarytool", "history", "--keychain-profile", plan["notary_profile"],
             "--output-format", "json"], candidate / "notary-preflight.log")
    identities = delivery.run(["security", "find-identity", "-v", "-p", "codesigning"])
    if plan["signing"]["certificate_sha1"] not in identities:
        raise Refusal("configured Developer ID identity is unavailable")


def check_bundle(app, plan):
    receipt = delivery.verify_distribution(app)
    fields = ("source", "version", "build", "channel", "target", "tree", "notes_sha256", "snapshot_sha256",
              "checklist_sha256", "documentation_sha256", "included", "base", "accepted_source", "identity", "signing", "created_at", "platform")
    if any(receipt.get(key) != plan[key] for key in fields):
        raise Refusal("distribution app differs from frozen release plan")
    if "ci_report_sha256" in plan and receipt["ci_report_sha256"] != plan["ci_report_sha256"]:
        raise Refusal("distribution CI evidence differs from release manifest")
    architectures = delivery.run(["lipo", "-archs", str(app / "Contents/MacOS/hex")]).split()
    if architectures != ["arm64"]:
        raise Refusal("release executable must contain exactly arm64")


def check_payload(directory, plan):
    app = directory / "archive-payload/hex.app"
    check_bundle(app, plan)
    for name in ("RELEASE-NOTES.md", "TESTING.md"):
        if (directory / "archive-payload" / name).read_bytes() != (app / "Contents/Resources" / name).read_bytes():
            raise Refusal("archive companion notes/checklist differ from sealed app")
    for name in ("hex-manual.html", "hex-manual.pdf", "documentation.json"):
        if (directory / "archive-payload" / name).read_bytes() != (app / "Contents/Resources/manual" / name).read_bytes():
            raise Refusal("archive manual differs from sealed app")
    return app


def discard_generated_stage(candidate, name):
    """Recover only this candidate's fixed reproducible staging directories."""
    if name not in {"bundle.pending.app", "archive-payload"}:
        raise Refusal("unknown release stage")
    path = candidate / name
    delivery.assert_plain_path(path)
    if path.exists():
        if not path.is_dir():
            raise Refusal("release stage is not a directory")
        shutil.rmtree(path)


def notarize(candidate, app, plan):
    check_bundle(app, plan)
    identifier = candidate / "submission.json"
    if not identifier.exists():
        upload = candidate / "notary-upload.pending.zip"
        if upload.exists():
            upload.unlink()
        command(["ditto", "-c", "-k", "--keepParent", str(app), str(upload)], candidate / "notary-archive.log")
        # Persist submission ID before waiting; a timeout resumes this same job.
        result = json.loads(command(["xcrun", "notarytool", "submit", str(upload), "--keychain-profile",
                                     plan["notary_profile"], "--output-format", "json"], candidate / "notary-submit.log"))
        immutable(identifier, encode({"id": result["id"], "app_files": delivery.inventory(app, include_receipt=True)}))
    submission = read(identifier)
    if submission["app_files"] != delivery.inventory(app, include_receipt=True):
        # Stapling may add its ticket after a previous successful wait. Verify
        # the existing ticket before allowing a resumed archive-only attempt.
        command(["xcrun", "stapler", "validate", str(app)], candidate / "existing-ticket.log")
    result = json.loads(command(["xcrun", "notarytool", "wait", submission["id"], "--keychain-profile",
                                 plan["notary_profile"], "--output-format", "json", "--timeout", "30m"],
                                candidate / "notary-wait.log"))
    if result.get("status") != "Accepted":
        command(["xcrun", "notarytool", "log", submission["id"], "--keychain-profile", plan["notary_profile"]],
                candidate / "notary-diagnostics.json")
        raise Refusal("notarization did not accept this app; retain diagnostics and make a new release for repairs")
    command(["xcrun", "stapler", "staple", str(app)], candidate / "staple.log")
    command(["xcrun", "stapler", "validate", str(app)], candidate / "stapler-validation.log")
    command(["spctl", "--assess", "--type", "execute", "--verbose=4", str(app)], candidate / "gatekeeper.log")
    check_bundle(app, plan)
    return submission["id"]


def package(candidate, *, adopt_release_target=False):
    with locked(candidate):
        plan = validate_plan(candidate)
        if (candidate / "release.json").exists():
            completed = verify_release(candidate / "release.json")
            publish_channel(candidate / "release.json")
            return completed
        preflight(candidate, plan)
        env = release_environment(candidate, plan)
        report_ref = candidate / "ci-report.json"
        if report_ref.exists():
            ci_report = Path(read(report_ref)["path"])
            if delivery.sha(ci_report) != read(report_ref)["sha256"]:
                raise Refusal("retained full CI report changed")
        else:
            runner = ci.LocalCiRunner(ROOT, env=env)
            if runner.run() != 0:
                raise Refusal(f"full local CI failed: {runner.summary_path}; resume after resolving the failure")
            ci_report = runner.summary_path
            immutable(report_ref, encode({"path": str(ci_report), "sha256": delivery.sha(ci_report)}))
        app = candidate / "hex.app"
        if not app.exists():
            runner = ci.LocalCiRunner(ROOT, env=env)
            runner.output_dir.mkdir(parents=True)

            def revalidate():
                validate_plan(candidate)
                delivery.validate_ci(ci_report, runner, plan["source"])

            revalidate()
            with delivery.optimized_artifact(runner, adopt=adopt_release_target, verification_check=revalidate) as (binary, ownership):
                stage = candidate / "bundle.pending.app"
                discard_generated_stage(candidate, "bundle.pending.app")
                resources = stage / "Contents/Resources"
                (stage / "Contents/MacOS").mkdir(parents=True)
                (resources / "licenses").mkdir(parents=True)
                shutil.copy2(binary, stage / "Contents/MacOS/hex")
                for filename, name in (("assets/fonts/ioskeley-mono/OFL.txt", "ioskeley-mono-OFL.txt"),
                                       ("third_party/gpui/LICENSE-APACHE", "gpui-LICENSE-APACHE"),
                                       ("ATTRIBUTION.md", "ATTRIBUTION.md")):
                    shutil.copy2(ROOT / filename, resources / "licenses" / name)
                for name in ("notes.json", "snapshot.json", "RELEASE-NOTES.md", "TESTING.md"):
                    shutil.copy2(candidate / name, resources / name)
                shutil.copytree(candidate / "manual", resources / "manual")
                shutil.copy2(ci_report, resources / "local-ci-summary.json")
                info = {"CFBundleExecutable": "hex", "CFBundleIdentifier": delivery.IDENTITY,
                        "CFBundleName": "hex", "CFBundleDisplayName": "hex", "CFBundlePackageType": "APPL",
                        "CFBundleShortVersionString": plan["target"], "CFBundleVersion": str(plan["build"]),
                        "NSHighResolutionCapable": True, "HexSourceRevision": plan["source"],
                        "NSMicrophoneUsageDescription": "hex uses audio input to record audio into your project.",
                        "HexReleaseVersion": plan["version"], "HexReleaseChannel": plan["channel"]}
                with (stage / "Contents/Info.plist").open("wb") as output:
                    plistlib.dump(info, output)
                receipt = {**plan, "ci_report_sha256": delivery.sha(ci_report), "profile": "release",
                           "build_argv": ownership["build_argv"], "input_executable_sha256": delivery.sha(binary),
                           "source_sha256": runner.summary["start_tree"]["content_sha256"],
                           "signature": "Developer ID hardened runtime with secure timestamp",
                           "verification": {"mode": "full-ci", "tests": "passed"}}
                immutable(resources / "release-build.json", encode(receipt))
                command(["codesign", "--force", "--sign", plan["signing"]["certificate_sha1"],
                         "--identifier", delivery.IDENTITY, "--options", "runtime", "--timestamp",
                         "--entitlements", str(ROOT / "scripts/hex-release.entitlements.plist"), str(stage)],
                        candidate / "codesign.log")
                check_bundle(stage, plan)
                revalidate()
                stage.rename(app)
        check_bundle(app, plan)
        submission = notarize(candidate, app, plan)
        pending = candidate / "package.pending.zip"
        if pending.exists():
            pending.unlink()
        payload = candidate / "archive-payload"
        discard_generated_stage(candidate, "archive-payload")
        payload.mkdir()
        command(["ditto", str(app), str(payload / "hex.app")], candidate / "archive-copy.log")
        for name in ("RELEASE-NOTES.md", "TESTING.md"):
            shutil.copy2(candidate / name, payload / name)
        for name in ("hex-manual.html", "hex-manual.pdf", "documentation.json"):
            shutil.copy2(candidate / "manual" / name, payload / name)
        command(["ditto", "-c", "-k", "--keepParent", str(payload), str(pending)], candidate / "archive.log")
        shutil.rmtree(payload)
        validate_plan(candidate)
        with tempfile.TemporaryDirectory(prefix="hex-release-verification-") as temporary:
            extracted = Path(temporary)
            command(["ditto", "-x", "-k", str(pending), str(extracted)], candidate / "extract.log")
            final_app = check_payload(extracted, plan)
            command(["xcrun", "stapler", "validate", str(final_app)], candidate / "extracted-stapler.log")
            command(["spctl", "--assess", "--type", "execute", "--verbose=4", str(final_app)],
                    candidate / "extracted-gatekeeper.log")
            if delivery.inventory(final_app, include_receipt=True) != delivery.inventory(app, include_receipt=True):
                raise Refusal("extracted app bytes differ from notarized app")
        archive = candidate / f"hex-{plan['version']}-macos-arm64.zip"
        # An interrupted final publication may already have the verified archive.
        # Never rewrite it: extract and verify the retained bytes before completing.
        if archive.exists():
            with tempfile.TemporaryDirectory(prefix="hex-release-recovery-") as temporary:
                command(["ditto", "-x", "-k", str(archive), temporary], candidate / "recovery-extract.log")
                recovered = check_payload(Path(temporary), plan)
                if delivery.inventory(recovered, include_receipt=True) != delivery.inventory(app, include_receipt=True):
                    raise Refusal("retained publication archive differs from notarized app")
                command(["xcrun", "stapler", "validate", str(recovered)], candidate / "recovery-stapler.log")
                command(["spctl", "--assess", "--type", "execute", str(recovered)], candidate / "recovery-gatekeeper.log")
            pending.unlink()
        else:
            os.link(pending, archive)
            pending.unlink()
            delivery.sync_directory(candidate)
        manifest = {**plan, "archive": archive.name, "archive_sha256": delivery.sha(archive),
                    "app_files": delivery.inventory(app, include_receipt=True), "notarization_id": submission,
                    "ci_report": str(ci_report), "ci_report_sha256": delivery.sha(ci_report),
                    "status": "ready", "delivery": "artifact_only", "native_acceptance": "outstanding",
                    "completed_at": ci.utc_now()}
        immutable(candidate / "release.json", encode(manifest))
        publish_channel(candidate / "release.json")
        return manifest


def verify_release(path):
    manifest = read(path)
    if manifest.get("schema") != 1 or manifest.get("status") != "ready":
        raise Refusal("release is incomplete or unsupported")
    filename = manifest["archive"]
    if Path(filename).name != filename or not filename.endswith(".zip"):
        raise Refusal("invalid release archive name")
    archive = path.parent / filename
    if archive.is_symlink() or delivery.sha(archive) != manifest["archive_sha256"]:
        raise Refusal("release archive hash mismatch")
    with tempfile.TemporaryDirectory(prefix="hex-release-verify-") as temporary:
        # Only locally produced archives with exact immutable manifest hashes are
        # accepted. Verification does not require the builder's current checkout.
        extracted = Path(temporary)
        subprocess.run(["ditto", "-x", "-k", str(archive), str(extracted)], check=True)
        app = check_payload(extracted, manifest)
        if delivery.inventory(app, include_receipt=True) != manifest["app_files"]:
            raise Refusal("release app inventory mismatch")
        subprocess.run(["xcrun", "stapler", "validate", str(app)], check=True)
        subprocess.run(["spctl", "--assess", "--type", "execute", "--verbose=4", str(app)], check=True)
    return manifest


def install(path):
    manifest = verify_release(path)
    configuration = read(ROOT / "scripts/hex-test-build.json")
    common = store_path().parent.parent
    configured = Path(os.environ.get("HEX_DEV_APP_PATH", configuration["app_path"]))
    app = configured if configured.is_absolute() else common / configured
    if app.name.casefold() != "hex.app":
        raise Refusal("configured development app must be named hex.app")
    delivery.assert_plain_path(app)
    app.parent.mkdir(parents=True, exist_ok=True)
    with delivery.activation_lock(app):
        delivery.recover(app)
        delivery.assert_inactive(app)
        workspace = delivery.install_workspace(app)
        workspace.mkdir()
        with tempfile.TemporaryDirectory(prefix="hex-release-install-") as temporary:
            extracted = Path(temporary)
            archive = path.parent / manifest["archive"]
            subprocess.run(["ditto", "-x", "-k", str(archive), str(extracted)], check=True)
            if delivery.sha(archive) != manifest["archive_sha256"]:
                raise Refusal("release archive changed during installation")
            checked = check_payload(extracted, manifest)
            if delivery.inventory(checked, include_receipt=True) != manifest["app_files"]:
                raise Refusal("installation app differs from release manifest")
            subprocess.run(["ditto", str(checked), str(workspace / "stage.app")], check=True)
        delivery.activate(app, workspace / "stage.app")
        receipt = {"release_manifest": str(path), "manifest_sha256": delivery.sha(path),
                   "source": manifest["source"], "version": manifest["version"], "build": manifest["build"],
                   "app_path": str(app), "status": "installed on disk; not launched", "installed_at": ci.utc_now()}
        delivery.write_json(app.parent / "hex-release-install-receipt.json", receipt)
        return receipt


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    for channel in ("beta", "nightly", "stable"):
        sub = commands.add_parser(channel, help=f"freeze and package a new {channel}; no uploads")
        sub.set_defaults(channel=channel)
        sub.add_argument("--version", help="target major.minor.patch (defaults to Cargo workspace base)")
        sub.add_argument("--base", help="explicit initial notes baseline; default previous stable or retained notes")
        sub.add_argument("--checklist", type=Path)
        sub.add_argument("--included", type=Path)
        sub.add_argument("--max-changes", type=int, default=512)
        sub.add_argument("--generation-timeout", type=int, default=300, help="seconds per bounded notes input, 1..600")
        sub.add_argument("--accept-source", help="explicit native-accepted full source SHA for stable")
        sub.add_argument("--adopt-release-target", action="store_true",
                         help="explicitly adopt an inspected Cargo cache for first release/recovery")
    for operation in ("resume", "verify", "install"):
        sub = commands.add_parser(operation)
        sub.add_argument("path", type=Path, help="candidate directory for resume; release.json for verify/install")
        if operation == "resume":
            sub.add_argument("--adopt-release-target", action="store_true",
                             help="explicitly adopt an inspected Cargo cache for first release/recovery")
    sub = commands.add_parser("notes", help="recover/prepare rolling notes without cutting a version")
    sub.add_argument("--base")
    sub.add_argument("--max-changes", type=int, default=20)
    sub.add_argument("--generation-timeout", type=int, default=300)
    commands.add_parser("status", help="list immutable locally completed releases")
    args = parser.parse_args(argv)
    try:
        store = store_path()
        if args.operation == "status":
            result = [{"manifest": str(p), "version": v["version"], "channel": v["channel"],
                       "build": v["build"], "source": v["source"]} for p, v in ready_releases(store)]
        elif args.operation == "notes":
            result = prepare_notes(store / "unreleased", source(), base=args.base,
                                   max_changes=args.max_changes, timeout=args.generation_timeout)
        elif args.operation == "verify":
            result = verify_release(args.path.resolve())
        elif args.operation == "install":
            result = install(args.path.resolve())
        else:
            candidate = args.path.resolve() if args.operation == "resume" else cut(args, store)
            adoption = " --adopt-release-target" if args.adopt_release_target else ""
            print(f"frozen release: {candidate}\nresume: ./release resume {candidate}{adoption}", flush=True)
            result = package(candidate, adopt_release_target=args.adopt_release_target)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    except (Refusal, notes.Refusal, ci.TargetCacheError, OSError, ValueError, KeyError, TypeError,
            subprocess.CalledProcessError, KeyboardInterrupt) as error:
        print(f"release incomplete: {error}; previous releases preserved; no upload or process termination", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
