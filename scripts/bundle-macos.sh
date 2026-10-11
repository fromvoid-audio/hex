#!/bin/sh
# Standard optimized native test delivery; no app launch or public release.
set -eu
exec python3 "$(dirname "$0")/hex-test-build.py" "$@"
