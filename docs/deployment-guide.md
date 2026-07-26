# Deployment and Release Guide

Verse is an offline Tauri desktop application. GitHub Actions validates the
source, builds installers, assembles checksums, and manages GitHub Releases;
there is no runtime server or cloud deployment.

## Platform status

The reusable release build produces artifacts for six Rust targets:

| Operating system | Architecture | Rust target | Support status |
| --- | --- | --- | --- |
| macOS | Apple Silicon | `aarch64-apple-darwin` | Documented user platform |
| macOS | Intel | `x86_64-apple-darwin` | Documented user platform |
| Windows | x64 | `x86_64-pc-windows-msvc` | Documented user platform |
| Windows | ARM64 | `aarch64-pc-windows-msvc` | Documented user platform |
| Linux | x64 | `x86_64-unknown-linux-gnu` | Build-qualified only |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` | Build-qualified only |

Linux packages are produced to exercise the build and make qualification
possible, but Linux is not currently an officially supported user platform.
Install smoke tests, end-user documentation, and an explicit support policy
are still required before that status changes.

Release packages are currently unsigned. They are not notarized on macOS and
do not carry a Windows code-signing certificate. The user-facing installation
instructions must continue to explain the operating-system warnings until a
separate signing and notarization process is approved and implemented.

## Continuous integration

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on pull requests
to `main`, pushes to `main`, and manual dispatches. Its Ubuntu 22.04 quality job
uses Node.js 22 and Rust 1.93.0, installs the Tauri Linux prerequisites, and
runs:

```sh
npm ci
node scripts/check-version.mjs
npm run test:frontend
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

GitHub Actions are pinned to immutable action commit SHAs. Dependency
installation uses `npm ci` and Cargo's committed lockfile; release work must
not replace these with floating installation commands.

## Version and changelog authority

Release Please owns routine version and changelog preparation. Conventional
Commits merged into `main` drive the release pull request. The configuration
in [`release-please-config.json`](../release-please-config.json) uses Semantic
Versioning, `vMAJOR.MINOR.PATCH` tags, dated entries in
[`CHANGELOG.md`](../CHANGELOG.md), and draft GitHub Releases.

One release version must be synchronized across:

- `package.json`;
- the root package and `packages[""]` entries in `package-lock.json`;
- `src-tauri/Cargo.toml`;
- the `verse` package entry in `src-tauri/Cargo.lock`;
- `src-tauri/tauri.conf.json`;
- `.release-please-manifest.json`;
- the latest dated release heading in `CHANGELOG.md`.

Run the following before approving a release pull request:

```sh
npm run version:check
```

For a specific release tag, use:

```sh
node scripts/check-version.mjs --tag vMAJOR.MINOR.PATCH
```

The checker requires strict SemVer, an exact `v`-prefixed tag, and a valid ISO
date in the latest changelog heading.

### Why `Cargo.lock` is synchronized by a script

Release Please updates every file in that list except `src-tauri/Cargo.lock`.
That file is an array of `[[package]]` tables, and selecting one of them needs a
jsonpath filter its TOML updater does not support, so an `extra-files` rule for
it silently does nothing — it left two releases with a lock still on the
previous version. Because `build.yml` runs `check-version.mjs` before it
compiles anything, such a release fails the gate and publishes with no binaries
at all.

[`scripts/sync-cargo-lock.mjs`](../scripts/sync-cargo-lock.mjs) performs that
one edit instead: it copies the version from `src-tauri/Cargo.toml` onto the
locked `verse` package, changes nothing else, and is a no-op once they agree.
`release-please.yml` runs it on the release branch while that branch is still a
pull request, verifies the result with `check-version.mjs`, and commits only
when the lock actually moved. Running it locally is safe at any time.

## Normal Release Please flow

The normal release path is:

1. Merge conventional feature and fix commits into `main`.
2. Let [`.github/workflows/release-please.yml`](../.github/workflows/release-please.yml)
   create or update the release pull request.
3. Review the proposed version, changelog, package metadata, and CI result.
4. Merge the release pull request.
5. Release Please creates the `vMAJOR.MINOR.PATCH` tag and draft release for
   the exact release commit.
6. The workflow invokes the reusable six-target build with the tag, the full
   40-character commit SHA, and `publish_release: true`.
7. The reusable workflow validates, builds, uploads all installers, assembles
   `SHA256SUMS`, updates the draft, and publishes it only after every check
   succeeds.

Do not move, delete, or recreate a published release tag. If a released build
is wrong, correct the source and publish a new version.

## Reusable release build

[`.github/workflows/build.yml`](../.github/workflows/build.yml) is the single
release build implementation. It may be called by another workflow or started
manually with:

- `tag`: a strict `vMAJOR.MINOR.PATCH` value;
- `commit_sha`: the full lowercase 40-character commit SHA;
- `publish_release`: whether the verified draft may become public.

A manual dispatch defaults `publish_release` to `false`. This permits an
operator to build or update a draft before a tag exists. Publication is
stricter: the tag must already exist on `origin` and resolve to the requested
commit.

### Identity validation

Before any platform build starts, the workflow:

- checks out the requested SHA directly;
- confirms `HEAD` equals that SHA;
- validates the tag syntax and full SHA syntax;
- checks any existing remote tag against the requested SHA;
- refuses publication when the remote tag does not exist;
- runs the full frontend and Rust quality gates;
- runs the version checker against the requested tag.

Each platform job checks out the same SHA and rechecks the version/tag
relationship before running the Tauri build. A branch name is never accepted
as release identity.

### Artifact assembly

Every target uploads its Tauri `release/bundle` directory as a temporary
workflow artifact. The publish job selects the latest run attempt for each of
the six expected targets and accepts releasable files with these extensions:

```text
.AppImage .deb .dmg .exe .msi .rpm .tar.gz .zip
```

Associated `.sig` files are retained when a build produces them, but their
presence does not imply that the current release is signed.

Published asset names are prefixed with the Rust target:

```text
verse-platform-<rust-target>-<installer-name>
```

The assembly fails on a missing target, missing installer, unexpected target,
or filename collision. It generates `SHA256SUMS` from the assembled files and
immediately verifies the checksum file before uploading anything.

### Draft update and publication

The publish job creates or updates the GitHub draft idempotently:

- the release target is set to the validated commit;
- existing draft assets are enumerated and replaced by the newly assembled
  assets;
- every installer and `SHA256SUMS` is uploaded;
- the remote tag is verified immediately before publication;
- the remote tag is verified again after publication.

If the release is already public, the workflow does not overwrite it. It
downloads the published assets, verifies `SHA256SUMS`, checks that every asset
belongs to an expected target, and requires at least one installer for each of
the six targets.

GitHub tag protection or immutable-release settings remain an administrator
responsibility. A workflow can detect tag movement; it cannot by itself make a
Git ref immutable.

## Tag-triggered flow and duplicate triggers

[`.github/workflows/release-tag.yml`](../.github/workflows/release-tag.yml)
runs for every pushed `v*.*.*` tag. It resolves annotated and lightweight tags
to a full commit SHA, then calls the same reusable build with publication
enabled.

Merging a Release Please release pull request can therefore cause two valid
triggers:

- the Release Please workflow calls the reusable build after reporting that a
  release was created;
- the new tag independently starts the tag workflow.

The reusable workflow uses a per-tag concurrency group with
`cancel-in-progress: false`, so these runs are serialized rather than
cancelled. Its draft and published-release operations are idempotent, and an
already published release is verified instead of replaced. Both runs may
still consume build time, so release operators should expect and monitor the
pair rather than interpreting the second run as a second release.

## Manual release checklist

Before starting or approving publication:

1. Confirm the intended release commit is on `main`.
2. Record its full SHA with `git rev-parse HEAD`.
3. Confirm every version source and the dated changelog entry with
   `npm run version:check`.
4. Run the complete local quality gates documented in
   [the contribution guide](contribution-guide.md).
5. Confirm risk-specific private and public corpus gates passed when the
   release changes parsing, lyrics, topology, rendering, or bundle semantics.
6. Confirm the strict tag is exactly `v<project-version>`.
7. Verify the remote tag resolves to the recorded SHA.
8. Confirm all six target jobs succeeded.
9. Download `SHA256SUMS` and verify the release assets before announcing the
   release.
10. State clearly that packages are unsigned and that Linux remains
    build-qualified rather than officially supported.

## Recovery from a failed release run

A validation or platform-build failure leaves no valid public release from
that run. Fix the source or workflow, keep the release draft unpublished, and
rerun against the same commit only when the commit itself remains the intended
release identity.

If the source must change, create a new release commit and version. Never move
an existing public tag to the replacement commit. Temporary GitHub Actions
artifacts expire after 14 days for platform bundles and 30 days for assembled
release assets; the GitHub Release is the durable public delivery location.
