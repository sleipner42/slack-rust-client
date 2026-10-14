#!/bin/bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_dir"
mkdir -p target/dist
stage="$(mktemp -d "$project_dir/target/dmg.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

# Build into a fresh directory: no saved sessions, caches or developer files.
BETTER_SLACK_PROFILE=release BETTER_SLACK_BUNDLE_ID=dev.better-slack.client \
    bash scripts/package-macos.sh "$stage/Better Slack.app"
app="$stage/Better Slack.app"
cp THIRD_PARTY.md "$app/Contents/Resources/Licenses/THIRD_PARTY.md"
codesign --force --sign - --timestamp=none "$app"
codesign --verify --deep --strict "$app"

arch="$(lipo -archs "$app/Contents/MacOS/better-slack")"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
output="${1:-$project_dir/target/dist/Better-Slack-$version-$arch.dmg}"
mkdir -p "$(dirname "$output")"
ln -s /Applications "$stage/Applications"
cat > "$stage/Install.txt" <<'TXT'
Better Slack

Drag Better Slack.app onto Applications, then open it from Applications.

This build is for Apple Silicon Macs (M1 or later), macOS 11 or later.
It is ad-hoc signed and has not been notarized by Apple. If macOS blocks
the app, and you trust its sender, open System Settings > Privacy & Security
and use Open Anyway after attempting to launch it.
Apple's instructions: https://support.apple.com/102445

The installer contains the app only. It does not contain the sender's
Slack session, messages or cache. Each person uses their own workspace session.
TXT
# Keep the temporary image outside its source directory.
temporary="$output.next.dmg"
hdiutil create -volname "Better Slack" -srcfolder "$stage" -fs HFS+ \
    -format UDZO -imagekey zlib-level=9 -ov "$temporary"
hdiutil verify "$temporary"
mv -f "$temporary" "$output"
(
    cd "$(dirname "$output")"
    shasum -a 256 "$(basename "$output")" > "$(basename "$output").sha256"
)
printf 'Created %s\n' "$output"
