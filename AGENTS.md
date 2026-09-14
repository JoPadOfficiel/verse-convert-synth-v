<!-- bmad:context -->
<!-- Verified 2026-09-14 against 93382860256c379132f78922dccaa843988de6ee. Managed by bmad-project-context; edits inside this block are replaced on refresh. Keep anything you want preserved outside the markers. -->

## Verse

Verse is an offline Tauri desktop application that converts source-owned MIDI/KAR, MusicXML and MuseScore evidence into editable Synthesizer V or OpenUtau vocal projects and preservation bundles. Rust owns musical meaning, provenance and artifact integrity; React owns presentation and user interaction. Authoritative architecture and fidelity documentation lives in `docs/`; BMAD working artifacts live locally under the ignored `_bmad-output/`.

## Policy

- Keep the installed application offline and local-first. Do not add telemetry, runtime model downloads, remote classification services or backend dependencies.
- Never invent lyrics, pitches, notes, holds, rests, instruments or audio. Preserve uncertain material in source evidence or reject it with a stable diagnostic.
- Never overwrite existing inputs or outputs. Direct exports and bundle publication remain no-replace operations.
- Keep shipped UI copy and diagnostics in English.
- Use Conventional Commits; Release Please derives release metadata from them.

## Where things are

- Source evidence and parsers: `src-tauri/src/engine/{midi,musicxml,musescore}.rs`; projection policy: `convert.rs`; target-neutral seam: `projection.rs`.
- Language ownership and pronunciation routing: `engine/language.rs` and `engine/target/{lexical,french,english}.rs`.
- Target dispatch and exactness gate: `engine/target/mod.rs`; serializers: `engine/target/{svp,ustx}.rs`.
- Tauri DTO changes must stay synchronized between `src-tauri/src/lib.rs`, `src/lib/tauri.ts`, frontend behavior and contract tests.
- Renderer process policy lives in `renderer.rs`; bundle ledger, validation and transactional publication live in `bundle.rs`.
- Read `docs/formats-and-fidelity.md` before changing musical ownership or pronunciation; read `docs/testing.md` before final verification.

## Running and verifying

- Match CI when tool versions matter: Node 22, Rust 1.93.0, and .NET 10 for the OpenUtau compatibility gate.
- `npm test` requires Chrome or Chromium; `npm run test:openutau:compat` requires network access and .NET 10 because it fetches the exact pinned OpenUtau revision.
- MuseScore is unnecessary for parsing or vocals-only export. Real complete-bundle rendering requires a user-installed MuseScore Studio 3.6.2+ or 4.x with supported Part extraction.
- Follow the core gate in `docs/testing.md`; private real-score gates stay optional and their fixtures must remain outside Git.

## Conventions that differ from defaults

- Source adapters emit richer evidence into the shared IR; never repair source-format semantics inside an export serializer.
- Keep `ProjectedProject` target-neutral. Grid conversion, markers, cosmetics and schema details belong only to the selected target adapter.
- A target-specific representation failure must be detected by the analysis `validate_for` gate; never discover it only during export.
- Refuse unrepresentable timing instead of rounding, clamping or inserting musical defaults.
- Keep source role, export representation, language ownership and mixer state as separate concepts.
- Automatic FR+EN classifies complete source-owned words with bundled English/French Lingua data plus Verse lexicons and passage context; it must remain deterministic, offline and non-translating.
- Automatic OpenUtau words route French through Millefeuille and English through the English DiffSinger/ARPAbet path. Never fall back to OpenUtau's Default phonemizer for classified text.
- Synthesizer V may share Automatic FR+EN ownership and diagnostics but must receive no OpenUtau `phonemizer`, `fr/...` or `en/...` metadata.
- OpenUtau mixed per-word switching requires 0.1.569+. Treat the track phonemizer only as fallback metadata; use native per-note overrides on complete word heads.
- Preserve stable source IDs, deterministic observable ordering, checked arithmetic and stable diagnostic codes. UI behavior must not depend on diagnostic message text.

## Known pitfalls

- An untexted, unlinked vocal note does not become an empty vocal lyric: keep it out of the vocal project while preserving it in source evidence and applicable audio.
- Source-proven continuations, melismas and their owning predecessor must stay together; do not infer holds from ordinary wordless MIDI notes.
- Lyrics remain owned by their source Part/staff/voice/lane/pass. Never copy a words track, globally merge verses or use global nearest-note assignment.
- Malformed fragment recovery may reconstruct a dictionary-attested complete word for pronunciation/rendering, but each original raw fragment must remain unchanged in source evidence (`Beat` + `tles` may render `beatles`, never destroy the two source fragments).
- Base USTX facts may come from the qualified 0.1.568 source, but per-note `phonemizer` behavior is a 0.1.569+ fact. Verify format behavior against the exact pinned OpenUtau source rather than documentation alone.

<!-- /bmad:context -->