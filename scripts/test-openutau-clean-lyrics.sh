#!/usr/bin/env bash
set -euo pipefail

readonly REVISION="3f213e8993ca792c3e6f8958c92ab27eae78eac5"
readonly REPOSITORY="https://github.com/openutau/OpenUtau.git"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly PATCH="$ROOT/compat/openutau/clean-note-labels.patch"
readonly WORK_RAW="$(mktemp -d "${TMPDIR:-/tmp}/verse-openutau-clean-lyrics.XXXXXX")"
readonly WORK="$(cd "$WORK_RAW" && pwd -P)"

cleanup() {
  rm -rf "$WORK_RAW"
}
trap cleanup EXIT

git -C "$WORK" init -q
git -C "$WORK" remote add origin "$REPOSITORY"
git -C "$WORK" fetch -q --depth 1 origin "$REVISION"
git -C "$WORK" checkout -q --detach FETCH_HEAD
test "$(git -C "$WORK" rev-parse HEAD)" = "$REVISION"
git -C "$WORK" apply --check --ignore-space-change "$PATCH"
git -C "$WORK" apply --ignore-space-change "$PATCH"
dotnet test "$WORK/OpenUtau.Test/OpenUtau.Test.csproj" \
  --filter FullyQualifiedName~NoteLyricDisplayTest \
  --disable-build-servers \
  -p:EmbeddedResourceUseDependentUponConvention=false
