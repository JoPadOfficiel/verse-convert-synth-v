# Verse Project Overview

**Date:** 2026-07-28  
**Type:** Offline desktop application  
**Architecture:** Tauri modular monolith with a React webview and Rust
conversion engine

## Executive summary

Verse converts `.kar`, `.mid`, `.midi`, `.mxl`, `.xml`, `.musicxml`, `.mscz`,
and `.mscx` sources into singing-synthesis projects for **two** target
applications: Synthesizer V Studio 1.x (`.svp`) and OpenUtau (`.ustx`). Its
central rule is that output may contain only evidence present in the source or an
explicit user choice. Missing lyrics do not become `la` or `a`, generic MIDI Text
does not become singing, and instrumental notes do not become fake vocal notes.

Verse offers two outputs:

1. A vocals-only project — `.svp` or `.ustx`, whichever the **Export target**
   selector names — containing editable, evidence-backed vocal notes.
2. A complete `.versebundle` containing the byte-identical source, the project in
   the selected target's format, one MuseScore-rendered WAV stem per note-bearing
   source Part, a muted full-score reference, a manifest, and a complete
   preservation ledger. Both project variants reference the same stems.

The application is local-first and offline at runtime. It has no account,
server, database, telemetry, or cloud dependency.

## Repository classification

- **Repository type:** Single repository / monolith
- **Product type:** Cross-platform desktop application
- **Primary languages:** Rust 2021 and TypeScript
- **Runtime boundary:** React webview ↔ typed Tauri commands ↔ Rust domain and
  adapters
- **External runtime dependency:** A user-installed MuseScore Studio renderer
  for complete bundles
- **Target applications:** Synthesizer V Studio 1.x (raw SVP project version
  113) and OpenUtau (`.ustx` at `ustx_version` 0.6)

## Technology stack

| Layer | Technology | Responsibility |
| --- | --- | --- |
| Desktop shell | Tauri 2 | Native window, IPC, dialogs, packaging |
| Frontend | React 19, TypeScript 5.8 | File selection, inspection, settings, export controls |
| Styling | Tailwind CSS 4, Radix UI primitives | Theme-aware desktop interface |
| Domain/backend | Rust 2021 | Parsing, topology, lyric evidence, projection, bundle integrity |
| Source parsing | Custom Rust parsers, `roxmltree`, `zip` | MIDI/KAR, MusicXML/MXL, MuseScore/MSCZ |
| Serialization | `serde`, `serde_json` | Tauri DTOs, SVP, manifests, ledgers, audit reports |
| USTX serialization | Hand-written deterministic YAML emitter | Byte-exact `.ustx`, every string scalar double-quoted; no YAML dependency |
| Rendering | MuseScore Studio 3.6.2+ or verified 4.x | Full-score and source-Part WAV rendering |
| Delivery | GitHub Actions, Release Please | Quality gates, multi-platform packages, tags, releases |

The lock files are build authority. CI uses Node.js 22 and Rust 1.93.0.

## Core capabilities

- Content-aware parsing of all supported extensions with a 128 MiB input cap.
- Stable Part → staff → voice → projection-lane topology for score formats.
- Exact lyric ownership, lyric-lane separation, line order, syllabic state,
  continuation evidence, and source provenance.
- Conservative Soft Karaoke support that binds external lyric streams only
  when the mapping is unique, complete, injective, and monotonic.
- Valid lyric-free MIDI conversion without generated words or tracks.
- Two export targets behind one target-neutral projection: Synthesizer V `.svp`
  and OpenUtau `.ustx`, each owning only its own grid, marker vocabulary and
  schema version.
- Exact tempo and meter projection where the selected target can represent it,
  with the exactness gate run at analysis time so a refusal never appears for
  the first time at export.
- Explicit rejection of ambiguous timing/navigation instead of truncation or
  approximation — including timing that misses OpenUtau's 480-tick grid, which
  OpenUtau's own MusicXML importer silently truncates.
- An untexted note written as `lyric: ""` in a `.ustx`, a state no OpenUtau
  importer can express, carried on a muted companion lane so the sung track
  holds only real words while nothing is deleted or filled in.
- One audio stem per note-bearing source Part, keeping technical chord lanes
  grouped with their source Part.
- Transactional, no-replace bundle publication with complete hash and
  reference validation.
- Batch analysis and sequential complete-project export.
- Byte-exact lyrics in any language with nothing to configure; there is no
  language selector.

## User workflow

1. Drop or select one or more supported files.
2. Verse parses each source and reports its Parts, voices, source roles,
   lyric status, projected lyric count, and warnings.
3. Choose the export target. Changing it re-analyses every loaded source,
   because the exactness gate is target-dependent.
4. Optionally enable or disable eligible vocal projection lanes at the Part
   level.
5. Export a vocals-only `.svp` or `.ustx`, or export a complete
   `.versebundle`.
6. Open the project and assign a Synthesizer V voice database or an OpenUtau
   singer to every vocal track.

Neither application can sing without a voice. Verse does not bundle, select, or
license commercial voice databases or voicebanks, and names none in its output.

## Preservation guarantees

Verse guarantees the following for a successful complete bundle:

- the original source bytes are copied unchanged;
- every inventoried source item has exactly one primary disposition;
- every disposition points to at least one preserving artifact;
- every expected note-bearing Part has exactly one validated stem;
- the full-score reference and all stems are valid, non-empty WAV files;
- the project audio references resolve inside the bundle;
- manifest sizes and SHA-256 hashes match the committed files;
- an existing destination is never overwritten;
- any failure before commit removes only Verse-owned staging.

“Source-faithful” does not mean every notation concept is editable in `.svp` or
`.ustx`. Unsupported or ambiguous material stays in the source, ledger, and
rendered audio, or causes a clear failure when exact output cannot be proven.

## MuseScore requirement

Only one compatible MuseScore version is required. MuseScore Studio 4.x is
recommended because it can open current MuseScore files. MuseScore 3.6.2+ is
supported for inputs it can open, but MuseScore 3 cannot render a native
MuseScore 4 score. Verse probes the selected executable, verifies its
`--score-parts` capability, records its version and SHA-256 identity, and
rejects unsupported future major versions.

Analysis and vocals-only export do not invoke MuseScore. Complete bundle export
always does.

## Explicit limitations

- Synthesizer V does not provide editable general-purpose instrument tracks;
  accompaniment is therefore represented by real audio stems.
- A vocals-only `.ustx` carries `wave_parts: []` — no instruments, no audible
  reference. Only a bundled `.ustx` carries the stems.
- Nobody has yet opened a `.ustx` bundle in OpenUtau. The audio references are
  written and verified against the manifest and the WAVs; the listening check that
  the stems play in the application is outstanding.
- The Part stems are MuseScore score-Part renders, not AI-separated,
  vocal-removed stems.
- SMPTE-timed MIDI is preserved but cannot currently be projected to either
  target.
- MIDI format 2 independent sequences are rejected rather than flattened.
- Meter changes inside a measure are rejected.
- Complex or ambiguous repeat/navigation graphs are rejected.
- Native MuseScore tie chains are merged, as MusicXML tie chains are, but a tie
  whose head is written in a later `<voice>` container of the same measure stays
  unmerged. Both notes remain audible and nothing is invented.
- Timing that does not divide exactly into the selected target's grid is
  refused. OpenUtau's 480 ticks per quarter accept a strict subset of what
  Synthesizer V blicks accept, so a septuplet exports to `.svp` and is refused
  for `.ustx`.
- XML encodings outside UTF-8, UTF-16, ISO-8859-1, and Windows-1252 are
  rejected.
- Released packages are currently unsigned.
- Linux artifacts are build-qualified; macOS and Windows are the documented
  user platforms.

## Repository structure

- `src/` — React UI and typed Tauri adapter.
- `src-tauri/src/engine/` — source parsers, shared musical model, the
  target-neutral projection, and the `.svp`/`.ustx` serializers under
  `engine/target/`.
- `src-tauri/src/stems.rs` — stable Part-to-stem planning.
- `src-tauri/src/renderer.rs` — bounded MuseScore process adapter.
- `src-tauri/src/bundle.rs` — ledger, rendering orchestration, validation, and
  transactional publication.
- `src-tauri/src/lib.rs` — Tauri DTOs and commands.
- `src-tauri/tests/` — cross-format, fidelity, and private-corpus gates.
- `scripts/` — release-version and OpenScore corpus automation.
- `.github/workflows/` — CI and release delivery.

See [Source tree analysis](source-tree-analysis.md) for the complete annotated
layout.

## Documentation map

The master map is [docs/index.md](index.md). Architectural changes must update
[architecture.md](architecture.md), and changes to persisted output must also
update [bundle-format.md](bundle-format.md) or
[tauri-command-contracts.md](tauri-command-contracts.md).
