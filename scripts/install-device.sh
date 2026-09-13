#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
device="${1:-552C8E6E-4E97-5AA8-B96B-2A014C5414EC}"
app=build/Build/Products/Release-iphoneos/Sesh.app

xcodebuild -project Sesh.xcodeproj -scheme Sesh -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath build \
  -allowProvisioningUpdates build | grep -E "error:|BUILD"
xcrun devicectl device install app --device "$device" "$app"
xcrun devicectl device process launch --device "$device" me.pecheny.sesh
