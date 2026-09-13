udid := "466D3ACF-302A-40B8-8570-719088AF057C"
bundle_id := "me.pecheny.sesh"
derived := "build"

# The published prebuilt is macOS-only in practice; see scripts/build-ghosttykit.sh.
ghosttykit:
    ./scripts/build-ghosttykit.sh

ghosttykit-prebuilt:
    ./scripts/fetch-ghosttykit.sh

gen: ghosttykit core
    xcodegen generate --quiet

build: gen
    xcodebuild -project Sesh.xcodeproj -scheme Sesh -configuration Debug \
        -destination 'platform=iOS Simulator,id={{udid}}' \
        -derivedDataPath {{derived}} build | tail -20

run: build
    xcrun simctl install {{udid}} {{derived}}/Build/Products/Debug-iphonesimulator/Sesh.app
    xcrun simctl launch {{udid}} {{bundle_id}}

core:
    ./scripts/build-core.sh

# Render every app-icon variant to build/icons; `just icon NAME` installs one.
icons:
    uv run scripts/appicon.py --sheet

icon name:
    uv run scripts/appicon.py --install {{name}}

core-test:
    cd core && cargo test

sshd:
    ./scripts/local-sshd.sh

sshd-stop:
    ./scripts/local-sshd.sh stop

e2e: build
    uv run scripts/e2e.py

# Build, sign and install on the paired iPhone (see scripts/install-device.sh).
device:
    ./scripts/install-device.sh
