# Formats and Fidelity

## Fidelity contract

Verse projects only source-backed musical evidence. It never invents lyrics,
notes, pitches, voices, tracks, instruments, or audio.

- A genuine source lyric such as `la` is retained.
- A note without a lyric is not written into the vocal project; it stays in
  the preserved source and its rendered stem.
- A continuation is emitted only when the source contains continuation or
  extension evidence.
- Instrumental and percussion material is not serialized as vocal-shaped notes
  in either target.
- Ambiguous ownership remains source-only or causes an explicit failure.
- The byte-identical source remains the final authority inside a complete
  bundle.

Neither `.svp` nor `.ustx` is a lossless notation container. “Source-faithful”
means exact source preservation, evidence-backed projection, an auditable
disposition ledger, and fail-closed handling of unrepresentable semantics.

## Export targets

Verse writes two project formats. The **Export target** selector chooses one;
`ExportTarget` serializes as the stable lowercase values `"svp"` and `"ustx"`,
and Synthesizer V is the default.

| | Synthesizer V | OpenUtau |
|---|---|---|
| Output | `.svp`, project version 113 | `.ustx`, `ustx_version` 0.6 |
| Time unit | blicks | integer ticks |
| One quarter note | 705,600,000 blicks | 480 ticks |
| Held syllable | `-` | `+~` |
| Syllable split | `+` | `+` |
| Inside a `.versebundle` | yes | yes |
| Bundle audio reference | instrumental track, `blickOffset = 0` | `wave_parts` entry, all offsets `0` |

Both targets read the same `engine/projection.rs` output, which stays in
source-exact IR ticks and carries the source's `LyricState` rather than a
rendered marker. A target owns its own grid, marker vocabulary, cosmetics and
schema version, and cannot reach back into the conversion engine or change what
the other target writes.

### Notes the source never texts

A lane the source texts usually also carries notes it never texts: an
instrumental doubling, an introduction, a harmony line the words do not reach.
Those are not vocal material, and they are not written into the vocal project.

Writing them is not merely untidy. OpenUtau's phonemizer cannot phonemize an
empty lyric and marks every such note `error`; measured on one reported score,
505 of them, which is a file that reads as a failed conversion. Synthesizer V
draws the same note as a greyed-out unsung `la`, which reads like an invented
syllable even though it is not.

Nothing is lost. Those notes stay byte-exact in the bundle's preserved source and
are audible in the stem MuseScore renders from it — the same way every
instrumental note has always been kept. Each lane reports how many it left out,
under `UNTEXTED_NOTES_LEFT_OUT`.

Two rules bound what counts as untexted.

- A note is sung whenever the source asks for it to be. That includes a `humming`
  or `laughing` vocalization no target can spell — a sound the score asks for —
  and every note of a melisma the source states: a MusicXML `<extend>`, a
  MuseScore extension length, or a qualified Soft Karaoke phrase. Those notes
  carry a continuation, and leaving them out would shorten a word the score
  sustains.
- A note also stays when it is the note a continuation marker leans on: the note
  immediately before that marker in time, whether or not the two touch. A marker
  carries the *previous* note's syllable, so which note precedes it decides which
  word is held. OpenUtau refuses the export outright when the two do not touch;
  Synthesizer V checks nothing and would silently rebind the hold to whatever
  note was left in front of it, sustaining a different syllable with no
  diagnostic. The second is the worse failure, and it is why the predecessor is
  kept in both cases.

A lane with nothing left to sing is returned whole rather than emptied: that is
what a user override projects from a wordless track, and emptying it would delete
the only thing the override asked for.

A melisma is a source claim, not a guess. Where a source states no continuation —
most Standard MIDI files, and any `.kar` whose karaoke evidence does not qualify —
Verse has never read a wordless note following a syllable as a held one, and does
not invent a hold now.

### The 480-tick grid, and why a septuplet splits the two targets

`UProject.resolution` is `[YamlIgnore] => 480` (`Ustx/UProject.cs:42`), so the
`resolution:` Verse emits is ignored on load and rescaling is impossible. 480
integer ticks per quarter note is a property of the format.

`705,600,000 / 480 = 1,470,000`, an exact integer. Every position OpenUtau can
state is therefore also a Synthesizer V position — but not the reverse. `480 =
2^5 × 3 × 5` has no factor 7, so a septuplet has no exact representation at 480
ticks per quarter while blicks state it exactly. A septuplet score exports to
`.svp` and is **refused** for `.ustx`.

That is why the export target is part of the convertibility verdict rather than
a write-time detail: `engine::target::validate_for` runs the chosen target's own
arithmetic during analysis, and changing the target re-analyses every loaded
file. Asking one target on behalf of another would clear a source the other must
refuse, and the refusal would resurface at export.

Refusing is not a shortfall. OpenUtau's own MusicXML importer computes
`int durTick = (int)note.Duration * uproject.resolution / divisions;`
(`Format/MusicXML.cs:128`) — truncating integer division with no exactness
check, so any duration that does not divide evenly is silently shortened.
Refusing the same case is strictly more faithful than the software's native
behaviour.

### Two marker vocabularies that must never be swapped

Synthesizer V spells a held syllable `-` and a syllable split `+`. OpenUtau
spells the hold `+~` and the split `+`:

- `Format/MusicXML.cs:157-160` writes `+~` for a slur, commented “OpenUtau uses
  +~ for extending the current syllable”;
- `Format/MusicXML.cs:147-149` writes `+` for the following syllables of a
  multi-syllable word, and `NotePresets.SplittedLyric` is `"+"`;
- `Format/MidiWriter.cs:209-211` converts an imported MIDI `-` into `+~`, and
  `:272-274` converts it back on export.

So `+` means the split in both targets, while `-` and `+~` mean the hold in one
each. A naive string substitution would turn every hold into a split. Verse
avoids that by never carrying rendered marker text through the projection: the
neutral projection carries `LyricState`, and each target renders it.

`UVoicePart.Validate` sets `Extends` on any lyric starting with `"+"`, which
covers both OpenUtau markers, and wires it only when `Prev.End == position`. A
marker on a note that does not touch its predecessor would reach the phonemizer
as a lyric and be sung as a word, so Verse refuses that case instead. The test
is on provenance, not on the emitted string: a source word may itself spell `+`
or `+~`, and such a word is text.

Text OpenUtau reinterprets before singing it is written byte-exactly and
diagnosed under `LYRIC_REINTERPRETED_BY_TARGET`: a source word beginning with
`+` (read as a continuation by `UVoicePart.Validate`) and bracketed text (taken
as a phonetic hint and stripped by `UNote.ToPhonemizerNote`).

### What each target refuses

Both targets refuse a position or duration that does not divide exactly into
their own grid, and report the offending MIDI tick and PPQ. Verse leaves the
source untouched and never rounds.

OpenUtau additionally refuses:

- a note whose duration falls under the 10-tick floor. `UNote.Validate` does
  `duration = Math.Max(10, duration)` and would silently lengthen it.
- two overlapping notes in one voice part. One `voice_part` is monophonic and
  OpenUtau sets `OverlapError` on the later note instead of singing it.
- a held syllable or syllable split on a note that does not begin exactly where
  its predecessor ends, including such a marker on the first note of a lane.
- a position or a position-plus-duration beyond the C# `int` range every USTX
  tick field uses.

Notes are emitted at strictly increasing positions, because `UNote.CompareTo`
falls back to `GetHashCode()` at equal positions and OpenUtau's load order would
otherwise be undefined.

### Structural defaults that assert nothing

A `.ustx` note carries `pitch.data` with two `y: 0` points and a true
`snap_first`, because `UNote.Validate` dereferences `pitch.data[0]` with no
guard. `vibrato.length: 0` disables vibrato and every other vibrato field is the
value OpenUtau's own `UVibrato` initializes. A track names
`OpenUtau.Core.DefaultPhonemizer` because a track must carry one.

`singer` and `renderer_settings` are deliberately absent: `URenderSettings`
resolves the renderer from whatever singer the user assigns, and naming one
would assert something about a voicebank Verse has never seen. `expressions`
need not be authored — `Ustx.Load` calls `AddDefaultExpressions` on every load.

`ustx_version` is emitted as `0.6` and never lower. For a project declaring
less than `0.6`, `Ustx.Load` replaces the whole `time_signatures` and `tempos`
lists with one entry each taken from the obsolete `bpm`/`beat_per_bar`/
`beat_unit` scalars, destroying every tempo and meter change in the score. Those
obsolete scalars are still written, set from the first tempo and meter, so that a
mistaken downgrade loses the later changes rather than corrupting the opening of
the score.

### The YAML emitter

`.ustx` is written by a hand-written deterministic emitter rather than a YAML
dependency: the schema is closed and small, and byte-exact output is testable.
Every string scalar is unconditionally double-quoted, which removes every YAML
ambiguity for arbitrary lyric text — a `:`, a `#`, a quote, a backslash, a
leading space — with no plain-scalar heuristic. Inside double quotes only `"`,
`\` and the code points a YAML 1.1 reader may treat as a line break are escaped;
every other code point, including all non-ASCII, is written literally as UTF-8.

Keys are `snake_case` because OpenUtau's `Util/Yaml.cs` uses
`UnderscoredNamingConvention`.

## Format detection

The UI filters by extension, then Rust verifies the content:

- XML is classified as MusicXML or native MuseScore XML.
- ZIP input is inspected for MusicXML or an MSCX master score.
- Remaining inputs must be valid Standard MIDI Files.
- A `.kar` extension enables the conservative cross-track karaoke ownership
  resolver but does not turn generic Text into lyrics.

All application paths—analysis, vocal-only export, and complete bundle
export—use the same extension-aware snapshot parser.

## MIDI and KAR

### Supported evidence

- SMF format 0 and 1.
- PPQ timing.
- Note on/off, including note-on velocity zero as note-off.
- FIFO pairing by channel and key when no richer source identity exists.
- Tempo and time signature.
- Track names, channel, port, program, bank, controls, pressure, pitch bend,
  SysEx, and unknown meta data retained in the source model/ledger.
- Lyric meta events (`0x05`).
- Generic Text meta events (`0x01`) as metadata.
- UTF-8 text with Windows-1252 fallback while retaining raw bytes.

### Soft Karaoke qualification

Text events become karaoke lyrics only when their own physical source track
contains a qualified Soft Karaoke profile, such as `@KMIDI` and recognized
line controls. A marker on another track or the `.kar` extension alone does
not qualify generic Text.

Detached lyrics may bind to a melody only when:

- the source is a `.kar` container;
- all real lyric tokens are mapped;
- the mapping is monotonic and injective;
- the temporal tolerance is at most half a quarter note;
- exactly one non-percussion target satisfies the evidence;
- target-owned lyrics are not replaced;
- polyphonic onset ambiguity is absent.

If two targets qualify, two streams compete for one target, or only a partial
mapping exists, the words stay source-only.

### Valid lyric-free MIDI

A standard MIDI file commonly has no lyrics. This is valid. Verse preserves
its events and topology, produces zero generated words, and does not create a
synthetic C4 lyric track.

### Voices inside one track

A Synthesizer V vocal track is monophonic, and so is one OpenUtau `voice_part`,
so a source track that sounds two notes at once is split into one lane per
simultaneous voice — the same decomposition score importers apply to a chord.
Splitting is driven by sounding overlap alone: a track that never overlaps is
projected unchanged.

This matters beyond tidiness. A karaoke syllable landing on a stack of notes has
no single note to own, and used to be dropped as ambiguous. Split into voices,
each voice owns the syllable it sounds.

Lane 0 keeps the source track's identity and every non-note record — tempo,
controls, program changes and lyrics belong to the track, not to one of its
voices, and are never duplicated into another lane.

### Audio stems

A score's Parts are extracted by MuseScore, which reads the same file Verse
did. An imported MIDI is different: MuseScore decides on its own how its tracks
become Parts, merging those that share an instrument and dropping empty ones, so
its Part list answers a different question than "which source track is this".

Verse therefore divides a MIDI itself, along the `MTrk` chunks the format
already separates. Each stem is the source track copied byte for byte, preceded
by a rebuilt meta track carrying only the marks that govern the whole file —
tempo, meter, key, SMPTE offset — so it renders on the reference mix's timeline.
Nothing is transposed, quantised, or invented, a stem is a subset of the source,
and the source track it holds is known because Verse chose it.

A stem may therefore be shorter than the reference mix when its track falls
silent before the end. Both start at zero, so it stays in step for every frame
it has; a stem running past the end of the whole score is still refused.

### Explicit refusals

- SMF format 2 independent sequences are not flattened.
- SMPTE timing is parsed/preserved but is not projected to either target.
- More than 4,096 tracks or 2,000,000 events is rejected.
- Malformed running status, chunks, lengths, ticks, or channel events is
  rejected.

## MusicXML and MXL

### Supported

- Raw `.xml`/`.musicxml` and compressed `.mxl`.
- `score-partwise`.
- UTF-8/ASCII, UTF-16 LE/BE, ISO-8859-1, and Windows-1252.
- Declared Parts, staves, voices, instruments, channels/programs, and
  percussion mapping.
- Multiple lyric lanes/verses, syllabic state, elisions, `time-only`, and
  extension state.
- Chords represented as technical monophonic lanes under one source voice.
- Grace-note and unpitched evidence preserved without fallback pitch.
- MusicXML start/stop tie chains merged into the editable projection.
- Repeats, voltas, and supported D.S./D.C./Coda/Fine playback expansion.
- Exact common PPQ derived from local `<divisions>` values when it fits the
  supported range.

### Not supported

- `score-timewise`.
- DTD/internal entity input.
- XML encodings outside the documented set.
- A timing grid whose exact common PPQ exceeds the supported `u16` range.
- Ambiguous or non-convergent playback navigation.

## Native MuseScore

### Supported

- Raw `.mscx` and packaged `.mscz`.
- Historical MuseScore 2 data and MuseScore 3/4 score structures used by the
  current parser.
- Strict master-score selection from `META-INF/container.xml`.
- Part names, instruments, staves, voices, rests, locations, chords, tuplets,
  graces, tempo, meter, navigation, and all source lyric lanes.
- Declared rest-only topology retained even when no editable projection lane
  exists.

The parser rejects archive traversal, ambiguous masters, a package containing
only Excerpts, malformed XML, and unsafe timing/pitch values.

### Lyrics on chords

A word written under a chord means one of two things, and they are opposites.

In a choir it is a **harmony**: the members are simultaneous voices of one line,
each sings that word, and each can be given its own voice database. A passage
harmonised in thirds keeps both voices and both carry the text.

In a reduction it is an **accompaniment**: one singer over chords no singer could
produce, since a voice sounds one note at a time. Reading such a chord as a
harmony asks one written syllable to be sung several times at once — measured on
one reported piano score, 491 sung syllables where the score writes 363, two of
them asked for nine times over.

Verse does not guess between the two. It reads what the part declares itself to
be, using the language-independent instrument identifier every notation program
writes — MuseScore `<instrumentId>`, MusicXML `<instrument-sound>`, a General MIDI
program. `voice.*` and `vocal.*`, and GM programs 52–54, are singing instruments
and select the harmony reading; anything else selects the reduction, where the
chord's highest note takes the word and the notes below it are kept without one.
Matching on the identifier and never on a display name is not a detail: the score
that exposed this names its instrument `Piano, Фортепиано`.

A part that declares no instrument at all is measured against its own chords: a
harmony if at least half of its chords carrying a word hold more than one note, a
reduction otherwise. A choir harmonises throughout; a reduction carries the
occasional chord under an otherwise single-note melody.

Both readings are reported under `CHORD_READING`, with the evidence that decided
them, whenever a source writes a word over a chord at all.

Under either reading, Verse never picks a note as "the melody" in order to *drop*
the others: the notes that do not take the word are kept, without one. And the
older failure must never return — reading a chord as ambiguous and leaving its
lyric source-only deleted whole sung phrases, such as an entire refrain line of a
score whose banjo doubles the melody a sixth below. Only a chord with no note at
all has nothing to carry its lyric, and that lyric stays source-only with a
diagnostic.

### Verse numbers

MuseScore numbers a verse with `<no>`, and omits it only for the first one. Two
`<Lyrics>` elements that both omit it are therefore one verse written twice —
something real files contain — and not a second verse. Reading the second
element's position among its siblings as a verse number copied every lane of a
score out a second time, note for note, and doubled its sung syllables.

### Native ties

Native MuseScore tie spanners are merged into one sustained projected note, as
MusicXML tie chains already were. The tail of a chain keeps its source identity
and loses only its played pitch, so the ledger still accounts for every source
note while the target application sees a single attack held for the whole chain.

A pairing is accepted only when MuseScore's own back reference confirms it: the
notes must be adjacent in time and the head must sit exactly where the tail's
`<location>` says it does. Pairing on pitch alone mis-merges measurably, so a
contradicted pairing is refused and both notes stay separate. Cross-staff ties,
ties ending on a grace note, and chains broken by a repeat jump are likewise
left unmerged rather than guessed.

A tie tail that carries its own syllable is not a plain sustain either: the
score asks for that word to be sung, so the note keeps its own attack. An
explicitly empty lyric is the opposite signal — both formats write one on a tied
note to state that nothing is sung there — and does not block the merge. Both
the MuseScore and the MusicXML path follow this rule, so the same score exported
either way projects identically.

### Repeat structure

Repeat barlines, voltas and jumps describe the score, not one staff or one Part,
and exporters usually write them under the first one only. Verse therefore
unrolls a single playback order, built from the union of every staff's marks in
MuseScore and every Part's marks in MusicXML, and applies it to all of them. Two
containers stating different values for the same mark, or disagreeing on measure
count, are rejected instead of arbitrated.

MusicXML leaves "al Fine" and "al Coda" implicit: the jump is only complete once
the To Coda and Fine marks are known. Those marks belong to the score too, so
they are resolved after the Parts have been merged — an exporter can write the
jump under one Part and its target under another, and reading them apart turns a
`D.S. al Coda` into a plain `D.S.`.

### Stacked verses

Verses stacked under one melody are alternatives, not simultaneous voices: the
score replays the music and sings the next verse. When the repeat structure
provides a pass per verse, Verse projects one track and sings verse N on pass N.
When there are more verses than passes there is nowhere to put the extra ones,
so each keeps a track of its own at the same instants and a
`LYRIC_VERSES_EXCEED_REPEAT_PASSES` diagnostic says so.

A score stacks verses only under the passage whose words differ. The refrain
that follows is normally written on a single lyric line meant for every pass.
Verse therefore reads a verse's silence per measure: where the pass's own verse
carries no word anywhere in the measure, the line that is written is common text
and every pass sings it. Where that verse does sing elsewhere in the same
measure, its silence on one note is the verses dividing a word into different
syllables, and the note stays wordless rather than borrowing a neighbouring
verse's syllable.

MIDI has no measures and cannot supply that evidence, so a replayed MIDI note
whose verse says nothing keeps its empty lyric.

## Source topology and projection lanes

Score formats preserve:

```text
Part → staff → voice → technical projection lanes
```

A Part is a score-owned musical unit. A voice is a source-owned notational
voice. A projection lane is an internal monophonic lane required to represent
polyphony in a target whose vocal lane is monophonic. Reporting and UI counts
use Parts and source voices; technical lanes remain visible only in detailed
track contracts.

Parts containing only metadata or rests remain in topology and the exact
source. Only note-bearing Parts require WAV stems.

## Timing and target defaults

The projection keeps every position in source-exact IR ticks, whose PPQ is
derived from the source itself — MuseScore's `Division`, or the LCM of the
MusicXML `<divisions>` values — so no source duration is lost before a target
sees it. Each target then converts once:

- Synthesizer V: one quarter note equals exactly 705,600,000 blicks;
- OpenUtau: one quarter note equals exactly 480 integer ticks.

Every projected note boundary and tempo position must divide exactly into the
selected target's grid. Verse rejects an inexact position instead of rounding
it, and the message names the MIDI tick and the source PPQ. See
[Export targets](#export-targets) for why OpenUtau's grid accepts a strict
subset of what blicks accept.

Both targets require a usable timeline. If the source declares no meter or
tempo, Verse supplies target defaults of 4/4 and 120 BPM at position zero.
These are serializer defaults, not claims that the source contained those
events; the preservation ledger does not mark them as source evidence.

Meter changes are accepted only on exact measure boundaries. A change inside a
measure is rejected because both formats index meter by integer bar: SVP
`Meter{index, numerator, denominator}` and USTX
`time_signatures[{bar_position, beat_per_bar, beat_unit}]`. No arithmetic
converts between them.

## No language selection

There is no language selector, and no language needs choosing. Lyrics are
source text: they are written byte for byte, in any language, with nothing to
configure. `src-tauri/tests/language_fidelity.rs` proves this for French,
Spanish, English, Portuguese, German, Polish and Turkish, deliberately passing a
language that is wrong for the content to show the text does not depend on it.

Verse does not fill `database.language` in a `.svp`, and the OpenUtau target
never writes a language at all. Verse has never seen the voice database or
singer a track will be sung with, so it states nothing about it. Verse likewise
never translates lyrics, changes spelling or Unicode, generates phonemes,
transliterates text, or changes source role or ownership.

## User overrides

The Part-level “Vocal SVP” / “Vocal USTX” control applies one explicit Boolean
decision to all eligible projection lanes in that Part. It may request pitched
notes as a vocal track or leave them in the reference audio. It never copies
lyrics from another Part or changes the source classification.

## Preservation outcomes

Each item represented in the current rich source model receives one of:

- `projectedExact`
- `renderedStem`
- `sourceOnly`
- `metadataOnly`

The original source preserves constructs that are not individually represented
by the current model. See [Bundle format](bundle-format.md) for the complete
ledger contract.
