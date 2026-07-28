# Contribution Guide

Verse converts source-owned musical evidence into Synthesizer V vocal
projects and preservation bundles. Contributions must protect that evidence:
successful conversion is never more important than avoiding fabricated or
silently discarded content.

## Development baseline

Use the committed lockfiles and the same major toolchain lines as CI:

- Node.js 22;
- Rust 1.93.0 with `rustfmt` and `clippy`;
- npm dependencies installed with `npm ci`;
- Cargo dependencies resolved with `src-tauri/Cargo.lock`;
- the native Tauri prerequisites for the development operating system.

Start the frontend-only development server with:

```sh
npm run dev
```

Start the desktop application with:

```sh
npm run tauri -- dev
```

MuseScore is not required for parser, projection, or vocal-only SVP work. A
compatible user-installed MuseScore Studio 3.6.2 or 4 executable with
`--score-parts` support is required for real reference-mix and Part-stem bundle
tests.

## Architectural ownership

Keep responsibilities in their current boundaries:

- `src/`: React presentation and ephemeral interaction state;
- `src/lib/tauri.ts`: typed frontend mirror of Tauri commands and DTOs;
- `src-tauri/src/lib.rs`: Tauri command adapter and current use-case
  orchestration;
- `src-tauri/src/engine/midi.rs`: shared source IR and MIDI/KAR parsing;
- `src-tauri/src/engine/musicxml.rs`: MusicXML/MXL adapter;
- `src-tauri/src/engine/musescore.rs`: MSCX/MSCZ adapter;
- `src-tauri/src/engine/convert.rs`: source classification, lyric ownership,
  vocal projection, and diagnostics;
- `src-tauri/src/engine/projection.rs`: the target-neutral projection only — no
  blicks, colours, display order or rendered marker text;
- `src-tauri/src/engine/target/svp.rs`: SVP serialization only;
- `src-tauri/src/stems.rs`: source Part stem planning;
- `src-tauri/src/renderer.rs`: bounded MuseScore process adapter;
- `src-tauri/src/bundle.rs`: ledger, manifest, validation, and transactional
  publication.

A format-specific parser should emit richer source evidence rather than make a
target-specific repair. The projector may consume evidence, but it must not
reinterpret adapter syntax or infer facts from display names.

When a Tauri DTO changes, update the Rust serialized type, its TypeScript
mirror in `src/lib/tauri.ts`, frontend behavior, and contract-focused tests in
the same change.

## Fidelity rules

The following rules are release invariants:

- Absence of a lyric remains an empty lyric. Never insert `la`, `あ`, a
  default syllable, or a phonetic substitute.
- Insert `-` only when a source extension or continuation proves it.
- Preserve source lyric text. Vocal-language selection configures the target
  voice language; it does not translate or normalize the words.
- Generic MIDI Text is metadata. Treat it as a karaoke lyric only when the
  exact source track carries the required local karaoke evidence.
- Never invent C4 or another pitch for an unpitched source without an explicit
  source-owned MIDI mapping.
- Keep source role, export representation, and mixer state separate. A user
  vocal override changes projection, not source truth.
- Preserve ambiguous content as source-only with a stable diagnostic, or
  reject the operation. Do not choose a melody, voice, lane, Part, or rootfile
  by guess.
- Do not serialize arbitrary symbolic instrument notes as vocal tracks.
  Instrument playback in a complete bundle comes from validated audio stems.
- Missing Parts, incomplete stems, invalid audio, incompatible renderers, and
  unsafe timing fail closed. There is no mixed-only or audio-less bundle
  fallback.
- Existing output files and bundle directories are never overwritten.
- Stable source IDs, deterministic ordering, checked arithmetic, and explicit
  bounds are part of correctness, not optional hardening.
- A byte-identical source snapshot preserves what the current IR cannot
  project; it does not justify a claim that the target format is lossless.

Any new heuristic requires explicit evidence, a deterministic tie-break
contract, diagnostics for every unresolved case, and negative tests proving
that unrelated content is not captured.

## Code conventions

### Rust

- Use Rust 2021 idioms and `rustfmt`.
- Name modules, files, functions, and local variables in `snake_case`; use
  `PascalCase` for types and variants.
- Prefer typed enums and structs over stringly typed branching.
- Serialize IPC and persisted DTOs with
  `#[serde(rename_all = "camelCase")]` unless compatibility requires an
  explicit field name.
- Use `Path` and `PathBuf` for paths. Never build subprocess commands through
  a shell string.
- Use checked integer arithmetic at parser, timing, size, and count
  boundaries.
- Preserve deterministic source order with ordered collections where output
  order is observable.
- Return stable error or diagnostic codes; UI logic must not depend on the
  English wording of a message.
- Keep external processes bounded by time, output size, log size, a controlled
  environment, and process-tree termination.

### TypeScript and React

- Use `PascalCase` for components and exported type aliases; use `camelCase`
  for variables, helpers, props, and DTO fields.
- Keep frontend mirrors as explicit unions rather than weakening them to
  arbitrary strings or `any`.
- React may own selections, expanded state, theme, and busy presentation.
  Musical classification and conversion policy remain Rust-owned.
- Use the `@/` import alias for project modules.
- Keep UI text and shipped diagnostics in English.
- Preserve keyboard behavior, labels, focus visibility, and disabled states
  when changing controls.

## Tests required for every change

Run the complete local gate before requesting review:

```sh
npm run version:check
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked --all-targets
git diff --check
```

Add tests at the boundary that owns the behavior:

| Change | Required test location |
| --- | --- |
| MIDI/KAR syntax or qualification | unit tests in `engine/midi.rs` |
| Lyric binding, roles, overrides, projection | unit tests in `engine/convert.rs` |
| MusicXML/MXL behavior | unit tests in `engine/musicxml.rs` |
| MuseScore/MSCZ behavior | unit tests in `engine/musescore.rs` |
| Cross-format semantic parity | `src-tauri/tests/parity.rs` |
| Source and no-invention contract | `src-tauri/tests/source_fidelity.rs` |
| Stem planning | unit tests in `stems.rs` |
| MuseScore process, capabilities, limits, WAV validation | unit tests in `renderer.rs` |
| Bundle layout, ledger, rollback, integrity | unit tests in `bundle.rs` |
| Tauri output safety or command mapping | tests in `lib.rs` and frontend command tests |
| Frontend helpers and state-independent behavior | `tests/*.test.mjs` |

A regression fix must include a negative test that fails under the old
behavior. For fidelity changes, test both the intended positive case and a
nearby ambiguous or unrelated case that must remain untouched.

## Corpus testing and fixture policy

Do not commit copyrighted songs or rights-unclear fixtures.
`src-tauri/tests/fixtures/` is ignored for this reason. Private corpus files
must stay outside Git and are loaded only through `VERSE_CORPUS_DIR`.

Run the private regression matrix with:

```sh
VERSE_CORPUS_DIR="/absolute/path/to/private-corpus" \
  cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test corpus \
  -- \
  --ignored
```

Requesting this ignored test is strict: missing or renamed expected fixtures
must fail rather than produce a silent pass. Do not commit its inputs,
generated SVP files, rendered WAV files, or reports.

The public bulk gate uses the pinned CC0 OpenScore Lieder checkout:

```sh
scripts/run-openscore-corpus.sh --full-parse
```

To include deterministic real MuseScore rendering:

```sh
VERSE_MUSESCORE_GATE="/absolute/path/to/mscore" \
  scripts/run-openscore-corpus.sh --full-parse --render-sample
```

The runner verifies the repository URL, exact commit, clean checkout, and
license evidence. Its cache and JSON reports live under the ignored
`src-tauri/target/` tree. See [Test Corpora](test-corpora.md) for the pinned
baseline and corpus scope.

Parser, topology, KAR lyric-binding, stem, or renderer changes require the
relevant private corpus gate and the public full-parse gate before release.
Renderer or bundle changes also require a deterministic render sample with a
compatible real MuseScore executable.

## Commits, changelog, and versions

Use Conventional Commits so Release Please can classify changes and maintain
the changelog. Examples:

```text
feat(engine): preserve MusicXML lyric lanes
fix(kar): keep ambiguous external lyrics source-only
fix(renderer): reject incomplete MuseScore Part payloads
docs: explain bundle schema v2
test(corpus): cover rest-only source Parts
```

Use `feat` for user-visible functionality and `fix` for corrected behavior.
Use an optional scope when it makes the affected boundary clearer. Keep one
coherent change per commit and explain fidelity or compatibility consequences
in the body when the subject alone is insufficient.

Ordinary contributions should not manually bump versions or edit release
headings in `CHANGELOG.md`; Release Please prepares those changes in its
release pull request. If a release metadata change is intentionally performed
outside that automation, all version sources listed in
[the deployment guide](deployment-guide.md) must change together and
`npm run version:check` must pass.

Do not rewrite historical changelog entries. Document a correction in the next
release entry.

## Pull request expectations

A reviewable pull request should:

1. State the source evidence or user-visible failure being addressed.
2. Identify the owning parser, domain rule, renderer, transaction, or UI
   boundary.
3. Explain why the change does not invent or silently discard musical data.
4. List new positive and negative regression tests.
5. Report the complete local quality-gate result.
6. Report private/public corpus evidence when required.
7. Update README or `docs/` when behavior, setup, compatibility, bundle
   schema, commands, or release operations change.
8. Keep generated outputs, local corpora, editor state, credentials, and
   signing material out of the commit.

Reviewers should reject a change that passes common examples by adding a
fallback, guess, fabricated default, silent truncation, or unbounded parser or
process behavior.

## Release-sensitive changes

Changes to persisted SVP or bundle schemas, renderer qualification, Tauri IPC,
resource limits, supported platforms, version files, GitHub Actions, or
release asset naming require explicit compatibility review.

For workflow changes:

- keep third-party actions pinned to immutable commit SHAs;
- retain exact commit and tag validation;
- retain `npm ci`, Cargo `--locked`, and all quality gates;
- preserve all six expected build targets unless a documented platform policy
  change is approved;
- preserve checksum generation and verification;
- test draft behavior before enabling publication;
- account for both Release Please and tag-push triggers.

Publishing a release is a maintainer operation. A contribution may prepare the
source and release metadata, but it must not move an existing tag or replace
the assets of an already published release.
