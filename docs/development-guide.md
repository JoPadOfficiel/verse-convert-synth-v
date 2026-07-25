# Verse Development Guide

**Baseline:** `e2a717cd5a0756a089f890478882045dcdf16e7c`

**Updated:** 2026-07-25

## Supported development environment

Verse is a Tauri 2 desktop application with a React/TypeScript frontend and a
Rust 2021 backend.

Required tools:

- Node.js **20.19 or later in the 20.x line**, or **22.12 or later**;
- npm with lockfile support;
- Rust and Cargo;
- Git;
- the operating-system prerequisites required by Tauri 2.

CI uses Node.js 22 and pins Rust **1.93.0** with `rustfmt` and `clippy`.
There is no repository `rust-toolchain.toml`, so install/select Rust 1.93.0
when reproducing CI exactly:

```sh
rustup toolchain install 1.93.0 --component rustfmt clippy
rustup run 1.93.0 rustc --version
```

Vite 7 is the reason for the precise Node floor. Node 22.0 through 22.11 does
not satisfy the locked Vite engine requirement.

## Operating-system prerequisites

### macOS

Install Xcode Command Line Tools:

```sh
xcode-select --install
```

### Windows

Install Visual Studio 2022 Build Tools with **Desktop development with C++**
and ensure the Microsoft Edge WebView2 runtime is available.

### Debian or Ubuntu Linux

The CI image installs:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
  build-essential \
  curl \
  file \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  libxdo-dev \
  patchelf \
  wget
```

Distribution package names may differ outside Debian/Ubuntu.

## MuseScore for development

MuseScore is not needed for frontend work, parser-only tests, source analysis,
or vocal-only `.svp` export. It is required for complete bundle exports,
real-render gates, and OpenScore render sampling.

Install one compatible executable:

- MuseScore Studio 4.x is recommended;
- MuseScore 3.6.2 or later in the 3.x line is supported for sources it can
  open;
- a native MuseScore 4 source requires MuseScore 4.

Use the renderer path shown in
[MuseScore renderer](musescore-renderer.md#installation), or select it in
Verse Settings.

## Bootstrap

From the repository root:

```sh
npm ci
npm run version:check
```

`npm ci` is required for reproducible dependency installation. Do not replace
the committed npm or Cargo lock files with unconstrained resolution.

## Run the application

Run the functional desktop application:

```sh
npm run tauri dev
```

Tauri starts the Vite server at `http://localhost:1420`, launches the native
window, and enables dialogs, drag/drop, IPC, filesystem operations, and Rust
commands.

For visual frontend work only:

```sh
npm run dev
```

This starts Vite without the Tauri host. The page can render in a browser, but
native dialogs, webview drag/drop events, and Tauri command invocations are
not functional there.

## Required quality gates

Run these before committing implementation changes:

```sh
npm run version:check
npm test
npm run build

cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy \
  --manifest-path src-tauri/Cargo.toml \
  --all-targets \
  --locked \
  -- \
  -D warnings
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --all-targets
```

`npm run build` runs strict TypeScript compilation before Vite bundling. The
frontend suite currently contains utility, version, and atomic override tests;
there is no configured ESLint, Prettier, React component, or desktop E2E gate.

GitHub CI runs the same frontend gates, Rust formatting, strict Clippy over all
targets, and locked Rust tests on Ubuntu.

## Optional supplied-score gates

The source-fidelity integration test accepts private real-world fixtures
without adding them to Git:

```sh
VERSE_MSCZ_GATE="/absolute/path/to/score.mscz" \
VERSE_MXL_GATE="/absolute/path/to/score.mxl" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test source_fidelity
```

These variables are optional during normal test runs. When configured, the
gates assert exact source-note, lyric, percussion, and no-invention behavior.

## Private corpus gates

Private or copyrighted song fixtures must remain outside the repository.
Point `VERSE_CORPUS_DIR` to the audited local corpus and explicitly request
ignored tests:

```sh
VERSE_CORPUS_DIR="/absolute/path/to/private-corpus" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --test corpus \
  -- \
  --ignored
```

The corpus gate requires all named fixtures. Missing files are failures, not
skips. It currently covers seven exact KAR lyric projections and four
MusicXML/MuseScore topology expectations. Never copy these fixtures into
`src-tauri/tests/fixtures/`, a commit, a build artifact, or a public report.

See [Corpus testing and licensing](test-corpora.md).

## Real MuseScore gates

All renderer paths and fixtures must be absolute.

Probe Part extraction and one WAV render:

```sh
VERSE_MUSESCORE_GATE="/absolute/path/to/mscore" \
VERSE_SCORE_PARTS_GATE="/absolute/path/to/score.mscz" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  configured_real_musescore_extracts_bounded_parts_and_renders_audio \
  -- \
  --nocapture
```

Create and validate a complete real-render bundle:

```sh
VERSE_MUSESCORE_GATE="/absolute/path/to/mscore" \
VERSE_BUNDLE_GATE="/absolute/path/to/score.mscz" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  configured_real_renderer_exports_one_verified_stem_per_source_part \
  -- \
  --nocapture
```

Audit source topology against extracted MuseScore Parts. The gate path may be
one file or a directory of supported sources:

```sh
VERSE_MUSESCORE_GATE="/absolute/path/to/mscore" \
VERSE_PART_MAPPING_GATE="/absolute/path/to/fixture-or-directory" \
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  configured_real_renderer_part_count_matches_source_topology \
  -- \
  --nocapture
```

Real rendering is deliberately sequential and can take several minutes.
MuseScore 4 on macOS additionally observes the documented cooldown and bounded
retry policy.

## OpenScore Lieder audit

The public runner pins the OpenScore Lieder repository and commit, rejects a
dirty or foreign cache, parses every canonical `.mscx`, and can render a
deterministic sample:

```sh
VERSE_MUSESCORE_GATE="/absolute/path/to/mscore" \
scripts/run-openscore-corpus.sh \
  --full-parse \
  --render-sample 3
```

Useful bounded overrides:

```sh
scripts/run-openscore-corpus.sh \
  --full-parse \
  --render-sample 5 \
  --renderer "/absolute/path/to/mscore" \
  --cache "/absolute/path/to/openscore-cache" \
  --report "/absolute/path/to/report.json" \
  --max-files 2000
```

The default cache and report live under ignored `src-tauri/target/`
directories. The first run requires network access to fetch the pinned
repository; the installed application itself remains offline.

## Build local packages

Build for the current host:

```sh
npm run tauri -- build
```

Outputs are written below `src-tauri/target/<target>/release/bundle/`.
Cross-platform release builds require the operating-system runners and Rust
targets defined in `.github/workflows/build.yml`; a local host is not expected
to reproduce all six release targets.

The frontend-only production output is:

```sh
npm run build
```

and is written to ignored `dist/`.

## Source organization and change rules

- React presentation and transient state belong in `src/`.
- Rust owns parsing, musical meaning, projection, renderer execution, and
  durable artifacts.
- Format-specific syntax belongs in its parser; cross-format policy belongs
  in the shared model or projection layer.
- `src/lib/tauri.ts` and Rust camelCase DTOs in `src-tauri/src/lib.rs` must
  remain synchronized.
- Stable structured error codes may be consumed by the UI; English message
  text must not become control flow.
- No missing lyric, pitch, track, instrument, or audio fallback may be added.
- Output writes must preserve no-replace and transactional ownership rules.
- A persisted bundle/SVP/ledger change requires corresponding documentation
  and compatibility tests.

See [Component inventory](component-inventory.md),
[Architecture](architecture.md), and
[Tauri command contracts](tauri-command-contracts.md).

## Test fixture and generated-file policy

Do not commit:

- `node_modules/`, `dist/`, or `src-tauri/target/`;
- generated Tauri schemas under `src-tauri/gen/`;
- private song fixtures under `src-tauri/tests/fixtures/`;
- OpenScore checkouts or generated corpus reports;
- local BMAD/agent working data under `_bmad/`, `_bmad-output/`, or `.agents/`.

Public tests should use synthetic bytes or sources with verified licenses.
Every external corpus must pin its identity and retain license evidence.

## Version and release metadata

Before a release, `npm run version:check` verifies one strict SemVer across:

- `package.json`;
- root and workspace entries in `package-lock.json`;
- `src-tauri/Cargo.toml`;
- the `verse` package in `src-tauri/Cargo.lock`;
- `src-tauri/tauri.conf.json`;
- `.release-please-manifest.json`;
- the latest dated `CHANGELOG.md` release;
- an optional `vMAJOR.MINOR.PATCH` tag.

Release Please and the six-target packaging workflow are documented in
`README.md` and `.github/workflows/`. Released packages are currently unsigned.

## Troubleshooting development failures

- **Vite rejects Node:** install Node 20.19+ or 22.12+; older Node 22 releases
  do not satisfy Vite 7.
- **Linux Tauri link failure:** compare installed system packages with the CI
  dependency list above.
- **Tauri calls fail in a browser:** run `npm run tauri dev`, not only
  `npm run dev`.
- **Complete bundle export reports a missing renderer:** install or select one
  supported MuseScore executable.
- **Tests try to use private files:** unset the optional `VERSE_*_GATE`
  variables or provide the complete audited fixture set.
- **An output already exists:** choose a new destination; Verse never
  overwrites it.
