"""Conservative Python self-test impact, independent of Rust check scope.

Only reviewed app-owned Rust edits have a fast path. Tooling, configuration,
unknown inputs and explicit audits retain complete unittest discovery.
"""
from __future__ import annotations

import os
from pathlib import Path

REVIEWED_MODULES = frozenset({
    "test_cargo_slot", "test_changelog", "test_ci_plan", "test_device_catalog", "test_device_layout",
    "test_eq8_response_compare", "test_gpui_patch", "test_dependency_patches",
    "test_hex_test_build",
    "test_launch", "test_launch_restart", "test_launch_restart_compile",
    # Update entry and process-observer fixtures exercise standalone tooling;
    # updater script and test edits still take comprehensive discovery.
    "test_hex_update_entry", "test_pending_update_exit",
    "test_local_ci", "test_pcm_build_gate", "test_python_test_plan",
    # Read-only target-volume probes depend on CI tooling and mocked OS metadata,
    # not app Rust; changes to the helper/tests retain comprehensive discovery.
    "test_ci_target_storage",
    # Release-ledger fixtures use only their tool and changelog module; edits
    # to those scripts still take the comprehensive unknown/tooling path.
    "test_release_notes",
    # Release/manual fixtures own temporary inputs and mocked native tools; app
    # Rust edits do not affect them. Tooling/docs/config edits remain full checks.
    "test_hex_release", "test_manual",
    "test_stream_build_inputs", "test_terminal_progress", "test_ui_build_identity",
    # Viewer style fixtures exercise the standalone guard with temporary source;
    # changes to the guard itself still retain comprehensive discovery.
    "test_ui_style_log_viewer", "test_ui_style_browser_scrollbar",
    # Architecture fixtures exercise check-architecture.py policy/refusal logic;
    # that guard still runs for app source/doc edits. Policy/test edits fall back.
    "test_architecture",
    # Mach-O/parser and receipt-bound bootstrap fixtures depend on native helper
    # sources/build inputs, not ordinary app Rust. Those inputs fall back below.
    "test_vst3_macos_bootstrap",
    "test_vst3_instance_worker",
})
SHARED_TESTS = (
    "test_ci_plan.ScopeTests.test_current_review_native_build_environment_variants_are_semantic",
    "test_ci_plan.ScopeTests.test_review_target_directory_is_ownership_not_semantic_config",
    "test_ci_plan.ScopeTests.test_known_scopes_keep_prose_and_all_dev_reverse_dependencies",
    "test_ci_plan.ScopeTests.test_high_risk_unknown_and_mixed_cannot_be_scoped_away",
    "test_ci_plan.ScopeTests.test_immutable_rename_deletion_untracked_and_prose_digest",
    "test_ci_plan.ScopeTests.test_profiles_preserve_default_and_feature_configurations",
    "test_ci_plan.ScopeTests.test_runtime_contract_consumers_and_required_configurations",
    "test_ci_plan.ScopeTests.test_requested_zero_match_and_all_skipped_refuse_but_metadata_needs_no_rust",
    "test_ci_plan.CacheIdentityTests.test_source_changes_stay_incremental_but_dependency_config_mode_and_membership_do_not",
)
# Preserve discovery's module names: lifecycle tests import test_local_ci as a
# shared fixture module. Package-qualified aliases load it twice.
UNITTEST_BOOTSTRAP = (
    "import sys, unittest; sys.path.insert(0, 'scripts/tests'); "
    "unittest.main(module=None)"
)
COMPREHENSIVE_ARGV = (
    "python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py",
)


SHARDED_DISCOVERY_ARGV = ("python3", "scripts/python_shards.py", "--discover")


def native_acceptance_inputs(env: dict) -> list[str]:
    """External fixtures/SDKs/signing state require fresh observations.

    Return names only, never paths or credentials. Include future/partial native
    opt-ins conservatively rather than treating an incomplete setup as absent.
    """
    return sorted(key for key, value in env.items() if value and (
        key.startswith("HEX_VST3_") or key == "HEX_TEST_SIGNING_IDENTITY"))


def available_modules(root: Path) -> list[str]:
    tests = root / "scripts/tests"
    # Discovery can recurse into packages. Preserve the path so a nested module
    # cannot disappear behind a reviewed top-level module with the same name.
    return sorted(".".join(path.relative_to(tests).with_suffix("").parts)
                  for path in tests.rglob("test_*.py"))


def select(profile: str, paths: list[str], inventory: list[dict],
           ui_packages: set[str], modules: list[str], *, env: dict | None = None) -> dict:
    """Select from non-prose Git inputs; no test imports or historical cache.

    Cargo inventory establishes ownership, not just a crates/ prefix. The caller
    retains ordinary Rust classification and all its reverse-dependency checks.
    Native opt-ins come from the invoking environment (or an explicit test map);
    their presence and comprehensive selection are recorded in the resolved plan.
    Health narrows only exclusively owned tooling; mixed or unowned inputs
    retain comprehensive Python coverage.
    """
    modules = sorted(set(modules))

    def comprehensive(reason: str, *, native: bool = False) -> dict:
        return {"mode": "comprehensive", "selected_groups": modules,
                "excluded_groups": [], "test_selectors": modules,
                "sharded_discovery": profile == "merge" and not native,
                "reasons": [reason]}

    if profile not in {"merge", "health"}:
        return comprehensive("explicit audit/release or non-merge profile retains full Python coverage")
    if not paths:
        return {"mode": "none", "selected_groups": [], "excluded_groups": modules,
                "test_selectors": [], "reasons": ["prose-only plan retains metadata checks"]}
    native = native_acceptance_inputs(os.environ if env is None else env)
    if native:
        return comprehensive("explicit native acceptance inputs retain full Python coverage: "
                             + ", ".join(native), native=True)
    if set(modules) != REVIEWED_MODULES:
        return comprehensive("new, missing or unregistered Python test module")
    owned = tooling_owner_modules(paths, modules)
    if owned:
        # A shared case inside a selected module already runs with that module.
        shared = [name for name in SHARED_TESTS if name.split(".")[0] not in owned]
        selectors = sorted({*owned, "test_python_test_plan", *shared})
        return {"mode": "shared", "selected_groups": sorted({"shared-invariants", "test_python_test_plan", *owned}),
                "excluded_groups": [name for name in modules if name not in owned and name != "test_python_test_plan"],
                "test_selectors": selectors,
                "reasons": ["tooling owner boundary: each changed script's own test module plus shared "
                            "invariants (user policy, 1 November 2026): " + ", ".join(owned)]}
    if profile == "health":
        return comprehensive("health inputs are not exclusively reviewed tooling owners")
    categories = set()
    for path in paths:
        if not path.endswith(".rs") or path.endswith("/build.rs"):
            return comprehensive("tooling, configuration, build input or unknown path: " + path)
        owners = [(str(Path(package["manifest"]).parent), package["name"])
                  for package in inventory
                  if str(Path(package["manifest"]).parent) != "."
                  and path.startswith(str(Path(package["manifest"]).parent) + "/")]
        if not owners:
            return comprehensive("Rust input has no supported Cargo workspace owner: " + path)
        owner, name = max(owners, key=lambda owner: len(owner[0]))
        relative = path[len(owner) + 1:]
        if not relative.startswith(("src/", "tests/", "examples/", "benches/")):
            return comprehensive("input is outside supported Cargo source directories: " + path)
        categories.add("ui" if name in ui_packages else "core")
    # UI and core Rust inputs reach the same shared invariants; a change touching
    # both needs no unrelated tooling suites either (owner policy, 1 November 2026).
    selectors = [*SHARED_TESTS, "test_python_test_plan"]
    return {"mode": "shared", "selected_groups": ["shared-invariants", "test_python_test_plan"],
            "excluded_groups": [*(name for name in modules
                                   if name not in {"test_ci_plan", "test_python_test_plan"}),
                                "test_ci_plan:remaining-cases"],
            "test_selectors": selectors,
            "reasons": ["ordinary app-only " + "/".join(sorted(categories))
                        + " Rust inputs; shared invariants retained, unaffected tooling suites excluded"]}


# Scripts whose tests live in another module than test_<stem>.
OWNER_TEST_MODULES = {"scripts/test_shards.py": "test_ci_plan",
                      "scripts/python_shards.py": "test_python_test_plan"}


def tooling_owner_modules(paths: list[str], modules: list[str]) -> list[str]:
    """Test modules owning every changed tooling path, or [] when any path is unowned.

    `scripts/NAME.py` (or `scripts/NAME`) is owned by `test_NAME` with hyphens as
    underscores; a changed `scripts/tests/test_X.py` owns itself. Any other
    input, or a script without its own test module, keeps comprehensive discovery.
    """
    owned = set()
    for path in paths:
        if path in OWNER_TEST_MODULES:
            module = OWNER_TEST_MODULES[path]
        elif path.startswith("scripts/tests/test_") and path.endswith(".py") and path.count("/") == 2:
            module = Path(path).stem
        elif path.startswith("scripts/") and path.count("/") == 1 and Path(path).suffix in {".py", ""}:
            module = "test_" + Path(path).stem.replace("-", "_")
        else:
            return []
        if module not in modules:
            return []
        owned.add(module)
    return sorted(owned)


def argv(selection: dict) -> tuple[str, ...]:
    if selection.get("mode") == "comprehensive":
        return SHARDED_DISCOVERY_ARGV if selection.get("sharded_discovery") else COMPREHENSIVE_ARGV
    if selection.get("mode") != "shared":
        raise ValueError("Python test step needs a known nonempty selection")
    selectors = selection.get("test_selectors")
    if not isinstance(selectors, list) or not selectors or any(
            not isinstance(name, str) or not name for name in selectors):
        raise ValueError("Python test selection is empty or malformed")
    # Owner selections run as concurrent single-test processes (Pulse #4001).
    return ("python3", "scripts/python_shards.py", "--", *selectors)
