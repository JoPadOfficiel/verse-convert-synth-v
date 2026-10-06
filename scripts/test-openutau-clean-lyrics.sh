#!/usr/bin/env bash
set -euo pipefail

readonly BASELINE="3f213e8993ca792c3e6f8958c92ab27eae78eac5"
readonly BETA="ec7ba520583173c67aabfc5feab33390b4f720a4"
readonly REPOSITORY="https://github.com/openutau/OpenUtau.git"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REVISION="$BASELINE"
MODE="full"
SOURCE_CHECKOUT=""
RESULTS=""
usage() {
  cat <<EOF
Usage: $0 [--revision SHA] [--native-only | --fixtures-only] [--source-checkout PATH] [--results PATH]
Immutable consumers only: $BASELINE (0.1.569), $BETA (0.1.571-beta).
Default full mode generates Verse fixtures with Cargo, then runs native tests.
--native-only runs load/save/clock/ownership/curves and captions without Cargo or models.
--fixtures-only also runs existing pronunciation tests using both VERSE_OPENUTAU_*_FIXTURE inputs.
--source-checkout fetches the exact commit from a local Git checkout, without remote access.
--results retains TRX and qualification.txt in a new directory; no existing result is replaced.
EOF
}
while (($#)); do
  case "$1" in
    --revision|--source-checkout|--results)
      if (($# < 2)) || [[ -z "$2" ]]; then usage >&2; exit 2; fi
      case "$1" in
        --revision) REVISION="$2" ;;
        --source-checkout) SOURCE_CHECKOUT="$2" ;;
        --results) RESULTS="$2" ;;
      esac
      shift 2 ;;
    --native-only|--fixtures-only)
      if [[ "$MODE" != full ]]; then usage >&2; exit 2; fi
      MODE="${1#--}"
      shift ;;
    --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
case "$REVISION" in
  "$BASELINE") PATCH="$ROOT/compat/openutau/clean-note-labels.patch" ;;
  "$BETA") PATCH="$ROOT/compat/openutau/clean-note-labels-beta.patch" ;;
  *) echo "Unsupported OpenUtau revision: $REVISION" >&2; exit 2 ;;
esac
if [[ "$MODE" == fixtures-only ]]; then
  test -s "${VERSE_OPENUTAU_EXACT_HINT_FIXTURE:?Provide the Verse-generated TSV}"
  test -s "${VERSE_OPENUTAU_PRONUNCIATION_FIXTURE:?Provide the Verse-generated USTX}"
  # Convert caller-relative fixture paths before entering the consumer directory.
  VERSE_OPENUTAU_EXACT_HINT_FIXTURE="$(cd "$(dirname "$VERSE_OPENUTAU_EXACT_HINT_FIXTURE")" && pwd)/$(basename "$VERSE_OPENUTAU_EXACT_HINT_FIXTURE")"
  VERSE_OPENUTAU_PRONUNCIATION_FIXTURE="$(cd "$(dirname "$VERSE_OPENUTAU_PRONUNCIATION_FIXTURE")" && pwd)/$(basename "$VERSE_OPENUTAU_PRONUNCIATION_FIXTURE")"
  export VERSE_OPENUTAU_EXACT_HINT_FIXTURE VERSE_OPENUTAU_PRONUNCIATION_FIXTURE
fi
if [[ -n "$SOURCE_CHECKOUT" ]]; then
  SOURCE_CHECKOUT="$(cd "$SOURCE_CHECKOUT" && pwd -P)"
  git -C "$SOURCE_CHECKOUT" cat-file -e "$REVISION^{commit}"
fi
if [[ -n "$RESULTS" ]]; then
  mkdir "$RESULTS"
  RESULTS="$(cd "$RESULTS" && pwd -P)"
fi
readonly WORK_RAW="$(mktemp -d "${TMPDIR:-/tmp}/verse-openutau-clean-lyrics.XXXXXX")"
readonly WORK="$(cd "$WORK_RAW" && pwd -P)"
cleanup() { rm -rf "$WORK_RAW"; }
trap cleanup EXIT

# Snapshot the actual patch inputs; receipts must not hash later repository edits.
cp "$PATCH" "$WORK/verse-caption.patch"
PATCH="$WORK/verse-caption.patch"
cp "$ROOT/compat/openutau/isolated-test-paths.patch" "$WORK/verse-isolation.patch"
if [[ "$MODE" == fixtures-only ]]; then
  cp "$VERSE_OPENUTAU_EXACT_HINT_FIXTURE" "$WORK/verse-exact-mfa-hints.tsv"
  cp "$VERSE_OPENUTAU_PRONUNCIATION_FIXTURE" "$WORK/verse-four-languages.ustx"
  export VERSE_OPENUTAU_EXACT_HINT_FIXTURE="$WORK/verse-exact-mfa-hints.tsv"
  export VERSE_OPENUTAU_PRONUNCIATION_FIXTURE="$WORK/verse-four-languages.ustx"
fi

git -C "$WORK" init -q
git -C "$WORK" remote add origin "${SOURCE_CHECKOUT:-$REPOSITORY}"
git -C "$WORK" fetch -q --depth 1 origin "$REVISION"
git -C "$WORK" checkout -q --detach FETCH_HEAD
test "$(git -C "$WORK" rev-parse HEAD)" = "$REVISION"
echo "Qualifying OpenUtau $REVISION ($MODE)"
grep -Fq '[YamlMember(Alias = "phonemizer", ApplyNamingConventions = false)]' \
  "$WORK/OpenUtau.Core/Ustx/UNote.cs"
grep -Fq 'public string? PhonemizerOverride { get; set; } = null;' \
  "$WORK/OpenUtau.Core/Ustx/UNote.cs"
grep -Fq 'if (!string.IsNullOrEmpty(note.PhonemizerOverride)) {' \
  "$WORK/OpenUtau.Core/Ustx/UPart.cs"
grep -Fq 'PhonemizerFactory.GetAll().FirstOrDefault(f => f.name == note.PhonemizerOverride)' \
  "$WORK/OpenUtau.Core/Ustx/UPart.cs"
# Each caption patch is explicitly mapped to its reviewed immutable source.
# The second patch redirects test-host paths before any native user-data access.
for patch in "$PATCH" "$WORK/verse-isolation.patch"; do
  git -C "$WORK" apply --check --ignore-space-change "$patch"
  git -C "$WORK" apply --ignore-space-change "$patch"
done
cp "$ROOT/compat/openutau/NativeCompatibilityTest.cs" "$WORK/OpenUtau.Test/App/NativeCompatibilityTest.cs"
if [[ "$REVISION" == "$BETA" ]]; then
  cp "$ROOT/compat/openutau/BetaCompatibilityTest.cs" "$WORK/OpenUtau.Test/App/BetaCompatibilityTest.cs"
fi
# Ustx.Save writes native preferences: keep all test-host data in this disposable root.
export VERSE_OPENUTAU_TEST_ROOT="$WORK/test-state"
export TESTINGPLATFORM_TELEMETRY_OPTOUT=1 DOTNET_CLI_TELEMETRY_OPTOUT=1
FILTER='FullyQualifiedName~NoteLyricDisplayTest|FullyQualifiedName~NativeCompatibilityTest|FullyQualifiedName~BetaCompatibilityTest'
if [[ "$MODE" != native-only ]]; then
  cp "$ROOT/compat/openutau/NativePronunciationTest.cs" "$WORK/OpenUtau.Test/App/NativePronunciationTest.cs"
  FILTER="$FILTER|FullyQualifiedName~NativePronunciationTest"
fi
if [[ "$MODE" == full ]]; then
  export VERSE_OPENUTAU_EXACT_HINT_FIXTURE="$WORK/verse-exact-mfa-hints.tsv"
  # Rust applies the pinned lexical evidence; native tests consume those results
  # with a synthetic singer and embedded, untrained ONNX timing scaffolds.
  # This verifies symbols and inventory failures, never acoustic quality. The stock
  # consumer has no Spanish+ context rewrite or exact missing-phone substitution.
  cargo test --manifest-path "$ROOT/src-tauri/Cargo.toml" --locked --lib \
    engine::target::diffsinger::tests::
  test -s "$VERSE_OPENUTAU_EXACT_HINT_FIXTURE"
  export VERSE_OPENUTAU_PRONUNCIATION_FIXTURE="$WORK/verse-four-languages.ustx"
  cargo test --manifest-path "$ROOT/src-tauri/Cargo.toml" --locked \
    --test mixed_languages four_language_routing_preserves_source_notes_across_all_adapters -- --exact
fi
if [[ -z "$RESULTS" ]]; then
  RESULTS="$WORK/results"
  mkdir "$RESULTS"
fi
RESULT_ARGS=(--logger 'trx;LogFileName=native-compatibility.trx' --results-directory "$RESULTS")
RUN_ARGS=("$WORK/OpenUtau.Test/OpenUtau.Test.csproj" --filter "$FILTER" --disable-build-servers)
if [[ "$REVISION" == "$BETA" ]]; then
  # The exact beta moved to xUnit v3 4.0 / MTP v2. Do not reuse VSTest flags.
  printf '%s\n' '{"test":{"runner":"Microsoft.Testing.Platform"}}' > "$WORK/global.json"
  CLASSES=(OpenUtau.App.NoteLyricDisplayTest OpenUtau.App.NativeCompatibilityTest OpenUtau.App.BetaCompatibilityTest)
  if [[ "$MODE" != native-only ]]; then CLASSES+=(OpenUtau.App.NativePronunciationTest); fi
  RUN_ARGS=(--project "$WORK/OpenUtau.Test/OpenUtau.Test.csproj" --filter-class "${CLASSES[@]}")
  RESULT_ARGS=(--report-xunit-trx --report-xunit-trx-filename native-compatibility.trx --results-directory "$RESULTS")
fi
if [[ -n "$RESULTS" ]]; then
  {
    echo "consumer_revision=$REVISION"
    echo "consumer_tree=$(git -C "$WORK" rev-parse 'HEAD^{tree}')"
    echo "mode=$MODE"
    echo "caption_patch_blob=$(git -C "$WORK" hash-object "$PATCH")"
    echo "isolation_patch_blob=$(git -C "$WORK" hash-object "$WORK/verse-isolation.patch")"
    echo "common_tests_blob=$(git -C "$WORK" hash-object "$WORK/OpenUtau.Test/App/NativeCompatibilityTest.cs")"
    if [[ "$REVISION" == "$BETA" ]]; then
      echo "beta_tests_blob=$(git -C "$WORK" hash-object "$WORK/OpenUtau.Test/App/BetaCompatibilityTest.cs")"
    fi
    if [[ "$MODE" != native-only ]]; then
      echo "pronunciation_tests_blob=$(git -C "$WORK" hash-object "$WORK/OpenUtau.Test/App/NativePronunciationTest.cs")"
      echo "hint_fixture_blob=$(git -C "$WORK" hash-object "$VERSE_OPENUTAU_EXACT_HINT_FIXTURE")"
      echo "project_fixture_blob=$(git -C "$WORK" hash-object "$VERSE_OPENUTAU_PRONUNCIATION_FIXTURE")"
    fi
    echo "installed_application_used=false"
    echo "acoustic_render=false"
    echo "daily_alpha_tested=false"
    echo "status=started"
  } > "$RESULTS/qualification.txt"
fi
# Run from the disposable checkout so native relative-path writes stay isolated.
cd "$WORK"
if dotnet test "${RUN_ARGS[@]}" \
  -p:EmbeddedResourceUseDependentUponConvention=false "${RESULT_ARGS[@]}"; then
  # A successful process with zero/missing/skipped selected tests is not qualification.
  if python3 -I - "$RESULTS/native-compatibility.trx" "$REVISION" "$MODE" <<'PY_RECEIPT'
import sys
import json
import xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
ns = {"t": "http://microsoft.com/schemas/VisualStudio/TeamTest/2010"}
results = root.findall(".//t:UnitTestResult", ns)
assert results and all(r.get("outcome") == "Passed" for r in results), "Incomplete native receipt"
names = [r.get("testName", "") for r in results]
for method in ("NativeCanvasCallsReadableCaptionHelper", "RepeatedUnclosedBracketsRemainVisible", "Ustx06LoadSaveReloadPreservesSourceOwnershipHintsAndCurves"):
    assert any(method in name for name in names), "Missing required native regression: " + method
assert sum("ActualNativeLyricEditUndoAndSaveLeaveExplicitClockInvariant" in name for name in names) == 2
if sys.argv[2] == "ec7ba520583173c67aabfc5feab33390b4f720a4":
    for method in ("OldSourceWithoutGraphsKeepsNativeDefaultsWithoutInventedAutomation", "SuppliedMaskedValuesAndHolesSurviveNativeLyricEditAndLoadSaveReload"):
        assert any(method in name for name in names), "Missing beta regression: " + method
native_names = [name for name in names if "NativePronunciationTest." not in name]
assert len(native_names) == (24 if sys.argv[2] == "ec7ba520583173c67aabfc5feab33390b4f720a4" else 22), "Unexpected native test count"
assert len(set(native_names)) == len(native_names), "Duplicate native regression receipt"
assert sum("ReadableCaptionPreservesNativeHintSyntax" in name for name in names) == 17, "Missing caption cases"
if sys.argv[3] != "native-only":
    assert any("VerseExportResolvesEveryNativeWordOverride" in name for name in names), "Missing Verse-generated USTX regression"
    assert sum("ExactMfaHintReachesNativeProcess" in name for name in names) == 16, "Missing Verse-generated hint cases"
    for method in ("MissingDurationPhoneRaisesNativeErrorWithoutApproximateFallback", "MissingAcousticPhoneFailsAfterSuccessfulNativePhonemization", "FrenchWholeWordHintSurvivesNativeLifecycle"):
        assert any(method in name for name in names), "Missing existing pronunciation regression: " + method
    # Both pinned runners emit the same complete case names. Count alone cannot
    # distinguish a missing case from a duplicate or a different theory argument.
    prefix = "OpenUtau.App.NativePronunciationTest."
    expected = set()
    def case(method, **arguments):
        values = [key + ": " + (str(value) if isinstance(value, bool) else json.dumps(value))
                  for key, value in arguments.items()]
        expected.add(prefix + method + ("(" + ", ".join(values) + ")" if values else ""))
    hints = (
        ("es", "boca", "b o k a"), ("es", "sabe", "s a B e"),
        ("es", "dame", "d a m e"), ("es", "cada", "k a D a"),
        ("es", "gato", "g a t o"), ("es", "lago", "l a G o"),
        ("es", "dedo", "d e D o"), ("pt", "acho", "a S u"),
    )
    for language, word, hint in hints:
        for prefixed in (False, True):
            arguments = dict(language=language, word=word, hint=hint, prefixed=prefixed)
            case("ExactMfaHintReachesNativeProcess", **arguments)
            missing = next((phone for phone in hint.split() if phone in ("B", "D", "G", "S")), None)
            if missing is not None and word != "dedo":
                for method in ("MissingDurationPhoneRaisesNativeErrorWithoutApproximateFallback",
                               "MissingAcousticPhoneFailsAfterSuccessfulNativePhonemization"):
                    case(method, **arguments, missing=missing)
    for word, hint in (("ciel", "fr/s fr/y fr/ae fr/l"), ("yeux", "fr/y fr/ee"),
                       ("blancs", "fr/b fr/l fr/en"), ("noel", "fr/n fr/oo fr/ae fr/l")):
        case("FrenchWholeWordHintSurvivesNativeLifecycle", word=word, hint=hint)
    for alias in ("fr/i", "fr/el"):
        case("InvalidFrenchAliasIsRefused", alias=alias)
    for language, word, hint, phonemizer in (
        ("fr", "ciel", "fr/s fr/y fr/ae fr/l", "DiffSinger French Millefeuille Phonemizer"),
        ("pt", "acho", "a S u", "DiffSinger Portuguese Phonemizer"),
        ("en", "read", "en/r en/iy en/d", "DiffSinger English Phonemizer"),
    ):
        case("ConfirmedReadingRoundTripPreservesNativeSymbolsAndMusicalFields",
             language=language, word=word, hint=hint, phonemizer=phonemizer)
    case("PortugueseExactHintSurvivesNativeParsingAndRoundTrip")
    case("VerseExportResolvesEveryNativeWordOverride")
    pronunciation_names = [name for name in names if name.startswith(prefix)]
    assert len(expected) == 43
    assert len(pronunciation_names) == 43 and set(pronunciation_names) == expected, "Incomplete or substituted native pronunciation case set"
    assert len(results) == len(native_names) + 43, "Unexpected full native receipt count"
print(f"Qualified {sys.argv[2]}: {len(results)} passed native tests")
PY_RECEIPT
  then
    echo "status=passed" >> "$RESULTS/qualification.txt"
  else
    echo "status=failed" >> "$RESULTS/qualification.txt"
    exit 1
  fi
else
  if [[ -n "$RESULTS" ]]; then echo "status=failed" >> "$RESULTS/qualification.txt"; fi
  exit 1
fi
