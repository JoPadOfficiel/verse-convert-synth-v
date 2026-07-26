# Formats and Fidelity

## Fidelity contract

Verse projects only source-backed musical evidence. It never invents lyrics,
notes, pitches, voices, tracks, instruments, or audio.

- A genuine source lyric such as `la` is retained.
- A note without a lyric receives an empty lyric.
- A continuation is emitted only when the source contains continuation or
  extension evidence.
- Instrumental and percussion material is not serialized as vocal-shaped SVP
  notes.
- Ambiguous ownership remains source-only or causes an explicit failure.
- The byte-identical source remains the final authority inside a complete
  bundle.

SVP is not a lossless notation container. “Source-faithful” means exact source
preservation, evidence-backed projection, an auditable disposition ledger, and
fail-closed handling of unrepresentable semantics.

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

### Explicit refusals

- SMF format 2 independent sequences are not flattened.
- SMPTE timing is parsed/preserved but not projected to SVP.
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

When a lyric belongs to a chord with exactly one pitch, ownership is exact.
When a lyric is attached to a polyphonic chord and the source does not identify
which pitch owns it, the lyric remains in a source-only lane with a diagnostic.
Verse does not choose the highest, lowest, or first note.

### Native ties

Native MuseScore tie spanners are merged into one sustained SVP note, as
MusicXML tie chains already were. The tail of a chain keeps its source identity
and loses only its played pitch, so the ledger still accounts for every source
note while Synthesizer V sees a single attack held for the whole chain.

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

Repeat barlines, voltas and jumps describe the score, not one staff, and
MuseScore usually writes them on the first staff only. Verse therefore unrolls
one playback order built from the union of every staff's marks and applies it to
all of them. Two staves stating different values for the same mark, or staves
that disagree on measure count, are rejected instead of arbitrated.

### Stacked verses

Verses stacked under one melody are alternatives, not simultaneous voices: the
score replays the music and sings the next verse. When the repeat structure
provides a pass per verse, Verse projects one track and sings verse N on pass N.
When there are more verses than passes there is nowhere to put the extra ones,
so each keeps a track of its own at the same instants and a
`LYRIC_VERSES_EXCEED_REPEAT_PASSES` diagnostic says so.

## Source topology and projection lanes

Score formats preserve:

```text
Part → staff → voice → technical projection lanes
```

A Part is a score-owned musical unit. A voice is a source-owned notational
voice. A projection lane is an internal monophonic lane required to represent
polyphony in Synthesizer V. Reporting and UI counts use Parts and source
voices; technical lanes remain visible only in detailed track contracts.

Parts containing only metadata or rests remain in topology and the exact
source. Only note-bearing Parts require WAV stems.

## Timing and target defaults

One quarter note equals exactly 705,600,000 Synthesizer V blicks. Every
projected note boundary and tempo position must divide exactly into this grid.
Verse rejects an inexact position instead of rounding it.

Synthesizer V requires a usable timeline. If the source declares no meter or
tempo, Verse supplies target defaults of 4/4 and 120 BPM at position zero.
These are serializer defaults, not claims that the source contained those
events; the preservation ledger does not mark them as source evidence.

Meter changes are accepted only on exact measure boundaries. A change inside a
measure is rejected because SVP stores meter positions by integer measure.

## Language selection

English/French selection sets the target Synthesizer V database language on
vocal tracks. It does not:

- translate lyrics;
- change spelling or Unicode;
- generate phonemes;
- transliterate text into Japanese;
- change source role or ownership.

## User overrides

The Part-level “Vocal SVP” control applies one explicit Boolean decision to all
eligible projection lanes in that Part. It may request pitched notes as a vocal
track or leave them in the reference audio. It never copies lyrics from another
Part or changes the source classification.

## Preservation outcomes

Each item represented in the current rich source model receives one of:

- `projectedExact`
- `renderedStem`
- `sourceOnly`
- `metadataOnly`

The original source preserves constructs that are not individually represented
by the current model. See [Bundle format](bundle-format.md) for the complete
ledger contract.
