# Corpus testing and licensing

Verse separates format coverage from rights clearance. A file being
downloadable does not make its lyrics, arrangement or transcription reusable.
Automated corpus runners therefore use only pinned, license-verifiable sources;
private songs stay outside Git.

## Automated public corpus

### OpenScore Lieder

[OpenScore Lieder](https://github.com/OpenScore/Lieder) is the primary public
vocal-score corpus. Its current catalogue metadata describes 1,356
nineteenth-century songs; the pinned checkout used by Verse contains 1,352
canonical MuseScore `.mscx` files plus extracted lyric text. The scores are
released under
[CC0 1.0](https://github.com/OpenScore/Lieder/blob/main/LICENSE.txt).

Verse pins commit
`6b2dc542ce2e8aa4b78c8ee62103b210efc07015`. The runner refuses a foreign
remote, a different commit or a checkout without the license evidence. It
stores the checkout and report under the ignored `src-tauri/target/` tree, so
corpus files are never committed.

```sh
scripts/run-openscore-corpus.sh --full-parse

VERSE_MUSESCORE_GATE="/path/to/mscore" \
scripts/run-openscore-corpus.sh --full-parse --render-sample
```

`--full-parse` audits every canonical `.mscx` score. `--render-sample [N]`
selects scores by ascending SHA-256 of their relative path, then renders the
full score and every extracted source Part. The default sample size is three.
Both modes write an atomic JSON report containing the pinned corpus identity,
per-file topology/projection results, typed errors and render coverage.

The final gate for the pinned commit currently records:

- 1,352 discovered score files;
- 1,343 parsed files and 1,277 exact SVP projections;
- 75 `ineligibleEvidenceFiles`;
- 0 `unexpectedErrors`, 0 evidence-invariant failures and 0 render errors;
- three deterministic scores rendered by MuseScore 4.7.4, with all six
  expected source Parts extracted and rendered as stems.

An evidence-ineligible file is not counted as a passing projection. It is a
narrowly classified source whose exact semantics cannot fit the current
projection contract: irregular/pickup measure boundaries, a truly intra-measure
meter change, conflicting global measure durations, or invalid/ambiguous repeat
navigation. The corpus runner measures the Synthesizer V target; a source it
projects exactly is not necessarily exportable to `.ustx`, whose 480-tick grid
accepts a strict subset. Unknown parse/projection errors always remain
`unexpectedError` and fail the runner. MuseScore 2 implicit voices, tuplets
and local `stretchN/stretchD` meters are parsed rather than allowlisted.

Useful overrides:

```sh
scripts/run-openscore-corpus.sh \
  --full-parse \
  --render-sample 5 \
  --renderer "/Applications/MuseScore 4.app/Contents/MacOS/mscore" \
  --cache "/external-disk/openscore-lieder" \
  --report "/external-disk/reports/openscore-lieder.json"
```

## Private regression corpus

The user-supplied `.kar`, `.mxl` and `.mscz` fixtures are intentionally
untracked. They cover exact regressions that an art-song corpus cannot cover:
Soft Karaoke control streams, detached lyric ownership, chord ambiguity,
lyrics-free MIDI and MuseScore/MusicXML topology parity.

```sh
VERSE_CORPUS_DIR="/path/to/private-corpus" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test corpus \
  -- \
  --ignored
```

The audited KAR expectations are exact, including 314 source-backed lyrics for
`Beatles - HELP.kar`, zero projected lyrics for the ambiguous Queen file and
eight unresolved chord pitches for Cabaret. These files must never be copied
into the repository or a release artifact.

## Additional sources evaluated

| Source | Scale / formats | Rights and lyric suitability | Verse policy |
|---|---|---|---|
| [PDMX](https://zenodo.org/records/14648209) | More than 250,000 MusicXML scores; the MXL archive is about 1.9 GB | Published as public-domain/CC0 data, but the maintainers report 31,221 files whose internal copyright metadata conflicted with the website metadata. It is not specifically a vocal-lyrics corpus. | Useful later for large structural/negative testing only after filtering out every `license_conflict` row. Not downloaded automatically. |
| [Mutopia](https://www.mutopiaproject.org/) | More than 2,000 public-domain editions; generated MIDI and LilyPond sources | Each contribution declares Public Domain, CC BY or CC BY-SA. MIDI is mainly a playback preview and often has no timed lyrics. | Good negative MIDI/instrument coverage after per-item license filtering; not treated as a KAR lyric oracle. |
| [Choral Public Domain Library](https://www.cpdl.org/wiki/index.php/ChoralWiki:Copyrights) | Large choral library; MIDI, source notation and occasional MusicXML/MXL | CPDL requires checking the license of each edition and warns that external links can have different terms. | Manual, manifest-based fixtures only. No bulk scraper. |
| [Wikimedia Commons KAR category](https://commons.wikimedia.org/wiki/Category:Kar_files) | Small number of `.kar`/MIDI karaoke files | License and source information are per file. | Suitable for a few reviewed public fixtures, not a homogeneous bulk corpus. |

Random MuseScore.com downloads, abandoned KAR archives and file-sharing
directories are excluded: download access alone does not establish the rights
to redistribute lyrics or arrangements. Synthetic MIDI/KAR tests remain the
safest way to exhaustively exercise malformed events, encodings, controls and
ambiguous mappings.

## Coverage model

No single corpus proves every property:

- OpenScore Lieder stresses real vocal lyrics, Unicode paths, Parts, staves,
  voices, chords and MuseScore rendering.
- The private corpus locks the reported KAR and MuseScore/MusicXML regressions.
- Generated tests cover corrupt archives, oversized payloads, path traversal,
  incomplete notes, lyric controls, ambiguous timings and transactional
  rollback.
- PDMX or Mutopia can later expand lyric-free and highly polyphonic negative
  coverage without weakening the evidence rules.
