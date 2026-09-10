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
