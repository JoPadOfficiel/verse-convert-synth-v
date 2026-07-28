# Troubleshooting

Start by identifying which output you opened:

- **Vocals only** is a bare `.svp` or `.ustx`, in whichever format the **Export
  target** selector named. It contains only editable vocal notes, and
  intentionally has no piano, percussion, instrument stems, or full-score audio.
- **Complete bundle project** is the file under `<name>.versebundle/project/`, a
  `.svp` or a `.ustx` per the export target. It contains editable vocals, the
  audio-backed instrumental material for every note-bearing source Part, and a
  muted full-score reference. Both variants reference the same stems: SVP
  instrumental tracks in a `.svp`, `wave_parts` in a `.ustx`.

Verse never overwrites an export. Re-testing after an upgrade therefore
requires a new destination name. Opening an older project file or older bundle
is a common source of misleading results.

## No singing, a beep, or “No default voice”

This is normally voice configuration in the target application, not a conversion
defect, when the vocal notes and their lyric text are visible in the piano roll.

Verse writes vocal tracks without selecting or bundling a voice. In
Synthesizer V:

1. select the vocal track;
2. assign an installed voice database;
3. select the matching singing language supported by that voice;
4. regenerate or play the track.

In OpenUtau, assign a singer to the track. A `.ustx` from Verse deliberately
omits `singer` and `renderer_settings`: `URenderSettings` resolves the renderer
from whatever singer you assign, and naming one would assert something about a
voicebank Verse has never seen. `UNote.Validate` reads the track's singer, so a
note is not valid until you assign one — that is the unassigned state, not a
conversion fault.

Verse has no language selector, and none is needed: lyrics are written byte for
byte in any language. Nothing Verse writes installs a voice, translates lyrics,
or guarantees that the voice you pick supports the language of the words.

Neither application is required to analyze a source or write a project file.
Either is required to open its project and synthesize a singing voice.

Treat the result as a likely conversion defect instead when:

- source-owned vocal notes are absent even though the source has a clear,
  monophonic vocal line;
- visible notes carry `la` or another lyric that is absent from the source;
- source lyrics disappear without a source-only or ambiguity diagnostic;
- the source Part/staff/voice layout is unexpectedly merged.

## The piano or other instruments are missing

The direct **Vocals only** action is vocal-only by design, in either format. It
is not a lossless container for arbitrary piano or orchestral playback, and a
vocals-only `.ustx` carries `wave_parts: []` — no instruments, no audible
reference. A bundled `.ustx` does carry them.

Export a complete `.versebundle`, then open the project inside its `project/`
directory. That project references the rendered WAV stems under `audio/stems/`
and the muted reference under `audio/full-score.wav` — as instrumental tracks in
a `.svp`, as `wave_parts` in a `.ustx`.

If the project contains only `Full score reference mix` and an empty
`Unnamed Track`, it is probably an older bundle or the wrong project file.
Current bundle schema v2 either contains one verified stem per note-bearing
source Part or fails without publishing a bundle. Export to a new name and
open the newly generated project.

Nobody has yet confirmed by ear that a `.ustx` bundle's stems play in OpenUtau
0.1.568. Export writes and verifies every reference against the manifest and the
WAVs, so a missing or inconsistent reference cannot be published — but if the
stems are silent in the application when the paths and durations check out,
report it, because that would be new.

The full-score track is a source reference mix. It is not a vocal-removed
accompaniment and remains muted by default to avoid doubling the Part stems.

## A note with no word is not in the project, and that is deliberate

A note the source never texted is not written into the vocal project. OpenUtau
cannot sing an empty lyric — its phonemizer marks the note `error` — and
Synthesizer V draws one as a greyed-out unsung `la`, so writing them fills a
project with notes that look broken and sing nothing.

The notes are not gone. They stay byte-exact in the bundle's `source/` and are
audible in the stem MuseScore rendered from it. The analysis reports how many
each track left out, under `UNTEXTED_NOTES_LEFT_OUT`.

A held syllable is not this case. A word sustained across several notes texts
only the first of them; the rest carry a continuation the source proves, they are
sung, and they stay. So does the note immediately before a held syllable, which
has to stay or the hold would attach itself to the wrong word.

If a track you expected to sing is missing words, the score did not write them
there. Open it in MuseScore and check the lyric line under that staff.

## Opening the source in OpenUtau puts `a` on every note

That is OpenUtau's own default, not something Verse wrote. `UNote.lyric` is declared with
a field initializer, `public string lyric = NotePresets.Default.DefaultLyric;`
(`Ustx/UNote.cs`), and `NotePresets.DefaultLyric` is `"a"`
(`Util/NotePresets.cs:61`). Its MusicXML note factory sets a lyric only for a
real `<lyric>` or for a slur continuation, with no `else` branch
(`Format/MusicXML.cs:141-161`), so any other note keeps `"a"`. Opening one real
`.musicxml` in OpenUtau 0.1.568 put `a` on **319 of 319 notes**. Its MIDI reader
substitutes the same default for an unmatched note
(`Format/MidiWriter.cs:205-208`) and builds its lyric dictionary from
`LyricEvent` only (`:190-192`) — it never reads the `TextEvent` a Soft Karaoke
`.kar` stores its words in, and a `.kar` loads as plain MIDI
(`Format/Formats.cs:16`), so **every note in a `.kar` opened directly in
OpenUtau becomes `a`**.

If you see `a` on notes the source never texted, you opened the source in
OpenUtau instead of opening Verse's `.ustx`.

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

1. confirm that you opened a newly exported file rather than an earlier
   `_LYRICS.svp` or `_LYRICS.ustx`;
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

## A karaoke line is missing from the project

Check whether the source harmonises that line. A track sounding two notes at
once used to make its syllables ambiguous, and they were dropped rather than
given to a voice — an entire refrain line disappeared from a score whose banjo
doubles the melody underneath it.

Current builds split such a track into one lane per simultaneous voice and let
each voice sing the word. The project therefore contains more vocal tracks than
before, one per voice, and each can be assigned its own voice database.

A `.kar` sometimes transcribes the same passage twice, in two competing text
tracks. Verse binds the fullest stream; the duplicate is not sung a second time
and is not a loss.

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

## The OpenUtau target refuses a file the Synthesizer V target accepts

This is expected for some sources and is not a defect. Synthesizer V places a
quarter note at 705,600,000 blicks; OpenUtau fixes 480 integer ticks
(`UProject.resolution` is `[YamlIgnore] => 480`, `Ustx/UProject.cs:42`).
`705,600,000 / 480 = 1,470,000` exactly, so OpenUtau can state a strict subset of
the positions Synthesizer V can. `480 = 2^5 × 3 × 5` has no factor 7, so a
septuplet exports to `.svp` and is refused for `.ustx`.

The refusal names the MIDI tick, the source PPQ, and the source track. Options,
in order of fidelity:

1. switch the **Export target** to Synthesizer V. Both **Vocals only** and
   **Complete project** then work, because a bundle carries the target's own
   project and Synthesizer V accepts the source;
2. change the notation in the source so the passage lands on a grid of 480 ticks
   per quarter note. This changes the music, so Verse will not do it for you.

Under the OpenUtau target, **Complete project** is disabled for such a source
too. A bundle now carries the chosen target's own project, so offering the button
would only move the same refusal to export time. That is why switching the target
is the fix rather than reaching for the bundle.

Rounding is not offered. OpenUtau's own MusicXML importer truncates the same
case with `(int)note.Duration * uproject.resolution / divisions`
(`Format/MusicXML.cs:128`), which is exactly the silent shortening Verse exists
to avoid.

The OpenUtau target also refuses, for reasons that are not about timing:

| Refusal | Why |
|---|---|
| A note under 10 OpenUtau ticks | `UNote.Validate` does `duration = Math.Max(10, duration)` and would silently lengthen it |
| Two overlapping notes in one lane | One `voice_part` is monophonic; OpenUtau sets `OverlapError` on the later note instead of singing it |
| A held syllable or split on a note that does not touch its predecessor | `UVoicePart.Validate` wires a continuation only when `Prev.End == position`; otherwise the marker reaches the phonemizer and is sung as a word |
| A position beyond the 32-bit tick range | Every USTX tick field is a C# `int` |

Because the verdict is target-dependent, changing the export target re-analyses
every loaded file. A file that was convertible before the change may report a
refusal afterwards, and the other way round.

## Lyrics or notes remain source-only

The preservation report can retain content that cannot be represented safely
as an editable monophonic vocal track in the selected target. Common reasons
include:

- standalone lyrics coexisting with already attached lyric lanes;
- a lyric attached to a polyphonic chord without unique pitch ownership;
- unpitched or unmapped percussion;
- grace notes with no positive playback duration;
- unsupported humming, laughing, or other non-text vocal content;
- ambiguous repeats or navigation;
- a true meter change inside a measure;
- timing that cannot be represented exactly on the selected target's grid.

Relevant diagnostics include:

- `STANDALONE_LYRICS_LEFT_SOURCE_ONLY`;
- `SOURCE_NOTES_NOT_IN_VOCAL_SVP`;
- `LYRIC_PROJECTION_AMBIGUOUS`;
- `LYRIC_REINTERPRETED_BY_TARGET`;
- `UNSUPPORTED_LYRIC_CONTENT`;
- `AMBIGUOUS_SOURCE_ROLE`.

`LYRIC_REINTERPRETED_BY_TARGET` is not a loss: the word is written byte for
byte. It reports that the target application will read it as something other than
the word it spells. For OpenUtau that is a source word beginning with `+`, which
`UVoicePart.Validate` reads as a continuation of the previous note, and bracketed
source text, which `UNote.ToPhonemizerNote` takes as a phonetic hint and strips.

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
| `BUNDLE_INTEGRITY_FAILED` | Keep the source and renderer details; a staged hash, ID, WAV, project audio reference, or manifest invariant failed |
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
| `CONVERSION_FAILED` | Read the exact timing, repeat, navigation, or ownership message; the selected target refused a lossy projection |
| `INVALID_OUTPUT` | Check that the filename ends in `.svp` or `.ustx` to match the selected export target, that the destination is new, and that it is not the source |
| `SERIALIZE_FAILED` | Preserve the source and report the serialization failure |
| `WRITE_FAILED` | Check destination permissions and available disk space |

The output filename and the export target are independent arguments, and the
save dialog's filter is only advisory, so Verse refuses to write OpenUtau YAML
into a `.svp` or a Synthesizer V project into a `.ustx` — either would produce a
file neither application opens.

MusicXML `score-timewise`, unsupported character encodings, entity-bearing
DTDs, malformed archive roots, MIDI format 2 projection, SMPTE projection,
non-converging repeats, and unsafe paths are examples of deliberate
fail-closed behavior rather than silent conversion.

## The bundle saved successfully but audio is silent or doubled

- Open the project under the bundle's `project/` directory, not a copy of it
  elsewhere.
- Confirm that the individual Part audio tracks are active. In a `.ustx` the mute
  state sits on each wave part's own track, because OpenUtau has no per-part mute.
- Keep `Full score reference mix` muted unless you intentionally want to hear
  it alongside every Part stem.
- Confirm that the application resolves the relative `../audio/...` paths. Both
  formats resolve them against the project file's own directory, so moving only
  the project out of the bundle breaks every reference.
- Do not rename or move individual `audio/stems/*.wav` files. A `.ustx` also keys
  each wave part's `name` to the WAV's basename.

Verse rejects a completely silent rendered WAV during export. Silence after
moving files is therefore usually a broken bundle layout or mute/playback
configuration, not accepted silent renderer output.

## Collecting a useful report

Record:

- Verse version and operating system;
- input format and the application/version that produced it;
- the selected export target, and whether the other target behaves differently;
- whether analysis, vocals-only export, or complete bundle export failed;
- structured error and diagnostic codes;
- the target application and its version when the file opens but looks wrong;
- MuseScore version and executable path when rendering is involved;
- whether a newly named export reproduces the problem;
- source Part/voice/lyric counts shown by Verse.

Do not publish copyrighted songs, private filesystem paths, commercial voice
databases, or voicebanks. Prefer a minimized synthetic or redistributable score
that preserves the same failure.
