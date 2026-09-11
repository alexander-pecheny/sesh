#!/usr/bin/env bash
# Xcode has no signed-in account here, so a managed profile cannot be used through
# xcodebuild; build unsigned and sign the bundle with the development certificate.
set -euo pipefail
cd "$(dirname "$0")/.."
device="${1:-552C8E6E-4E97-5AA8-B96B-2A014C5414EC}"
app=build/Build/Products/Release-iphoneos/Sesh.app
profile="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles/2574e594-106a-45f0-a3dd-f17abcb0e8bd.mobileprovision"

xcodebuild -project Sesh.xcodeproj -scheme Sesh -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath build \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO build | grep -E "error:|BUILD"
cp "$profile" "$app/embedded.mobileprovision"
cat > .local/device.entitlements <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>B5T934YFU5.me.pecheny.sesh</string>
  <key>com.apple.developer.team-identifier</key><string>B5T934YFU5</string>
  <key>get-task-allow</key><true/>
  <key>keychain-access-groups</key><array><string>B5T934YFU5.*</string></array>
</dict></plist>
PLIST
codesign -f -s "Apple Development: ap@pecheny.me (9JTT8XLTCV)" --entitlements .local/device.entitlements "$app"
xcrun devicectl device install app --device "$device" "$app"
xcrun devicectl device process launch --device "$device" me.pecheny.sesh
