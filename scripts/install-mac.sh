#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
id=me.pecheny.sesh.mac
app=build/mac/Build/Products/Release/Sesh.app
running() { [ "$(osascript -e "application id \"$id\" is running")" = true ]; }

xcodebuild -project Sesh.xcodeproj -scheme SeshMac -configuration Release \
  -destination 'platform=macOS,arch=arm64' -derivedDataPath build/mac build | grep -E "error:|BUILD"
was_running=false
if running; then
  was_running=true
  osascript -e "tell application id \"$id\" to quit"
  for _ in $(seq 50); do running || break; sleep 0.2; done
  if running; then echo "Sesh did not quit" >&2; exit 1; fi
fi
ditto "$app" /Applications/Sesh.app
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/Sesh.app
if $was_running; then open /Applications/Sesh.app; fi
