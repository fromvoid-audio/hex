"""Load independent restart and delivery tooling for their own tests."""
import importlib.util
from pathlib import Path
import sys


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parents[1] / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


restart = load("hex_restart_test_support", "launch-restart.py")
delivery = load("hex_delivery_restart_test_support", "hex-test-build.py")
