udid := "466D3ACF-302A-40B8-8570-719088AF057C"
bundle_id := "me.pecheny.sesh"
derived := "build"

ghosttykit:
    ./scripts/fetch-ghosttykit.sh

gen: ghosttykit
    xcodegen generate --quiet

build: gen
    xcodebuild -project Sesh.xcodeproj -scheme Sesh -configuration Debug \
        -destination 'platform=iOS Simulator,id={{udid}}' \
        -derivedDataPath {{derived}} build | tail -20

run: build
    xcrun simctl install {{udid}} {{derived}}/Build/Products/Debug-iphonesimulator/Sesh.app
    xcrun simctl launch {{udid}} {{bundle_id}}

core:
    @echo "phase 2+"

sshd:
    @echo "phase 2+"

e2e:
    @echo "phase 2+"
