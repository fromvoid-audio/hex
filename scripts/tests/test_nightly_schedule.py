"""Deterministic tests for the optional local nightly release schedule (#2877)."""

from __future__ import annotations

from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from zoneinfo import ZoneInfo

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import nightly_schedule as ns  # noqa: E402


class NightlyScheduleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.store = self.root / "releases"
        self.store.mkdir()

    def _write_channel_manifest(self, source: str, version: str = "0.1.0-nightly.20261125.1", build: int = 10):
        channels = self.store / "channels"
        channels.mkdir(exist_ok=True)
        payload = {
            "schema": 1,
            "channel": "nightly",
            "version": version,
            "build": build,
            "source": source,
            "manifest": str(self.store / "candidates" / "prev" / "release.json"),
            "manifest_sha256": "a" * 64,
            "archive_sha256": "b" * 64,
        }
        path = channels / "nightly.json"
        path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return path, path.read_bytes()

    def test_disabled_by_default_and_refuses_beta_stable_or_forbidden_options(self):
        # Absent config file defaults to disabled and performs no work.
        cfg = ns.load_config(self.store / "missing-schedule.json")
        self.assertFalse(cfg.enabled)
        called = []
        outcome = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 26, 4, 0, tzinfo=timezone.utc),
            release_runner=lambda *a: called.append(a),
        )
        self.assertEqual(outcome["status"], "disabled")
        self.assertEqual(called, [])
        self.assertFalse((self.store / "nightly-schedule-state.json").exists())

        # Beta and stable channels are strictly forbidden from scheduled cuts.
        for channel in ("beta", "stable"):
            with self.subTest(channel=channel):
                bad = self.root / f"{channel}.json"
                bad.write_text(json.dumps({"enabled": True, "channel": channel}), encoding="utf-8")
                with self.assertRaises(ns.ScheduleRefusal):
                    ns.load_config(bad)

        # Forbidden install/launch/quit/kill/skip options are refused.
        for option in ("install", "auto_install", "launch_app", "quit_app", "kill_busy", "skip_checks"):
            with self.subTest(option=option):
                bad = self.root / f"{option}.json"
                bad.write_text(json.dumps({"enabled": True, option: True}), encoding="utf-8")
                with self.assertRaises(ns.ScheduleRefusal):
                    ns.load_config(bad)

    def test_timezone_and_dst_transitions_in_europe_amsterdam(self):
        cfg = ns.ScheduleConfig(enabled=True, timezone="Europe/Amsterdam", hour=3, minute=0)
        tz = ZoneInfo("Europe/Amsterdam")

        # Winter (CET = UTC+1): 03:00 local == 02:00 UTC.
        before_winter = datetime(2026, 1, 15, 1, 59, tzinfo=timezone.utc)
        at_winter = datetime(2026, 1, 15, 2, 0, tzinfo=timezone.utc)
        self.assertEqual(
            ns.latest_due_slot_utc(before_winter, cfg),
            datetime(2026, 1, 14, 2, 0, tzinfo=timezone.utc),
        )
        self.assertEqual(
            ns.latest_due_slot_utc(at_winter, cfg),
            datetime(2026, 1, 15, 2, 0, tzinfo=timezone.utc),
        )

        # Summer (CEST = UTC+2): 03:00 local == 01:00 UTC.
        before_summer = datetime(2026, 7, 15, 0, 59, tzinfo=timezone.utc)
        at_summer = datetime(2026, 7, 15, 1, 0, tzinfo=timezone.utc)
        self.assertEqual(
            ns.latest_due_slot_utc(before_summer, cfg),
            datetime(2026, 7, 14, 1, 0, tzinfo=timezone.utc),
        )
        self.assertEqual(
            ns.latest_due_slot_utc(at_summer, cfg),
            datetime(2026, 7, 15, 1, 0, tzinfo=timezone.utc),
        )

        # Spring forward day in Europe/Amsterdam: 2026-03-29 (02:00 CET -> 03:00 CEST = 01:00 UTC).
        spring_due = ns.latest_due_slot_utc(datetime(2026, 3, 29, 1, 0, tzinfo=timezone.utc), cfg)
        self.assertEqual(spring_due, datetime(2026, 3, 29, 1, 0, tzinfo=timezone.utc))
        self.assertEqual(spring_due.astimezone(tz).hour, 3)

        # Fall back day in Europe/Amsterdam: 2026-10-25 (03:00 CEST -> 02:00 CET; 03:00 CET = 02:00 UTC).
        fall_before = ns.latest_due_slot_utc(datetime(2026, 10, 25, 1, 30, tzinfo=timezone.utc), cfg)
        self.assertEqual(fall_before, datetime(2026, 10, 24, 1, 0, tzinfo=timezone.utc))
        fall_at = ns.latest_due_slot_utc(datetime(2026, 10, 25, 2, 0, tzinfo=timezone.utc), cfg)
        self.assertEqual(fall_at, datetime(2026, 10, 25, 2, 0, tzinfo=timezone.utc))
        self.assertEqual(fall_at.astimezone(tz).hour, 3)

    def test_missed_nights_perform_at_most_one_current_catchup_and_then_dedup(self):
        cfg = ns.ScheduleConfig(enabled=True)
        # Seed state as if 2026-11-20 03:00 CET (02:00 UTC) was handled.
        ns.save_state(
            self.store,
            {
                "schema": 1,
                "last_handled_slot_utc": "2026-11-20T02:00:00Z",
                "last_published_source": "1111" * 10,
                "last_good_manifest": None,
                "attempts_by_source": {},
                "failures": [],
                "last_outcome": None,
            },
        )
        runs = []

        def fake_runner(_cfg, _root, run_ts):
            runs.append(run_ts)
            date_tag = run_ts.astimezone(ZoneInfo("Europe/Amsterdam")).strftime("%Y%m%d")
            return {
                "schema": 1,
                "channel": "nightly",
                "version": f"0.1.0-nightly.{date_tag}.1",
                "build": 11,
                "source": "2222" * 10,
            }

        # Mac wakes up 5 nights later on 2026-11-25 at 09:30 CET (08:30 UTC).
        wake_utc = datetime(2026, 11, 25, 8, 30, tzinfo=timezone.utc)
        outcome = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=wake_utc,
            get_head_sha=lambda: "2222" * 10,
            get_changed_files=lambda _b, _h: ["crates/hex-audio/src/lib.rs"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=fake_runner,
        )
        self.assertEqual(outcome["status"], "published")
        self.assertEqual(outcome["run_reason"], "catchup")
        self.assertEqual(outcome["missed_slots"], 5)
        self.assertEqual(outcome["version"], "0.1.0-nightly.20261125.1")
        self.assertEqual(len(runs), 1, "must perform at most ONE catch-up, never replay all 5 nights")

        # Re-evaluating later the same day is an idempotent no-op.
        second = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 25, 14, 0, tzinfo=timezone.utc),
            get_head_sha=lambda: "2222" * 10,
            get_changed_files=lambda _b, _h: ["crates/hex-audio/src/lib.rs"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=fake_runner,
        )
        self.assertEqual(second["status"], "not_due")
        self.assertEqual(len(runs), 1)

    def test_unchanged_source_and_non_application_source_commits_are_skipped(self):
        cfg = ns.ScheduleConfig(enabled=True)
        prev_sha = "aaaa" * 10
        self._write_channel_manifest(prev_sha)
        runs = []

        # Case 1: identical HEAD SHA -> skipped_unchanged_source.
        out1 = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 26, 3, 0, tzinfo=timezone.utc),
            get_head_sha=lambda: prev_sha,
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=lambda *a: runs.append(a),
        )
        self.assertEqual(out1["status"], "skipped_unchanged_source")
        self.assertEqual(runs, [])

        # Case 2: next night has new commit touching only docs/research -> skipped_unchanged_application_source.
        doc_only_sha = "bbbb" * 10
        out2 = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 27, 3, 0, tzinfo=timezone.utc),
            get_head_sha=lambda: doc_only_sha,
            get_changed_files=lambda _b, _h: ["docs/manual/index.md", "research/notes.md", "README.md"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=lambda *a: runs.append(a),
        )
        self.assertEqual(out2["status"], "skipped_unchanged_application_source")
        self.assertEqual(runs, [])

    def test_simultaneous_manual_release_scheduled_lock_busy_slot_and_unsupported_platform_defer(self):
        cfg = ns.ScheduleConfig(enabled=True)
        now = datetime(2026, 11, 26, 3, 0, tzinfo=timezone.utc)
        runs = []

        # 1. Manual release holds `<store>/.lock` -> visible deferral, no duplicate job.
        manual_lock_fd = os.open(self.store / ".lock", os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(manual_lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            out_manual = ns.run_scheduled_check(
                cfg,
                self.store,
                now_utc=now,
                get_head_sha=lambda: "cccc" * 10,
                platform_preflight=lambda: (True, None),
                cargo_slot_busy_check=lambda: (False, None),
                release_runner=lambda *a: runs.append(a),
            )
            self.assertEqual(out_manual["status"], "deferred_busy")
            self.assertIn("manual release lock", out_manual["reason"])
            self.assertEqual(runs, [])
        finally:
            fcntl.flock(manual_lock_fd, fcntl.LOCK_UN)
            os.close(manual_lock_fd)

        # 2. Concurrent scheduled run holds `.nightly-schedule.lock` -> visible deferral.
        sched_lock_fd = os.open(self.store / ".nightly-schedule.lock", os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(sched_lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            out_sched = ns.run_scheduled_check(
                cfg,
                self.store,
                now_utc=now,
                get_head_sha=lambda: "cccc" * 10,
                platform_preflight=lambda: (True, None),
                cargo_slot_busy_check=lambda: (False, None),
                release_runner=lambda *a: runs.append(a),
            )
            self.assertEqual(out_sched["status"], "deferred_busy")
            self.assertIn("schedule lock", out_sched["reason"])
        finally:
            fcntl.flock(sched_lock_fd, fcntl.LOCK_UN)
            os.close(sched_lock_fd)

        # 3. Busy shared Cargo slot -> visible deferral without killing/stealing slot.
        out_busy = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=now,
            get_head_sha=lambda: "cccc" * 10,
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (True, "shared cargo slot occupied (pid=999)"),
            release_runner=lambda *a: runs.append(a),
        )
        self.assertEqual(out_busy["status"], "deferred_busy")
        self.assertIn("cargo slot", out_busy["reason"])

        # 4. Unsupported/dirty native platform -> visible deferral without weakening checks.
        out_plat = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=now,
            get_head_sha=lambda: "cccc" * 10,
            platform_preflight=lambda: (False, "codesign identity unavailable"),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=lambda *a: runs.append(a),
        )
        self.assertEqual(out_plat["status"], "deferred_unsupported_platform")
        self.assertIn("codesign", out_plat["reason"])
        self.assertEqual(runs, [])

    def test_build_or_signing_failure_retains_history_preserves_last_good_manifest_and_bounds_retries(self):
        cfg = ns.ScheduleConfig(enabled=True, max_retries_per_source=2)
        prev_sha = "1111" * 10
        bad_sha = "dead" * 10
        channel_path, original_bytes = self._write_channel_manifest(prev_sha)
        now = datetime(2026, 11, 26, 3, 0, tzinfo=timezone.utc)
        attempts = []

        def failing_runner(_cfg, _root, _ts):
            attempts.append(len(attempts) + 1)
            raise RuntimeError("codesign/notarization failed; candidate retained")

        # Attempt 1 fails -> retained, retries_remaining == 1, last-good manifest untouched.
        fail1 = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=now,
            get_head_sha=lambda: bad_sha,
            get_changed_files=lambda _b, _h: ["crates/hex-desktop/src/main.rs"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=failing_runner,
        )
        self.assertEqual(fail1["status"], "failed_retained")
        self.assertEqual(fail1["attempt"], 1)
        self.assertEqual(fail1["retries_remaining"], 1)
        self.assertTrue(fail1["last_good_channel_preserved"])
        self.assertEqual(channel_path.read_bytes(), original_bytes)

        # Attempt 2 fails -> retry budget exhausted, slot closed, last-good manifest untouched.
        fail2 = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 26, 4, 0, tzinfo=timezone.utc),
            get_head_sha=lambda: bad_sha,
            get_changed_files=lambda _b, _h: ["crates/hex-desktop/src/main.rs"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=failing_runner,
        )
        self.assertEqual(fail2["status"], "failed_retained")
        self.assertEqual(fail2["attempt"], 2)
        self.assertEqual(fail2["retries_remaining"], 0)
        self.assertEqual(channel_path.read_bytes(), original_bytes)

        # Next night on the same failing commit -> skipped_retry_budget_exhausted (no spin).
        dedup = ns.run_scheduled_check(
            cfg,
            self.store,
            now_utc=datetime(2026, 11, 27, 3, 0, tzinfo=timezone.utc),
            get_head_sha=lambda: bad_sha,
            get_changed_files=lambda _b, _h: ["crates/hex-desktop/src/main.rs"],
            platform_preflight=lambda: (True, None),
            cargo_slot_busy_check=lambda: (False, None),
            release_runner=failing_runner,
        )
        self.assertEqual(dedup["status"], "skipped_retry_budget_exhausted")
        self.assertEqual(len(attempts), 2)
        state = ns.load_state(self.store)
        self.assertEqual(len(state["failures"]), 2)
        self.assertEqual(channel_path.read_bytes(), original_bytes)

    def test_release_command_never_invokes_install_or_app_signals(self):
        cfg = ns.ScheduleConfig(enabled=True, fresh_release_target="/tmp/hex-fresh-target")
        cmd = ns.build_nightly_release_command(cfg)
        self.assertIn("nightly", cmd)
        self.assertNotIn("install", cmd)
        self.assertNotIn("beta", cmd)
        self.assertNotIn("stable", cmd)
        self.assertEqual(cmd[-2:], ["--fresh-release-target", "/tmp/hex-fresh-target"])


if __name__ == "__main__":
    unittest.main()
