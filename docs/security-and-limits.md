# Security and Resource Limits

Verse treats imported scores, MIDI files, archive contents, output paths,
MuseScore executables, renderer output, and generated bundle metadata as
untrusted. Conversion is offline, but local files can still attempt path
traversal, decompression bombs, excessive allocation, process hangs, or
inconsistent output substitution.

The governing rule is **fail closed**: when Verse cannot prove ownership,
timing, identity, or integrity, it preserves the source evidence where
possible and refuses the unsafe projection or bundle. It does not insert
fallback lyrics, pitches, tracks, instruments, or audio.

## Trust boundaries

The main boundaries are:

1. The Tauri command layer accepts a local source path and a new output path.
2. The format parsers turn untrusted bytes into an evidence-bearing source
   model.
3. The projection layer emits only representable, source-owned vocal content.
4. The renderer adapter invokes one qualified user-installed MuseScore
   executable with fixed arguments.
5. The bundle writer validates every staged artifact before an atomic
   no-replace commit.

The React frontend is not an authority for paths, process arguments, stem
identity, source ownership, or integrity decisions. Those checks remain in
Rust.

## Input and parser limits

| Area | Limit or rule |
|---|---|
| Top-level source | Regular file, supported extension, at most 128 MiB |
| Supported extensions | `.kar`, `.mid`, `.midi`, `.mxl`, `.xml`, `.musicxml`, `.mscz`, `.mscx` |
| MIDI tracks | At most 4,096 |
| MIDI events | At most 2,000,000 across the file |
| Playback expansion | At most 1,000,000 measures and 8,000,016 navigation steps |
| Compressed score XML | At most 64 MiB after decompression per selected XML entry |
| XML tree | At most 5,000,000 nodes |
| XML nesting | At most 200 levels |
| Projected timing | Exact positive PPQ that the selected export target can represent |
| Synthesizer V tick range | Positions and durations must fit the blick range at 705,600,000 blicks per quarter |
| OpenUtau tick range | Positions and durations must fit a C# `int` at 480 ticks per quarter, and a note must reach the 10-tick floor |

MIDI format 2 contains independent sequences and is not flattened into a
single project timeline. SMPTE division is parsed for preservation but is not
projected to either target. Zero or inconsistent PPQ is rejected.

Timing is converted once, from source-exact IR ticks onto the selected target's
grid, and a quantity that does not divide exactly is refused with the tick and
PPQ named. OpenUtau's grid accepts a strict subset of what Synthesizer V blicks
accept, so the refusal set is target-dependent and is evaluated at analysis time.

MusicXML parsing expects a `score-partwise` document. Unsupported encodings,
malformed declarations, internal DTD subsets, entity declarations, and
external entity resolution are rejected. The small official external
MusicXML doctype form is accepted without resolving the DTD.

MSCZ/MXL archives must select one unambiguous master document. Absolute paths,
parent traversal, backslash traversal, duplicate container declarations,
missing roots, excerpt-only MuseScore archives, and ambiguous score roots are
rejected.

Durations, divisions, pitches, tuplets, meters, repeats, and navigation are
validated exactly. Values are not silently clamped or truncated. Nested
repeats, non-converging playback, multiple ambiguous jumps, missing navigation
targets, conflicting global durations, or an actual time-signature change
inside a measure can therefore make a source ineligible for projection to either
target.

## Lyric and musical-evidence safety

Verse keeps source role and export representation separate:

- MIDI meta `0x05` is a lyric event.
- MIDI meta `0x01` remains generic text unless the same physical track proves
  the supported Soft Karaoke profile.
- A `.kar` filename alone does not promote generic text to lyrics.
- Cross-track KAR binding requires a complete, unique, injective, monotonic,
  non-percussion melody match within the timing tolerance.
- A chord onset with more than one possible pitch remains unresolved.
- A user vocal override may select source notes for vocal export, but it does
  not authorize copying lyrics from another track.
- A lyric-free MIDI file is valid and produces no synthetic vocal track.
- Missing lyrics remain empty. Genuine source `la` remains `la`; Verse never
  fills empty notes with `la`, and never with OpenUtau's own default `a`.
- Lyric text is written byte for byte, in any language. Nothing configures it,
  and no output names a voice database or singer.
- Unmapped percussion, grace notes, unsupported vocal effects, and other
  non-projectable source items remain in the source inventory and full-score
  audio rather than becoming fabricated vocal notes.

When no tempo or meter exists at all, both project timelines require neutral
defaults of 120 BPM and 4/4. These are project-timeline defaults, not source
evidence and not permission to fabricate musical content. The same applies to
the structural values a `.ustx` note must carry: two `y: 0` pitch points, because
`UNote.Validate` dereferences `pitch.data[0]` with no guard, and
`vibrato.length: 0`, which disables vibrato. Neither states an expression.

## MuseScore process boundary

Verse does not execute a command supplied by the frontend. It canonicalizes
the configured executable, requires a plausible regular MuseScore binary,
probes its identity and capabilities, and constructs fixed argument arrays
without a shell:

```text
MuseScore -F --score-parts <input>
MuseScore -F -o <output.wav> <input>
```

Accepted renderers are MuseScore 3.6.2 or later in the 3.x line and MuseScore
4.x, with `--score-parts` support. Unsupported versions, future unqualified
major versions, fake executables, and MuseScore 3 used with a native MuseScore
4 score are rejected.

The executable SHA-256 is checked before and after relevant calls. Child
processes receive:

- closed standard input;
- fixed arguments;
- a private working directory, home, configuration, cache, and temporary
  environment;
- a cleared environment with only a small platform/locale allowlist;
- bounded stdout/stderr capture;
- process-tree termination on timeout.

## Renderer resource limits

| Renderer resource | Limit |
|---|---:|
| Version/capability probe | 10 seconds |
| Aggregate Part extraction + full-score + all-stem render | 20 minutes |
| Captured failure log | 64 KiB |
| `--help` output | 1 MiB |
| `--score-parts` JSON | 128 MiB |
| Extracted Parts | 1–256 |
| One decoded Part MSCZ | 32 MiB |
| Aggregate decoded Part MSCZ data | 512 MiB |
| Archive entries in an extracted Part | At most 128 |
| Native-score version prologue | 1 MiB |
| One rendered WAV | 2 GiB |
| Aggregate bundle audio | 8 GiB |

Every WAV must be a regular file with a valid, completely decodable header and
sample payload, nonzero frames, sample rate, channels, and duration, finite
samples, and audible non-silent content. All stems must match the full-score
sample rate and frame count.

On macOS with MuseScore 4, only the known shutdown `SIGABRT` race is retried.
Score-loading calls are serialized, a ten-second cooldown is applied, and at
most three attempts fit within the same aggregate deadline. A final successful
exit and valid output remain mandatory.

## Part identity and audio integrity

Part extraction must map one-to-one to every note-bearing source Part. Verse
uses a native Part identifier first and a unique normalized name only as a
fallback identity key. Missing, duplicate, ambiguous, extra, or mismatched
Parts block the bundle.

The complete bundle requires:

- exactly one rendered stem for every expected note-bearing Part;
- matching expected, rendered, and recorded stem IDs;
- exactly one project audio reference per stem asset — one audio-backed SVP track
  in a `.svp` bundle, one `wave_parts` entry in a `.ustx` bundle;
- an empty `svpGroupId` in a `.ustx` bundle, so no Synthesizer V identity is
  invented for a project that has none;
- exact relative audio references that remain under the bundle root;
- a muted full-score reference track;
- valid mute state and group identity;
- source and artifact size/SHA-256 matches;
- one preservation disposition for every inventoried source item;
- no duplicate, missing, or traversal path among the manifest-declared
  artifacts, and no ledger reference outside that declared set.

Verification is manifest-driven: it checks everything the manifest declares and
never walks the bundle directory, so an extra unrelated file placed inside a
committed bundle is not detected. See
[Bundle format](bundle-format.md#integrity-validation).

Verse does not downgrade a failed Part extraction to a mixed-only bundle and
does not substitute a renderer output from another path.

## Transactional publication

A complete destination must:

- end in `.versebundle`;
- have an existing parent directory;
- not already exist;
- use a source filename that can be represented safely.

Verse stages the bundle in the destination's parent directory, creates files
without replacement, flushes staged data, validates the complete bundle, and
publishes with an operating-system no-replace rename. It never overwrites an
existing bundle.

If a destination appears during a race, the external destination is preserved.
Any renderer, validation, serialization, I/O, or commit failure rolls back the
private staging directory and leaves no published partial bundle.

## Structured error codes

The desktop command boundary returns a stable code, a message, and sometimes a
remediation.

| Code | Meaning |
|---|---|
| `UNSUPPORTED_FILE` | Extension is outside the accepted input set |
| `SOURCE_TOO_LARGE` | Top-level source exceeds 128 MiB |
| `SOURCE_READ_FAILED` | Source cannot be read as a regular file |
| `SOURCE_PARSE_FAILED` | Format, encoding, archive, or source structure is invalid |
| `CONVERSION_FAILED` | An exact projection to the selected target could not be produced |
| `INVALID_OUTPUT` | The output path is invalid, already exists, is the source, or its extension does not match the selected export target |
| `SERIALIZE_FAILED` | Encoding the target's own model into bytes failed. Distinct from `CONVERSION_FAILED`, which says the source cannot be represented |
| `WRITE_FAILED` | Vocals-only output could not be written |
| `INVALID_DESTINATION` | Bundle destination is not a valid new `.versebundle` directory |
| `DESTINATION_EXISTS` | Verse refused to overwrite an existing destination |
| `INVALID_SOURCE_NAME` | Source filename is unsafe or unrepresentable |
| `RENDERER_NOT_FOUND` | No qualified MuseScore executable was found |
| `RENDERER_UNSUPPORTED` | Renderer identity, version, capability, or score compatibility failed |
| `RENDERER_TIMEOUT` | Renderer exceeded its bounded deadline |
| `RENDERER_FAILED` | Renderer process, Part extraction, or WAV validation failed |
| `STEM_PLAN_INVALID` | Source Parts cannot form an exact one-stem-per-Part plan |
| `PRESERVATION_INCOMPLETE` | Preservation ledger is incomplete or inconsistent |
| `BUNDLE_INTEGRITY_FAILED` | Staged source, SVP, audio, IDs, hashes, or references do not agree |
| `BUNDLE_IO_FAILED` | Bundle staging or filesystem I/O failed |
| `BUNDLE_SERIALIZE_FAILED` | Bundle manifest or ledger serialization failed |
| `BUNDLE_COMMIT_FAILED` | Atomic no-replace publication failed |
| `BUNDLE_TASK_FAILED` | Background bundle worker did not complete |

Only `RENDERER_*` codes mean that audio rendering is unavailable. Other codes
must not be presented as a harmless “download the complete bundle” fallback.

## Reporting security defects

Preserve the original input and the structured error, but do not publish
copyrighted fixtures or user paths in a public report. A useful minimized
report identifies:

- input format and producing application/version;
- Verse version and operating system;
- MuseScore version when rendering is involved;
- the structured error code and sanitized message;
- whether the failure occurred during analysis, vocal-only export, Part
  extraction, rendering, validation, or publication;
- a minimal redistributable or synthetic reproduction when possible.
