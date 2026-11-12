#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

bundle="$PWD/src-tauri/target/aarch64-apple-darwin/release/bundle"
app="$bundle/macos/视频转码.app"
if [[ ! -d "$app" ]]; then
  echo 'Build the macOS application bundle before creating the DMG.' >&2
  exit 1
fi
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
staging="$(mktemp -d "${TMPDIR:-/tmp}/video-trans-dmg.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/volume" "$staging/image"
ditto "$app" "$staging/volume/视频转码.app"
ln -s /Applications "$staging/volume/Applications"

mkdir -p "$bundle/dmg"
name="视频转码_${version}_aarch64.dmg"
hdiutil create -quiet -fs HFS+ -format UDZO -volname "视频转码" \
  -srcfolder "$staging/volume" "$staging/image/$name"
hdiutil verify "$staging/image/$name"
mv -f "$staging/image/$name" "$bundle/dmg/$name"
printf '%s\n' "$bundle/dmg/$name"
