# Verse Source Tree Analysis

**Date:** 2026-07-25

## Overview

Verse is one Tauri application. The React code is the presentation adapter;
the Rust crate owns source interpretation, conversion, external process
control, filesystem transactions, and persisted artifacts.

## Directory structure

```text
.
├── .github/workflows/
│   ├── ci.yml
│   ├── build.yml
│   ├── release-please.yml
│   └── release-tag.yml
├── docs/
├── public/
├── scripts/
│   ├── check-version.mjs
│   └── run-openscore-corpus.sh
├── src/
│   ├── components/
│   │   ├── ui/
│   │   ├── Dropzone.tsx
│   │   ├── FileList.tsx
│   │   ├── Settings.tsx
│   │   └── theme-provider.tsx
│   ├── lib/
│   │   ├── file-utils.ts
│   │   ├── tauri.ts
│   │   ├── utils.ts
│   │   └── vocal-overrides.ts
│   ├── App.tsx
│   ├── index.css
│   └── main.tsx
├── src-tauri/
│   ├── capabilities/default.json
│   ├── icons/
│   ├── src/
│   │   ├── bin/corpus_audit.rs
│   │   ├── engine/
│   │   │   ├── convert.rs
│   │   │   ├── midi.rs
│   │   │   ├── musescore.rs
│   │   │   ├── musicxml.rs
│   │   │   ├── projection.rs
│   │   │   └── target/
│   │   │       ├── mod.rs
│   │   │       └── svp.rs
│   │   ├── bundle.rs
│   │   ├── lib.rs
│   │   ├── main.rs
│   │   ├── renderer.rs
│   │   └── stems.rs
│   ├── tests/
│   │   ├── corpus.rs
│   │   ├── parity.rs
│   │   └── source_fidelity.rs
│   ├── Cargo.lock
│   ├── Cargo.toml
│   └── tauri.conf.json
├── tests/
├── CHANGELOG.md
├── README.md
├── package-lock.json
├── package.json
├── release-please-config.json
├── tsconfig.json
└── vite.config.ts
```

Generated and private trees are intentionally excluded:

- `node_modules/`, `dist/`, and `src-tauri/target/`;
- `src-tauri/gen/`;
- `src-tauri/tests/fixtures/`, which may contain private copyrighted scores;
- `_bmad/`, `_bmad-output/`, `.agents/`, and other local agent tooling.

## Entry points

| Entry point | Purpose |
|---|---|
| `src/main.tsx` | Mounts React Strict Mode, theme provider, and `App` |
| `src/App.tsx` | Owns transient UI state and all user workflows |
| `src-tauri/src/main.rs` | Starts the desktop binary |
| `src-tauri/src/lib.rs::run` | Configures Tauri plugins and command handlers |
| `src-tauri/src/bin/corpus_audit.rs` | Standalone public-corpus audit CLI |

## Critical directories

### `src-tauri/src/engine/`

The source and target format boundary. It contains the shared musical model,
source topology, exact event evidence, MIDI/KAR parser, MusicXML and MuseScore
adapters, vocal projection, and SVP v113 serialization.

Format-specific rules stay in the owning parser. Cross-format lyric, timing,
role, and projection semantics stay in `midi.rs` and `convert.rs`.

### `src-tauri/src/`

The application and infrastructure boundary:

- `lib.rs` validates command inputs, parses immutable snapshots, projects
  vocals, builds a stem plan, and invokes exports.
- `stems.rs` maps one note-bearing source Part to one stable stem descriptor.
- `renderer.rs` probes and runs MuseScore with fixed arguments and bounded
  resources.
- `bundle.rs` builds the preservation ledger, renders artifacts, validates the
  complete staged bundle, and commits it without replacement.

### `src/`

The untrusted presentation boundary. It may select files through Tauri dialogs,
hold draft choices, invoke typed commands, and display results. It does not
parse music, launch MuseScore, or write output directly.

### `src-tauri/tests/`

Cross-module behavior:

- `parity.rs` compares musical semantics across adapters.
- `source_fidelity.rs` verifies public no-invention and preservation contracts,
  with optional real `.mxl`/`.mscz` fixtures.
- `corpus.rs` runs exact expectations over a private, ignored multi-format
  corpus.

### `.github/workflows/`

- `ci.yml` runs all mandatory quality gates.
- `release-please.yml` prepares synchronized release changes.
- `release-tag.yml` resolves a pushed tag to an immutable commit.
- `build.yml` validates and builds six platform targets, assembles stable
  assets, creates `SHA256SUMS`, and publishes only after verification.

## File organization patterns

- Rust modules and files use `snake_case`.
- React components use PascalCase files and exported names.
- Frontend utilities use kebab-case files and camelCase functions.
- Frontend local imports use the `@/` alias.
- Rust DTOs serialize with camelCase and are mirrored explicitly in
  `src/lib/tauri.ts`.
- Tests live beside owning Rust modules for internal rules and in
  `src-tauri/tests/` for public cross-module behavior.
- Persisted JSON contracts are independent of UI labels.

## Assets

- `src-tauri/icons/` contains desktop, Windows Store, Android, and iOS icon
  variants generated for Tauri packaging.
- `docs/screenshot.png` is the README product screenshot.
- `public/` and `src/assets/` retain scaffold assets; they are not conversion
  inputs or runtime musical assets.

## Configuration files

| File | Responsibility |
|---|---|
| `package.json` / `package-lock.json` | Frontend dependencies, commands, and locked versions |
| `src-tauri/Cargo.toml` / `Cargo.lock` | Rust crate and locked dependencies |
| `src-tauri/tauri.conf.json` | Product identity, window, CSP, and bundles |
| `src-tauri/capabilities/default.json` | Least-privilege webview permissions |
| `vite.config.ts` | React/Tailwind plugins and Tauri development server |
| `tsconfig.json` | Strict TypeScript and `@/` alias |
| `release-please-config.json` | Version and changelog automation |
| `.release-please-manifest.json` | Current release version |

## Development notes

The current source is intentionally a modular monolith, but orchestration still
resides mainly in `src-tauri/src/lib.rs`. The BMAD target architecture plans a
future `application/` use-case layer, backend-issued handles, immutable plans,
and typed jobs. Those concepts are not current runtime contracts and must not
be documented or consumed as already implemented.

---

_Generated using the BMAD Method `document-project` workflow._
