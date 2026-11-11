#!/bin/sh
# Run the optimized desktop from the checkout already updated by launch.
set -e
if [ "$(uname -s):$(uname -m)" = Darwin:arm64 ]; then
    HEX_LAUNCH_DISCOVERY_APP=$(python3 scripts/prepare-launch-discovery.py) || HEX_LAUNCH_DISCOVERY_APP=
    export HEX_LAUNCH_DISCOVERY_APP
    HEX_LAUNCH_INSTANCE_APP=$(python3 scripts/prepare-launch-discovery.py --instance) || HEX_LAUNCH_INSTANCE_APP=
    export HEX_LAUNCH_INSTANCE_APP
fi
exec cargo run -r -- "$@"
