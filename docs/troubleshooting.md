# Troubleshooting

Start by identifying which output you opened:

- **Vocal-only SVP** contains only editable vocal notes. It intentionally has
  no piano, percussion, instrument stems, or full-score audio.
- **Complete bundle project** is the `.svp` under
  `<name>.versebundle/project/`. It contains editable vocals, one audio-backed
  instrumental track per note-bearing source Part, and a muted full-score
  reference.

Verse never overwrites an export. Re-testing after an upgrade therefore
requires a new destination name. Opening an older SVP or older bundle is a
common source of misleading results.

## No singing, a beep, or “No default voice”

This is normally Synthesizer V voice configuration, not a conversion defect,
when the vocal notes and their lyric text are visible in the piano roll.

Verse writes vocal tracks without selecting or bundling a commercial voice
database. In Synthesizer V:

1. select the vocal track;
2. assign an installed voice database;
3. select the matching singing language supported by that voice;
4. regenerate or play the track.

The English/French choice in Verse records the intended vocal language. It
does not install a voice, translate lyrics, or guarantee that the selected
Synthesizer V database supports that language.

Synthesizer V itself is not required to analyze a source or write an SVP. It is
required to open the project and synthesize a singing voice.

Treat the result as a likely conversion defect instead when:

- source-owned vocal notes are absent even though the source has a clear,
  monophonic vocal line;
- visible notes carry `la` or another lyric that is absent from the source;
- source lyrics disappear without a source-only or ambiguity diagnostic;
- the source Part/staff/voice layout is unexpectedly merged.

## The piano or other instruments are missing

The direct **Vocals `.svp`** action is vocal-only by design. It is not a
lossless container for arbitrary piano or orchestral playback.

Export a complete `.versebundle`, then open the SVP inside its `project/`
directory. That project references the rendered WAV stems under `audio/stems/`
and the muted reference under `audio/full-score.wav`.

If the project contains only `Full score reference mix` and an empty
`Unnamed Track`, it is probably an older bundle or the wrong project file.
Current bundle schema v2 either contains one verified stem per note-bearing
source Part or fails without publishing a bundle. Export to a new name and
open the newly generated `project/*.svp`.

The full-score track is a source reference mix. It is not a vocal-removed
accompaniment and remains muted by default to avoid doubling the Part stems.

## `la` appears where the source note is blank

Current Verse does not use `la` as a fallback lyric. Synthesizer V does: it
displays its own default syllable, greyed out and unsung, on any note whose
lyric is empty. A grey `la` in the piano roll is therefore a note Verse exported
with no lyric, not a syllable Verse invented — inspect the source at that note
before reporting it.

Until MuseScore tie merging shipped, the commonest cause was a tied note: the
tail of the tie was exported as a separate note with no lyric, so a sustained
`shine` stopped dead at the tie and showed a grey `la` for the rest of its
length. If you still see that pattern, re-export with a current build.

For the audited “This Little” score, the first vocal note is pitch 65 with an
empty lyric, and the first non-empty lyric is `let`. The regression test also
proves that the number of projected `la` syllables equals the number found in
the source.

If an unexpected `la` is visible:

1. confirm that you opened a newly exported file rather than the old
   `_LYRICS.svp`;
2. export to a destination that does not already exist;
3. inspect the original score at the same note and lyric lane;
4. check the track diagnostics in Verse;
5. retain the original source and current output if the unexpected lyric is
   reproducible.

An output lyric with no source evidence is a bug and should not be manually
explained away as a Synthesizer V phoneme.

## Notes are there but silent from the middle of the piece

Check whether the silent stretch is the second pass of a repeat, and whether the
score stacks two verses earlier on. Until verse silence was read per measure,
Verse assigned verse 2 to the whole second pass and dropped everything the
refrain wrote on a single lyric line: the notes kept their pitch and duration
while every lyric came out empty, and Synthesizer V displayed grey `la`
placeholders for the rest of the piece.

Current builds sing the line that is written wherever the pass's own verse says
nothing anywhere in that measure. If a current build still shows the pattern,
check the source at the same measure and lyric lane before reporting it, and
note whether the missing words sit on a lane that does sing elsewhere in the
same measure — that silence is deliberate.

## `MuseScore extracted N Parts but the source topology requires M`

This affected MIDI and KAR sources and blocked every complete bundle they
produced. MuseScore decides on its own how an imported MIDI becomes Parts: it
merges tracks that share an instrument, drops empty ones, and splits others. A
two-track file came back as one Part, a three-track file as four, a twelve-track
file as eleven. Verse compared that count against its own source tracks and
refused the export.

Current builds do not ask MuseScore to divide a MIDI. A MIDI, unlike a score,
divides exactly along its own `MTrk` chunks, so Verse cuts it itself: each stem
is the source track copied byte for byte, preceded by a rebuilt meta track
carrying the file's tempo, meter, key and SMPTE marks so it renders on the same
timeline as the reference mix. The stem is named `<track> (MIDI track)` rather
than `(MuseScore Part)`, because Verse chose the division and knows exactly
which source track each stem holds.

Score sources are unchanged: their Parts still come from MuseScore, and a
mismatch there is still a blocking error.

A MIDI stem may be shorter than the full-score reference when its track falls
silent before the end. Both start at zero, so it stays in step; padding it would
add audio the source never carried.

## A KAR file shows `Words`, but no vocal track is exported

KAR files often store text separately from melody notes. Verse does not infer
that every text event is singable:

- MIDI meta `0x05` is a lyric;
- generic meta `0x01` Text remains metadata unless the same physical track
  proves the supported Soft Karaoke controls/profile;
- the `.kar` extension alone is not evidence;
- control records such as `@KMIDI`, line markers, and line breaks are never
  sung;
- cross-track binding requires a complete, unique, injective, monotonic,
  non-percussion melody match;
- chord onsets with multiple pitches remain unresolved.

Look for these diagnostics:

| Diagnostic | Meaning |
|---|---|
| `EXTERNAL_KARAOKE_LYRICS_BOUND` | Detached lyrics were safely mapped |
| `EXTERNAL_KARAOKE_MELODY_TARGET` | Melody target was identified |
| `EXTERNAL_KARAOKE_LYRICS_UNRESOLVED` | The complete lyric stream could not be mapped |
| `EXTERNAL_KARAOKE_LYRICS_AMBIGUOUS` | More than one melody interpretation remained |
| `EXTERNAL_KARAOKE_TARGET_CONFLICT` | Candidate ownership conflicted |
| `KARAOKE_CHORD_PITCH_AMBIGUOUS` | A lyric onset had more than one possible pitch |
| `KARAOKE_CONTROLS_PRESERVED_AS_METADATA` | Karaoke controls were retained but not sung |
| `GENERIC_MIDI_TEXT_NOT_LYRICS` | Generic Text lacked lyric evidence |

A vocal override selects source notes; it does not copy a detached `Words`
stream onto them. A result with zero vocal tracks can therefore be the correct,
lossless outcome for an ambiguous KAR file.

## A MIDI file has no lyrics

Ordinary MIDI frequently contains only performance data. It can contain timed
lyrics through meta event `0x05`, but the format does not require them.

A valid lyric-free MIDI file:

- remains analyzable;
- retains its performance events in the source inventory;
- produces zero synthetic vocal tracks;
- can still contribute to complete source audio when its Parts can be
  rendered.

Verse deliberately does not generate `la la la`, guess words from a title, or
translate generic metadata into lyrics.

## Lyrics or notes remain source-only

The preservation report can retain content that cannot be represented safely
as an editable monophonic SVP vocal track. Common reasons include:

- standalone lyrics coexisting with already attached lyric lanes;
- a lyric attached to a polyphonic chord without unique pitch ownership;
- unpitched or unmapped percussion;
- grace notes with no positive playback duration;
- unsupported humming, laughing, or other non-text vocal content;
- ambiguous repeats or navigation;
- a true meter change inside a measure;
- timing that cannot be represented exactly in the supported SVP PPQ range.

Relevant diagnostics include:

- `STANDALONE_LYRICS_LEFT_SOURCE_ONLY`;
- `SOURCE_NOTES_NOT_IN_VOCAL_SVP`;
- `LYRIC_PROJECTION_AMBIGUOUS`;
- `UNSUPPORTED_LYRIC_CONTENT`;
- `AMBIGUOUS_SOURCE_ROLE`.

Source-only does not mean deleted. The item remains inventoried in
`preservation.json`, in the byte-identical original source, and—where
renderable—in the full-score or Part audio.

## MuseScore and complete bundles

Install only one supported renderer:

- MuseScore Studio 4.x is recommended and is required for native MuseScore 4
  scores;
- MuseScore 3.6.2 or later in the 3.x line is supported for scores it can
  open;
- MuseScore 5 and older MuseScore 3 releases are not currently qualified.

The executable must expose `--score-parts`. Configure the actual executable in
Verse Settings, not an application directory, alias, or custom shell command.
See [MuseScore renderer](musescore-renderer.md) for discovery paths.

### `RENDERER_NOT_FOUND`

- Install MuseScore 4 or MuseScore 3.6.2+.
- On macOS, place the normal application under `/Applications` or
  `~/Applications`, or select its internal `mscore` executable.
- On Windows, select `MuseScore4.exe` or `MuseScore3.exe`.
- On Linux, ensure `mscore4`, `musescore4`, `mscore3`, `musescore3`, `mscore`,
  or `musescore` resolves to the real binary.

Analysis and vocal-only export still work without MuseScore. Complete bundle
export does not have an audio-less fallback.

### `RENDERER_UNSUPPORTED`

Verify:

- the selected file is a real MuseScore executable;
- `--version` reports MuseScore 3.6.2+ or 4.x;
- `--help` advertises `--score-parts`;
- a native MuseScore 4 source is being rendered by MuseScore 4.

### `RENDERER_TIMEOUT` or `RENDERER_FAILED`

First open the source directly in MuseScore and confirm that:

- the score loads without an interactive repair dialog;
- Parts are available;
- playback is not entirely muted;
- console WAV export completes;
- every note-bearing Part selected for rendering contains the expected audible
  material.

The 20-minute renderer deadline covers Part extraction, the full reference,
and every sequential Part stem. Very large scores with many Parts can exhaust
that aggregate budget. A silent, truncated, malformed, oversized, or
identity-mismatched output is reported as a renderer failure.

On macOS, Verse already applies a bounded workaround for the known MuseScore 4
shutdown `SIGABRT`. Other crashes are not ignored, and the final accepted
attempt must still exit successfully.

## Bundle and destination errors

| Error code | Resolution |
|---|---|
| `INVALID_DESTINATION` | Choose a new path ending in `.versebundle` under an existing parent directory |
| `DESTINATION_EXISTS` | Choose another name; Verse never overwrites |
| `INVALID_SOURCE_NAME` | Rename the source to a safe representable filename |
| `STEM_PLAN_INVALID` | Verify that the source has unambiguous note-bearing Parts |
| `PRESERVATION_INCOMPLETE` | Keep the source and report the failure; the evidence ledger did not balance |
| `BUNDLE_INTEGRITY_FAILED` | Keep the source and renderer details; a staged hash, ID, WAV, SVP reference, or manifest invariant failed |
| `BUNDLE_IO_FAILED` | Check free space, parent permissions, and filesystem health |
| `BUNDLE_SERIALIZE_FAILED` | Preserve the source and report the reproducible metadata failure |
| `BUNDLE_COMMIT_FAILED` | Check destination races, permissions, and filesystem support for atomic publication |
| `BUNDLE_TASK_FAILED` | Retry after restarting Verse; report recurrence with the sanitized message |

All bundle failures are transactional. Verse removes its private staging
directory and leaves no published partial destination. If another process
creates the destination during export, Verse preserves that external
destination.

## Source and vocal-only export errors

| Error code | Resolution |
|---|---|
| `UNSUPPORTED_FILE` | Use one of `.kar`, `.mid`, `.midi`, `.mxl`, `.xml`, `.musicxml`, `.mscz`, `.mscx` |
| `SOURCE_TOO_LARGE` | The top-level input exceeds the 128 MiB safety limit |
| `SOURCE_READ_FAILED` | Check that the path is a readable regular file |
| `SOURCE_PARSE_FAILED` | Validate the archive/XML/MIDI in its producing application and export a fresh supported file |
| `CONVERSION_FAILED` | Read the exact timing, repeat, navigation, or ownership message; Verse refused a lossy projection |
| `INVALID_OUTPUT` | Preserve the source and report the generated SVP validation failure |
| `SERIALIZE_FAILED` | Preserve the source and report the serialization failure |
| `WRITE_FAILED` | Check destination permissions and available disk space |

MusicXML `score-timewise`, unsupported character encodings, entity-bearing
DTDs, malformed archive roots, MIDI format 2 projection, SMPTE projection,
non-converging repeats, and unsafe paths are examples of deliberate
fail-closed behavior rather than silent conversion.

## The bundle saved successfully but audio is silent or doubled

- Open the SVP under the bundle's `project/` directory.
- Confirm that the individual Part audio tracks are active.
- Keep `Full score reference mix` muted unless you intentionally want to hear
  it alongside every Part stem.
- Confirm that Synthesizer V can resolve the relative `../audio/...` paths;
  moving only the SVP outside the bundle breaks those references.
- Do not rename or move individual `audio/stems/*.wav` files.

Verse rejects a completely silent rendered WAV during export. Silence after
moving files is therefore usually a broken bundle layout or mute/playback
configuration, not accepted silent renderer output.

## Collecting a useful report

Record:

- Verse version and operating system;
- input format and the application/version that produced it;
- whether analysis, vocal-only export, or complete bundle export failed;
- structured error and diagnostic codes;
- MuseScore version and executable path when rendering is involved;
- whether a newly named export reproduces the problem;
- source Part/voice/lyric counts shown by Verse.

Do not publish copyrighted songs, private filesystem paths, or commercial
voice databases. Prefer a minimized synthetic or redistributable score that
preserves the same failure.
