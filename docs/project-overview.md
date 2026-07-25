# Verse Project Overview

**Date:** 2026-07-25  
**Type:** Offline desktop application  
**Architecture:** Tauri modular monolith with a React webview and Rust
conversion engine

## Executive summary

Verse converts `.kar`, `.mid`, `.midi`, `.mxl`, `.xml`, `.musicxml`, `.mscz`,
and `.mscx` sources into Synthesizer V Studio 1.x projects. Its central rule is
that output may contain only evidence present in the source or an explicit
user choice. Missing lyrics do not become `la`, generic MIDI Text does not
become singing, and instrumental notes do not become fake vocal notes.

Verse offers two outputs:

1. A vocal-only `.svp` containing editable, evidence-backed vocal notes.
2. A complete `.versebundle` containing the byte-identical source, the vocal
   project, one MuseScore-rendered WAV stem per note-bearing source Part, a
   muted full-score reference, a manifest, and a complete preservation ledger.

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
- **Target application:** Synthesizer V Studio 1.x, raw SVP project version
  113

## Technology stack

| Layer | Technology | Responsibility |
|---|---|---|
| Desktop shell | Tauri 2 | Native window, IPC, dialogs, packaging |
| Frontend | React 19, TypeScript 5.8 | File selection, inspection, settings, export controls |
| Styling | Tailwind CSS 4, Radix UI primitives | Theme-aware desktop interface |
| Domain/backend | Rust 2021 | Parsing, topology, lyric evidence, projection, bundle integrity |
| Source parsing | Custom Rust parsers, `roxmltree`, `zip` | MIDI/KAR, MusicXML/MXL, MuseScore/MSCZ |
| Serialization | `serde`, `serde_json` | Tauri DTOs, SVP, manifests, ledgers, audit reports |
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
- Exact tempo and meter projection where Synthesizer V can represent it.
- Explicit rejection of ambiguous timing/navigation instead of truncation or
  approximation.
- One audio stem per note-bearing source Part, keeping technical chord lanes
  grouped with their source Part.
- Transactional, no-replace bundle publication with complete hash and
  reference validation.
- Batch analysis and sequential complete-project export.
- English/French Synthesizer V database language selection without
  translating source lyrics.

## User workflow

1. Drop or select one or more supported files.
2. Verse parses each source and reports its Parts, voices, source roles,
   lyric status, projected lyric count, and warnings.
3. Optionally enable or disable eligible vocal projection lanes at the Part
   level.
4. Export a vocal-only `.svp`, or export a complete `.versebundle`.
5. Open the project in Synthesizer V Studio and assign a compatible voice
   database to every vocal track.

Synthesizer V cannot sing without a voice database. Verse does not bundle,
select, or license commercial voices.

## Preservation guarantees

Verse guarantees the following for a successful complete bundle:

- the original source bytes are copied unchanged;
- every inventoried source item has exactly one primary disposition;
- every disposition points to at least one preserving artifact;
- every expected note-bearing Part has exactly one validated stem;
- the full-score reference and all stems are valid, non-empty WAV files;
- the SVP audio references resolve inside the bundle;
- manifest sizes and SHA-256 hashes match the committed files;
- an existing destination is never overwritten;
- any failure before commit removes only Verse-owned staging.

“Source-faithful” does not mean every notation concept is editable in SVP.
Unsupported or ambiguous material stays in the source, ledger, and rendered
audio, or causes a clear failure when exact output cannot be proven.

## MuseScore requirement

Only one compatible MuseScore version is required. MuseScore Studio 4.x is
recommended because it can open current MuseScore files. MuseScore 3.6.2+ is
supported for inputs it can open, but MuseScore 3 cannot render a native
MuseScore 4 score. Verse probes the selected executable, verifies its
`--score-parts` capability, records its version and SHA-256 identity, and
rejects unsupported future major versions.

Analysis and vocal-only `.svp` export do not invoke MuseScore. Complete bundle
export always does.

## Explicit limitations

- Synthesizer V does not provide editable general-purpose instrument tracks;
  accompaniment is therefore represented by real audio stems.
- The Part stems are MuseScore score-Part renders, not AI-separated,
  vocal-removed stems.
- SMPTE-timed MIDI is preserved but cannot currently be projected to SVP.
- MIDI format 2 independent sequences are rejected rather than flattened.
- Meter changes inside a measure are rejected.
- Complex or ambiguous repeat/navigation graphs are rejected.
- Native MuseScore tie/spanner graphs stay in the original score and audio;
  MusicXML start/stop tie chains are projected.
- XML encodings outside UTF-8, UTF-16, ISO-8859-1, and Windows-1252 are
  rejected.
- Released packages are currently unsigned.
- Linux artifacts are build-qualified; macOS and Windows are the documented
  user platforms.

## Repository structure

- `src/` — React UI and typed Tauri adapter.
- `src-tauri/src/engine/` — source parsers, shared musical model, projection,
  and SVP serializer.
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

---

_Generated using the BMAD Method `document-project` workflow._
