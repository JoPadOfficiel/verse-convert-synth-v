# Readable OpenUtau note captions

`clean-note-labels.patch` changes the native OpenUtau piano-roll caption renderer
so `rêves[fr/r fr/ae fr/v]` displays as `rêves`. The phoneme lane continues to
display the resolved phonemes. The full stored lyric remains available in the
lyric editor, in saved USTX files, and to the phonemizer. The same display rule
applies to English hints.

This is an **optional, local OpenUtau build**, not a new USTX field or a change
that a Verse export can activate in stock OpenUtau. An unmodified OpenUtau build
still displays its full stored lyric. Verse keeps the standard inline hints
because removing them would discard its explicit pronunciation corrections.

## Pinned consumer contract

The patch targets OpenUtau revision
`3f213e8993ca792c3e6f8958c92ab27eae78eac5`, the revision identified by the installed
consumer during verification. Its source establishes the relevant boundaries:

- `OpenUtau.Core/Ustx/UNote.cs:118`: `ToPhonemizerNote` extracts word-level hints
  from the stored lyric using the greedy, line-local `\[(.*)\]` pattern.
- `OpenUtau.Core/DiffSinger/DiffSingerBasePhonemizer.cs:169`: `GetSymbols` gives
  the extracted hint priority over dictionary lookup.
- `OpenUtau/Controls/NotesCanvas.cs:648`: the caption previously drew the
  complete stored lyric. This is the changed rendering boundary.

`phoneme_overrides` applies to already generated phonemes. It is not a separate
serialized word-level hint and cannot replace the input needed for syllable
allocation. No voicebank dictionary, source note, or synthesis setting is
changed by this patch.

The bounded compatibility sidecar also qualifies exact 0.1.571-beta revision
`ec7ba520583173c67aabfc5feab33390b4f720a4`. Its caption boundary is unchanged apart
from the four-line source offset: `clean-note-labels-beta.patch` explicitly
targets that revision. The CLI maps each complete SHA to its own caption patch;
it never applies the baseline patch speculatively to a tag, branch or alpha.
`NativeCompatibilityTest.cs` runs on both revisions, and
`BetaCompatibilityTest.cs` is copied only into the exact beta checkout.

The beta's primary source sets `Ustx.kUstxVersion` to `0.10`, introduces
`UExpressionType.MaskedCurve`, and stores expression graphs in `UProject` with
optional per-renderer defaults and track overrides. `UPart.AfterLoad` resolves
ordinary curves through track/project descriptors and removes undeclared ones.
Tests preserve declared input expressions, curves and explicit masked values;
they do not treat unknown expressions as safely preserved, generate graph
automation, render RPIT data, install banks or models, or change renderers.
Missing graph libraries/defaults remain absent across native load/save/reload.
No nonempty user graph evaluation or daily alpha compatibility is claimed.

The gate applies `isolated-test-paths.patch` only to its disposable test host,
before native preferences/cache initialization. This patch is **not** a consumer
feature and must never be used in an application build. Native USTX Save still
runs its normal code, including isolated preference writes. See
[`docs/testing.md`](../../docs/testing.md) for immutable selection, model-free
native mode, external Verse fixtures and retained TRX receipts.

Full and fixtures-only receipts require the exact 46-case
`NativePronunciationTest` inventory, including theory arguments, plus all native
caption/compatibility cases: 68 total on the baseline and 70 on the beta.
Missing, duplicated, substituted or skipped cases fail qualification. Recheck
the validator without rebuilding either consumer using qualified full receipts:

```sh
python3 compat/openutau/test_receipt_validator.py \
  --baseline-trx /path/to/baseline-full/native-compatibility.trx \
  --beta-trx /path/to/beta-full/native-compatibility.trx
```

This exercises the production validator with `PYTHONOPTIMIZE=1`, including the
regression where 26 pronunciation results are deleted while all formerly
required minimum checks still match. It does not change the source receipts.

## Display behavior

The helper follows the same bracket syntax as the native phonemizer. It retains
the existing caption when no complete hint is present. For a phoneme-only lyric,
it retains that input as a visible caption instead of creating a blank note.
Holds, splits, unclosed brackets and line breaks remain valid. The regex uses
the .NET non-backtracking engine to keep repeated painting bounded for malformed
input. The patch adds positive and negative tests for these cases.

## Reproduce the local build

In a separate checkout of the exact revision above, apply the patch using its
absolute path. Do not apply it to an installed application:

```sh
git apply --check --ignore-space-change /absolute/path/to/verse/compat/openutau/clean-note-labels.patch
git apply --ignore-space-change /absolute/path/to/verse/compat/openutau/clean-note-labels.patch
dotnet test OpenUtau.Test/OpenUtau.Test.csproj \
  --filter FullyQualifiedName~NoteLyricDisplayTest --disable-build-servers
```

The whitespace option handles the pinned source's CRLF context lines while the
reviewable patch uses LF. Applying it to the pristine archive was checked to
produce byte-for-byte the files used for the build.

Use the revision's own macOS build procedure with a separate output directory:

```sh
dotnet msbuild OpenUtau/OpenUtau.csproj -restore -t:BundleApp \
  -p:Configuration=Release -p:RuntimeIdentifier=osx-arm64 \
  -p:UseAppHost=true -p:SelfContained=true \
  -p:Version=0.1.569.1 -p:CFBundleVersion=0.1.569.1 \
  -p:CFBundleShortVersionString=0.1.569.1 \
  -p:CFBundleIdentifier=com.verse.openutau-clean-lyrics \
  '-p:CFBundleDisplayName=OpenUtau Clean Lyrics' \
  -p:OutputPath=/absolute/path/to/new-output/ -m:2
```

`0.1.569.1` identifies this local build; it is not an upstream release claim.
Retain OpenUtau's MIT license when distributing the local artifact. Local
ad-hoc signing does not establish Developer ID signing or notarization. Keep the
existing installed application and open edited projects intact; open a copied
project explicitly with the separately named build.

Release evidence must cover actual native captions and phonemes, plus stored
lyric/geometry preservation. Passing only the helper tests does not prove the
complete editor experience. Invocation receipts, patch/source hashes and local
artifacts are recorded in the task's ignored implementation-artifact directory;
private scores, renders and generated application bundles do not belong in Git.

## French word editing and schema recovery

On the full word head, choose `DiffSinger French Millefeuille Phonemizer`, reset
stale generated alias substitutions (right-click the alias text), and regenerate
or supply the complete supported hint. Native regressions cover
`ciel[fr/s fr/y fr/ae fr/l]`, `yeux[fr/y fr/ee]`,
`blancs[fr/b fr/l fr/en]` and `noel[fr/n fr/oo fr/ae fr/l]`.
They reject `fr/i` and `fr/el`. Preserve `+` syllables and `+~` holds.
Changing an alias prefix alone cannot select the word language or translate phones.
The French tests use synthetic untrained duration graphs and verify native
SetSinger/SetUp/Process and acoustic token consumption; they make no listening claim.

The pinned 0.1.569 consumer supports USTX 0.9; the exact qualified 0.1.571-beta
supports 0.10 and `MaskedCurve`. Deserialization precedes the native version
check, so opening a newer schema with the baseline
can surface as `Exception during deserialization`. Application labels alone do
not prove schema support. Preserve edited projects in new recovery copies and
reject active or unexplained newer-schema values. Verify the exact consumer's
load/save/reload and resolved WAV references independently of pronunciation.
