#!/bin/sh
# Read-only bundle inventory; optional first argument overrides the reference path.
set -eu
app=${1:-/Applications/Ableton Live 12 Suite.app}
plist="$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist"
/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist"
executable=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist")
binary="$app/Contents/MacOS/$executable"
file "$binary"
shasum -a 256 "$binary"
otool -arch arm64 -L "$binary"
