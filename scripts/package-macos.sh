#!/bin/bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_dir"
app_path="${1:-$project_dir/target/Better Slack.app}"
bundle_id="${BETTER_SLACK_BUNDLE_ID:-dev.better-slack.client}"
build_profile="${BETTER_SLACK_PROFILE:-release}"
binary_profile="$build_profile"
if [[ "$build_profile" == dev ]]; then binary_profile=debug; fi

cargo build --locked --profile "$build_profile"
mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources" target/AppIcon.iconset

# Stage and rename so an existing running process keeps its executable intact.
cp "target/$binary_profile/better-slack" "$app_path/Contents/MacOS/better-slack.next"
mv "$app_path/Contents/MacOS/better-slack.next" "$app_path/Contents/MacOS/better-slack"

for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/app-icon.png --out "target/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
    doubled=$((size * 2))
    sips -z "$doubled" "$doubled" assets/app-icon.png --out "target/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
python3 - "$app_path" "$bundle_id" <<'PY'
from pathlib import Path
import plistlib
import struct
import sys

app = Path(sys.argv[1])
# ICNS supports PNG payloads directly, preserving the generated alpha at every scale.
representations = {
    'icp4': '16x16', 'icp5': '32x32', 'icp6': '32x32@2x',
    'ic07': '128x128', 'ic08': '256x256', 'ic09': '512x512',
    'ic10': '512x512@2x', 'ic11': '16x16@2x',
    'ic12': '32x32@2x', 'ic13': '128x128@2x', 'ic14': '256x256@2x',
}
licenses = app / 'Contents/Resources/Licenses'
licenses.mkdir(exist_ok=True)
(licenses / 'Ubuntu-UFL.txt').write_text(Path('assets/fonts/UFL.txt').read_text())
chunks = []
for kind, size in representations.items():
    data = Path(f'target/AppIcon.iconset/icon_{size}.png').read_bytes()
    chunks.append(kind.encode('ascii') + struct.pack('>I', len(data) + 8) + data)
payload = b''.join(chunks)
(app / 'Contents/Resources/AppIcon.icns').write_bytes(b'icns' + struct.pack('>I', len(payload) + 8) + payload)
info = {
    'CFBundleExecutable': 'better-slack',
    'CFBundleIdentifier': sys.argv[2],
    'CFBundleName': app.stem,
    'CFBundleDisplayName': app.stem,
    'CFBundleIconFile': 'AppIcon',
    'CFBundlePackageType': 'APPL',
    'CFBundleShortVersionString': '0.1.0',
    'CFBundleVersion': '1',
    'LSMinimumSystemVersion': '11.0',
    'NSHighResolutionCapable': True,
}
with (app / 'Contents/Info.plist').open('wb') as file:
    plistlib.dump(info, file)
print(f'Built {app}')
PY
