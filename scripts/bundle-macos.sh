#!/bin/sh
# Local development bundle only; no signing, installation, or publication.
set -eu
cd "$(dirname "$0")/.."

# Never replace the development executable while it may own an unsaved project.
# macOS filesystems commonly ignore case (Hex.app and hex.app are one bundle).
assert_bundle_inactive() {
python3 - <<'PY'
from pathlib import Path
import subprocess
import sys

executable_directory = str((Path.cwd() / 'artifacts/hex.app/Contents/MacOS').resolve()).casefold() + '/'
processes = subprocess.run(
    ['ps', '-axo', 'pid=,comm='], check=True, capture_output=True, text=True
)
for line in processes.stdout.splitlines():
    fields = line.strip().split(None, 1)
    if len(fields) != 2:
        continue
    pid, executable = fields
    if str(Path(executable).resolve()).casefold().startswith(executable_directory):
        print(f'refusing to replace running hex development bundle (pid {pid}); '
              'close it first or prepare a separate review bundle', file=sys.stderr)
        sys.exit(1)
PY
}

assert_bundle_inactive
cargo build -p hex-desktop
assert_bundle_inactive
bundle=artifacts/hex.app/Contents
mkdir -p "$bundle/MacOS" "$bundle/Resources/licenses"
cp target/debug/hex-desktop "$bundle/MacOS/hex"
cp assets/fonts/ioskeley-mono/OFL.txt "$bundle/Resources/licenses/ioskeley-mono-OFL.txt"
cp third_party/gpui/LICENSE-APACHE "$bundle/Resources/licenses/gpui-LICENSE-APACHE"
cat > "$bundle/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>hex</string>
<key>CFBundleIdentifier</key><string>org.portalsurfer.hex.dev</string>
<key>CFBundleName</key><string>hex</string>
<key>CFBundleDisplayName</key><string>hex</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
printf '%s\n' "Local development app: $(pwd)/artifacts/hex.app"
