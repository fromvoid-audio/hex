#!/usr/bin/env python3
"""Optional, disabled-by-default local nightly release schedule evaluator and driver.

This module owns explicit local nightly schedule evaluation (`03:00 Europe/Amsterdam`
by default), DST-aware single-catch-up slot selection, unchanged-application-source
skipping, single-publisher locking, busy/unsupported-platform visible deferral,
bounded retry/deduplication, and failure retention while preserving the last-good
published nightly channel manifest.

Strict invariants:
- Disabled by default; never installs cron jobs, LaunchAgents, daemons, or hosted CI.
- Strictly nightly-only; `beta` and `stable` cuts remain explicit manual commands.
- Invokes the exact local `scripts/hex-release.py nightly` pipeline with full checks;
  never weakens checks, never kills processes, never installs/launches/quits `hex.app`,
  and never modifies user projects.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import date, datetime, time as dt_time, timedelta, timezone
import errno
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import platform
import stat
import subprocess
import sys
import tempfile
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

ROOT = Path(__file__).resolve().parent.parent

DEFAULT_APPLICATION_SOURCE_PREFIXES: tuple[str, ...] = (
    "crates/",
    "third_party/",
    "assets/",
    "Cargo.toml",
    "Cargo.lock",
    "scripts/hex-release.py",
    "scripts/hex-test-build.py",
    "scripts/local-ci.py",
    "scripts/manual.py",
    "scripts/release_notes.py",
)

MAX_RETAINED_FAILURES = 50


class ScheduleRefusal(RuntimeError):
    """Raised when configuration or safety invariants are violated."""


@dataclass(frozen=True)
class ScheduleConfig:
    enabled: bool = False
    channel: str = "nightly"
    timezone: str = "Europe/Amsterdam"
    hour: int = 3
    minute: int = 0
    max_retries_per_source: int = 2
    application_source_prefixes: tuple[str, ...] = DEFAULT_APPLICATION_SOURCE_PREFIXES
    fresh_release_target: str | None = None
    existing_release_target: str | None = None

    def validate(self) -> ZoneInfo:
        if not isinstance(self.enabled, bool):
            raise ScheduleRefusal("schedule config 'enabled' must be a boolean")
        if self.channel != "nightly":
            raise ScheduleRefusal(
                f"scheduled releases only allow channel 'nightly', refused {self.channel!r}; "
                "beta and stable cuts remain explicit manual commands"
            )
        if not isinstance(self.hour, int) or isinstance(self.hour, bool) or not (0 <= self.hour <= 23):
            raise ScheduleRefusal("schedule config 'hour' must be an integer in 0..23")
        if not isinstance(self.minute, int) or isinstance(self.minute, bool) or not (0 <= self.minute <= 59):
            raise ScheduleRefusal("schedule config 'minute' must be an integer in 0..59")
        if (
            not isinstance(self.max_retries_per_source, int)
            or isinstance(self.max_retries_per_source, bool)
            or not (1 <= self.max_retries_per_source <= 10)
        ):
            raise ScheduleRefusal("schedule config 'max_retries_per_source' must be in 1..10")
        if not self.application_source_prefixes:
            raise ScheduleRefusal("schedule config 'application_source_prefixes' must be non-empty")
        if self.fresh_release_target is not None and self.existing_release_target is not None:
            raise ScheduleRefusal(
                "fresh_release_target and existing_release_target are mutually exclusive"
            )
        try:
            return ZoneInfo(self.timezone)
        except (ZoneInfoNotFoundError, ValueError, KeyError) as error:
            raise ScheduleRefusal(f"invalid schedule timezone: {self.timezone!r}") from error


def load_config(path: Path) -> ScheduleConfig:
    """Load an explicit local schedule JSON config; absent file yields disabled default."""
    if not path.exists() and not path.is_symlink():
        return ScheduleConfig(enabled=False)
    if path.is_symlink() or not path.is_file():
        raise ScheduleRefusal(f"schedule config must be a regular file: {path}")
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise ScheduleRefusal("schedule config root must be a JSON object")
    if raw.get("schema", 1) != 1:
        raise ScheduleRefusal(f"unsupported schedule config schema: {raw.get('schema')!r}")
    for forbidden in ("install", "auto_install", "launch_app", "quit_app", "kill_busy", "skip_checks"):
        if raw.get(forbidden):
            raise ScheduleRefusal(f"forbidden schedule option: {forbidden}")
    prefixes = raw.get("application_source_prefixes")
    if prefixes is None:
        prefix_tuple = DEFAULT_APPLICATION_SOURCE_PREFIXES
    elif isinstance(prefixes, list) and all(isinstance(p, str) and p for p in prefixes):
        prefix_tuple = tuple(prefixes)
    else:
        raise ScheduleRefusal("application_source_prefixes must be a list of non-empty strings")
    config = ScheduleConfig(
        enabled=raw.get("enabled", False),
        channel=raw.get("channel", "nightly"),
        timezone=raw.get("timezone", "Europe/Amsterdam"),
        hour=raw.get("hour", 3),
        minute=raw.get("minute", 0),
        max_retries_per_source=raw.get("max_retries_per_source", 2),
        application_source_prefixes=prefix_tuple,
        fresh_release_target=raw.get("fresh_release_target"),
        existing_release_target=raw.get("existing_release_target"),
    )
    config.validate()
    return config


def slot_utc_for_local_date(local_day: date, hour: int, minute: int, tz: ZoneInfo) -> datetime:
    """Resolve the canonical UTC instant for a local calendar day's scheduled slot.

    Uses `fold=0` for fall-back DST ambiguity (first occurrence) and lets ZoneInfo
    normalize any nonexistent spring-forward wall time cleanly without crashing.
    """
    naive = datetime.combine(local_day, dt_time(hour=hour, minute=minute))
    aware = naive.replace(tzinfo=tz, fold=0)
    # Round-trip through UTC to verify whether the wall time fell inside a spring-forward gap.
    utc_candidate = aware.astimezone(timezone.utc)
    round_tripped = utc_candidate.astimezone(tz)
    if (round_tripped.hour, round_tripped.minute) != (hour, minute) and round_tripped.date() == local_day:
        # Nonexistent spring-forward time (e.g., 02:30 during 02:00->03:00 jump):
        # advance to the first valid instant after the transition.
        return round_tripped.astimezone(timezone.utc)
    return utc_candidate


def latest_due_slot_utc(now_utc: datetime, config: ScheduleConfig) -> datetime:
    """Return the most recent scheduled slot at or before `now_utc` (in UTC)."""
    if now_utc.tzinfo is None:
        raise ScheduleRefusal("now_utc must be timezone-aware")
    tz = config.validate()
    now_utc = now_utc.astimezone(timezone.utc)
    local_now = now_utc.astimezone(tz)
    today_slot = slot_utc_for_local_date(local_now.date(), config.hour, config.minute, tz)
    if now_utc >= today_slot:
        return today_slot
    yesterday = local_now.date() - timedelta(days=1)
    return slot_utc_for_local_date(yesterday, config.hour, config.minute, tz)


def count_due_slots_since(
    last_handled_slot_utc: datetime | None,
    due_slot_utc: datetime,
    config: ScheduleConfig,
) -> int:
    """Count distinct scheduled slots strictly after `last_handled_slot_utc` up to `due_slot_utc`."""
    if last_handled_slot_utc is None:
        return 1
    last_utc = last_handled_slot_utc.astimezone(timezone.utc)
    if due_slot_utc <= last_utc:
        return 0
    tz = config.validate()
    start_day = last_utc.astimezone(tz).date()
    end_day = due_slot_utc.astimezone(tz).date()
    count = 0
    cursor = start_day
    while cursor <= end_day:
        slot = slot_utc_for_local_date(cursor, config.hour, config.minute, tz)
        if last_utc < slot <= due_slot_utc:
            count += 1
        cursor += timedelta(days=1)
    return count


def is_application_source_path(rel_path: str, prefixes: tuple[str, ...]) -> bool:
    normalized = rel_path.lstrip("./")
    for prefix in prefixes:
        if prefix.endswith("/"):
            if normalized.startswith(prefix):
                return True
        elif normalized == prefix:
            return True
    return False


def has_application_source_changes(changed_paths: list[str], prefixes: tuple[str, ...]) -> bool:
    return any(is_application_source_path(path, prefixes) for path in changed_paths)


def _format_utc(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _parse_utc(text: str | None) -> datetime | None:
    if not text:
        return None
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    parsed = datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        raise ScheduleRefusal(f"timestamp must include timezone: {text!r}")
    return parsed.astimezone(timezone.utc)


def _read_bounded_json(path: Path) -> dict | None:
    if not path.exists() and not path.is_symlink():
        return None
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise ScheduleRefusal(f"refusing symbolic link metadata: {path}") from None
        raise
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 2 * 1024 * 1024:
            raise ScheduleRefusal(f"expected bounded regular JSON file: {path}")
        with os.fdopen(fd, "rb", closefd=False) as handle:
            raw = handle.read(2 * 1024 * 1024 + 1)
        if len(raw) > 2 * 1024 * 1024:
            raise ScheduleRefusal(f"JSON file exceeds limit: {path}")
        parsed = json.loads(raw.decode("utf-8"))
        if not isinstance(parsed, dict):
            raise ScheduleRefusal(f"expected JSON object in {path}")
        return parsed
    finally:
        os.close(fd)


def _write_atomic_json(path: Path, payload: dict) -> None:
    if path.is_symlink():
        raise ScheduleRefusal(f"refusing to overwrite symbolic link: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")
    fd, tmp_name = tempfile.mkstemp(prefix=".nightly-schedule-", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_name, path)
    except Exception:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def load_state(store: Path) -> dict:
    path = store / "nightly-schedule-state.json"
    data = _read_bounded_json(path)
    if data is None:
        return {
            "schema": 1,
            "last_handled_slot_utc": None,
            "last_published_source": None,
            "last_good_manifest": None,
            "attempts_by_source": {},
            "failures": [],
            "last_outcome": None,
        }
    if data.get("schema") != 1:
        raise ScheduleRefusal(f"unsupported schedule state schema: {data.get('schema')!r}")
    return data


def save_state(store: Path, state: dict) -> None:
    _write_atomic_json(store / "nightly-schedule-state.json", state)


def read_published_nightly_channel(store: Path) -> dict | None:
    return _read_bounded_json(store / "channels" / "nightly.json")


@contextmanager
def try_schedule_lock(store: Path):
    """Non-blocking single-publisher schedule lock; yields False when already held."""
    store.mkdir(parents=True, exist_ok=True)
    lock_path = store / ".nightly-schedule.lock"
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(lock_path, flags, 0o600)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise ScheduleRefusal(f"schedule lock is a symbolic link: {lock_path}") from None
        raise
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ScheduleRefusal(f"schedule lock is not a regular file: {lock_path}")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        try:
            yield True
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


def is_lock_held_by_other(path: Path) -> bool:
    """Non-disruptively probe whether another process holds `flock` on `path`."""
    flags = os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except FileNotFoundError:
        return False
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise ScheduleRefusal(f"lock path is a symbolic link: {path}") from None
        raise
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ScheduleRefusal(f"lock path is not a regular file: {path}")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        else:
            fcntl.flock(fd, fcntl.LOCK_UN)
            return False
    finally:
        os.close(fd)


def default_cargo_slot_busy(slot_path: Path | None = None) -> tuple[bool, str | None]:
    """Read-only probe of `scripts/cargo-slot.py` without acquiring or disturbing locks."""
    spec = importlib.util.spec_from_file_location("hex_cargo_slot_probe", ROOT / "scripts" / "cargo-slot.py")
    if spec is None or spec.loader is None:
        return False, None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    target = slot_path if slot_path is not None else mod.default_slot_path()
    status = mod.inspect_slot_status(target)
    if status["state"] == "occupied":
        summary = mod.format_holder_summary(status["holder"])
        return True, f"shared cargo slot occupied ({summary})"
    return False, None


def default_git_head(repo_root: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def default_git_clean(repo_root: Path) -> bool:
    result = subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain", "--untracked-files=normal"],
        capture_output=True,
        text=True,
        check=True,
    )
    return not result.stdout.strip()


def default_git_changed_files(repo_root: Path, base_sha: str, head_sha: str) -> list[str]:
    result = subprocess.run(
        ["git", "-C", str(repo_root), "diff", "--name-only", f"{base_sha}..{head_sha}"],
        capture_output=True,
        text=True,
        check=True,
    )
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def default_platform_preflight(repo_root: Path) -> tuple[bool, str | None]:
    if sys.platform != "darwin" or platform.machine() != "arm64":
        return False, f"unsupported host platform {sys.platform}/{platform.machine()} (requires macOS arm64)"
    if not default_git_clean(repo_root):
        return False, "worktree has uncommitted changes; deferring nightly build"
    return True, None


def build_nightly_release_command(config: ScheduleConfig) -> list[str]:
    """Construct the exact local `hex-release.py nightly` command (never install/beta/stable)."""
    config.validate()
    cmd = [sys.executable, str(ROOT / "scripts" / "hex-release.py"), "nightly"]
    if config.fresh_release_target is not None:
        cmd.extend(["--fresh-release-target", config.fresh_release_target])
    elif config.existing_release_target is not None:
        cmd.extend(["--existing-release-target", config.existing_release_target])
    return cmd


def default_run_nightly_pipeline(
    config: ScheduleConfig,
    repo_root: Path,
    now_utc: datetime,
) -> dict:
    """Run the exact local `hex-release.py nightly` pipeline without installing or signaling."""
    cmd = build_nightly_release_command(config)
    completed = subprocess.run(cmd, cwd=repo_root, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip()
        raise RuntimeError(f"hex-release nightly failed (exit {completed.returncode}): {detail}")
    # hex-release prints human lines before the final JSON payload.
    stdout = completed.stdout.strip()
    start = stdout.find("{")
    if start < 0:
        raise RuntimeError("hex-release nightly did not emit a JSON manifest")
    manifest = json.loads(stdout[start:])
    if not isinstance(manifest, dict):
        raise RuntimeError("hex-release nightly emitted non-object manifest")
    return manifest


def run_scheduled_check(
    config: ScheduleConfig,
    store: Path,
    *,
    now_utc: datetime | None = None,
    repo_root: Path = ROOT,
    cargo_slot_path: Path | None = None,
    get_head_sha=None,
    get_changed_files=None,
    platform_preflight=None,
    cargo_slot_busy_check=None,
    release_runner=None,
) -> dict:
    """Evaluate the local nightly schedule once and execute at most ONE due run.

    Never kills processes, never weakens checks, never installs or signals `hex.app`,
    and never mutates `channels/nightly.json` on failure.
    """
    config.validate()
    now = (now_utc or datetime.now(timezone.utc)).astimezone(timezone.utc)
    if not config.enabled:
        return {
            "status": "disabled",
            "evaluated_at_utc": _format_utc(now),
            "reason": "local nightly schedule is disabled by configuration",
        }

    due_slot = latest_due_slot_utc(now, config)
    due_slot_str = _format_utc(due_slot)

    with try_schedule_lock(store) as acquired:
        if not acquired:
            return {
                "status": "deferred_busy",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "reason": "another scheduled nightly run already holds the schedule lock",
            }

        state = load_state(store)
        last_handled = _parse_utc(state.get("last_handled_slot_utc"))
        missed_slots = count_due_slots_since(last_handled, due_slot, config)
        if missed_slots == 0:
            return {
                "status": "not_due",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "last_handled_slot_utc": state.get("last_handled_slot_utc"),
            }

        # Check simultaneous manual release or busy shared Cargo slot before starting work.
        if is_lock_held_by_other(store / ".lock"):
            outcome = {
                "status": "deferred_busy",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "missed_slots": missed_slots,
                "reason": "manual release lock is active in release store",
            }
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        slot_check = cargo_slot_busy_check or (lambda: default_cargo_slot_busy(cargo_slot_path))
        busy, busy_reason = slot_check()
        if busy:
            outcome = {
                "status": "deferred_busy",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "missed_slots": missed_slots,
                "reason": busy_reason or "shared cargo slot is busy",
            }
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        preflight = platform_preflight or (lambda: default_platform_preflight(repo_root))
        supported, preflight_reason = preflight()
        if not supported:
            outcome = {
                "status": "deferred_unsupported_platform",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "missed_slots": missed_slots,
                "reason": preflight_reason or "host platform unavailable for native release",
            }
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        head_fn = get_head_sha or (lambda: default_git_head(repo_root))
        current_source = head_fn()
        published_channel = read_published_nightly_channel(store)
        last_published_source = (
            published_channel.get("source")
            if published_channel and isinstance(published_channel.get("source"), str)
            else state.get("last_published_source")
        )

        if last_published_source == current_source:
            outcome = {
                "status": "skipped_unchanged_source",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "missed_slots": missed_slots,
                "source": current_source,
            }
            state["last_handled_slot_utc"] = due_slot_str
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        if last_published_source:
            changed_fn = get_changed_files or (
                lambda base, head: default_git_changed_files(repo_root, base, head)
            )
            changed = changed_fn(last_published_source, current_source)
            if not has_application_source_changes(changed, config.application_source_prefixes):
                outcome = {
                    "status": "skipped_unchanged_application_source",
                    "evaluated_at_utc": _format_utc(now),
                    "due_slot_utc": due_slot_str,
                    "missed_slots": missed_slots,
                    "source": current_source,
                    "last_published_source": last_published_source,
                    "changed_files": changed,
                }
                state["last_handled_slot_utc"] = due_slot_str
                state["last_outcome"] = outcome
                save_state(store, state)
                return outcome

        attempts_map = state.setdefault("attempts_by_source", {})
        prior_attempts = int(attempts_map.get(current_source, 0))
        if prior_attempts >= config.max_retries_per_source:
            outcome = {
                "status": "skipped_retry_budget_exhausted",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "missed_slots": missed_slots,
                "source": current_source,
                "attempts": prior_attempts,
                "max_retries_per_source": config.max_retries_per_source,
            }
            state["last_handled_slot_utc"] = due_slot_str
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        run_reason = "catchup" if missed_slots > 1 else "scheduled"
        runner = release_runner or (lambda cfg, root, ts: default_run_nightly_pipeline(cfg, root, ts))
        try:
            manifest = runner(config, repo_root, now)
        except Exception as error:
            attempts_map[current_source] = prior_attempts + 1
            failure_entry = {
                "at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "run_reason": run_reason,
                "missed_slots": missed_slots,
                "source": current_source,
                "attempt": prior_attempts + 1,
                "error": str(error),
            }
            failures = state.setdefault("failures", [])
            failures.append(failure_entry)
            if len(failures) > MAX_RETAINED_FAILURES:
                del failures[:-MAX_RETAINED_FAILURES]
            # If retries remain, leave last_handled_slot_utc un-advanced so one bounded retry
            # can run on a later check; once exhausted, close the slot to prevent spin.
            if prior_attempts + 1 >= config.max_retries_per_source:
                state["last_handled_slot_utc"] = due_slot_str
            outcome = {
                "status": "failed_retained",
                "evaluated_at_utc": _format_utc(now),
                "due_slot_utc": due_slot_str,
                "run_reason": run_reason,
                "missed_slots": missed_slots,
                "source": current_source,
                "attempt": prior_attempts + 1,
                "retries_remaining": max(0, config.max_retries_per_source - (prior_attempts + 1)),
                "error": str(error),
                "last_good_channel_preserved": published_channel is not None,
            }
            state["last_outcome"] = outcome
            save_state(store, state)
            return outcome

        outcome = {
            "status": "published",
            "evaluated_at_utc": _format_utc(now),
            "due_slot_utc": due_slot_str,
            "run_reason": run_reason,
            "missed_slots": missed_slots,
            "source": current_source,
            "version": manifest.get("version"),
            "build": manifest.get("build"),
            "channel": manifest.get("channel", "nightly"),
        }
        state["last_handled_slot_utc"] = due_slot_str
        state["last_published_source"] = current_source
        state["last_good_manifest"] = manifest
        state["last_outcome"] = outcome
        save_state(store, state)
        return outcome


def default_config_path(store: Path) -> Path:
    return store / "nightly-schedule.json"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Optional, disabled-by-default local nightly release schedule check."
    )
    parser.add_argument(
        "--config",
        type=Path,
        help="path to explicit schedule JSON config (defaults to <store>/nightly-schedule.json)",
    )
    parser.add_argument(
        "--store",
        type=Path,
        help="release store directory (defaults to artifacts/releases beside git common dir)",
    )
    parser.add_argument(
        "--dry-run-slot",
        action="store_true",
        help="print current config and latest due slot in UTC/local timezone without executing",
    )
    args = parser.parse_args(argv)
    try:
        store = args.store.resolve() if args.store else (ROOT / "artifacts" / "releases")
        config_path = args.config.resolve() if args.config else default_config_path(store)
        config = load_config(config_path)
        if args.dry_run_slot:
            tz = config.validate()
            now = datetime.now(timezone.utc)
            due = latest_due_slot_utc(now, config)
            print(
                json.dumps(
                    {
                        "enabled": config.enabled,
                        "channel": config.channel,
                        "timezone": config.timezone,
                        "hour": config.hour,
                        "minute": config.minute,
                        "now_utc": _format_utc(now),
                        "due_slot_utc": _format_utc(due),
                        "due_slot_local": due.astimezone(tz).isoformat(),
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
            return 0
        outcome = run_scheduled_check(config, store)
        print(json.dumps(outcome, indent=2, sort_keys=True))
        return 0 if outcome["status"] != "failed_retained" else 1
    except (ScheduleRefusal, OSError, ValueError, KeyError, TypeError) as error:
        print(f"nightly schedule error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
