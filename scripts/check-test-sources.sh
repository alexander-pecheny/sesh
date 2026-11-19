#!/bin/bash
# The SeshTests target compiles only the Shared files project.yml lists, so a listed file
# that uses a type from an unlisted one breaks the tests while both apps still build.
set -euo pipefail
cd "$(dirname "$0")/.."
files=()
while IFS= read -r file; do files+=("$file"); done < <(
    awk '/^  SeshTests:/ { t = 1; next } t && /^  [A-Za-z]/ { t = 0 } t && /- path: Shared\// { print $3 }' project.yml)
xcrun swiftc -typecheck -suppress-warnings -sdk "$(xcrun --show-sdk-path --sdk macosx)" "${files[@]}"
