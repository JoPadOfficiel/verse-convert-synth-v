# Verse Documentation

**Project type:** Offline Tauri desktop application  
**Primary languages:** Rust and TypeScript  
**Architecture:** Modular desktop monolith with a provenance-preserving
conversion pipeline  
**Documentation baseline:** `e2a717cd5a0756a089f890478882045dcdf16e7c`  
**Last updated:** 2026-07-25

## Start here

- [Project overview](project-overview.md) — purpose, supported inputs, outputs,
  guarantees, and limitations.
- [Installation and development](development-guide.md) — prerequisites,
  MuseScore 3/4 setup, local development, and quality gates.
- [Troubleshooting](troubleshooting.md) — missing lyrics, silent vocals,
  missing instruments, renderer errors, and platform-specific fixes.
- [Architecture](architecture.md) — current runtime design, dependency
  boundaries, data flow, decisions, and planned convergence.

## Product and format documentation

- [Formats and fidelity](formats-and-fidelity.md) — MIDI/KAR, MusicXML,
  MuseScore, lyric ownership, topology, and SVP projection rules.
- [Bundle format](bundle-format.md) — `.versebundle` schema v2 layout,
  manifest, preservation ledger, stem policy, integrity, and atomic commit.
- [MuseScore renderer](musescore-renderer.md) — supported versions,
  installation, discovery, compatibility, process isolation, and rendering
  limits.
- [Tauri command contracts](tauri-command-contracts.md) — frontend/backend
  DTOs, commands, structured errors, and authority boundaries.

## Engineering documentation

- [Source tree analysis](source-tree-analysis.md) — annotated repository
  structure and entry points.
- [Component inventory](component-inventory.md) — Rust modules, React
  components, shared helpers, and external adapters.
- [Testing](testing.md) — unit, integration, private corpus, OpenScore, and
  real-render gates.
- [Corpus testing and licensing](test-corpora.md) — public and private dataset
  policy.
- [Security and resource limits](security-and-limits.md) — trust boundaries,
  parser guards, renderer sandboxing, size limits, and no-replace publication.
- [Deployment and releases](deployment-guide.md) — CI, six-target builds,
  Release Please, immutable tags, checksums, and unsigned packages.
- [Contributing](contribution-guide.md) — change ownership, required tests,
  documentation, and Conventional Commits.

## Quick setup

You need only **one** compatible MuseScore installation:

- MuseScore Studio **4.x** is recommended and is required for native
  MuseScore 4 files.
- MuseScore **3.6.2 or later in the 3.x line** is supported for files it can
  open.
- MuseScore is optional for analysis and vocal-only `.svp` export, but it is
  required for complete `.versebundle` exports.

For development:

```sh
npm ci
npm run version:check
npm test
npm run build
npm run tauri dev
```

Then run the Rust gates documented in
[Installation and development](development-guide.md#required-quality-gates).

## Documentation authority

Current source, tests, this `docs/` directory, `README.md`, and the latest
release metadata are the operational authority. Files under
`_bmad-output/` are local planning and historical evidence; they are not
shipped in Git. The authoritative BMAD architecture has therefore been
reconciled into [architecture.md](architecture.md).

---

_Generated and reconciled with the BMAD Method `document-project` workflow._
