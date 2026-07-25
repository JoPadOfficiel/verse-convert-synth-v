# Tauri Command Contracts

## Boundary

`src/lib/tauri.ts` is the frontend adapter. `src-tauri/src/lib.rs` is the Rust
command adapter and current orchestration seam. The webview exchanges compact
JSON-compatible DTOs; source bytes, parsed musical IR, and audio never cross
IPC.

Rust DTOs use camelCase serialization and are mirrored explicitly in
TypeScript. Any shape change must update both sides and tests together.

## Commands

### `convert_files`

```text
request:
  paths: string[]
  write: boolean
  outDir?: string
  language?: "english" | "french"
  overrides?: Record<sourcePath, Record<trackIdString, boolean>>

response:
  FileResult[]
```

The current UI always passes `write=false`; direct output is handled by the
dedicated export commands. Analysis is synchronous in Rust.

### `export_svp`

```text
request:
  path: string
  target: string
  language?: "english" | "french"
  overrides?: Record<trackIdString, boolean>

response:
  string  // committed target path
```

The target must be new and must not be the source.

### `export_bundle`

```text
request:
  path: string
  target: string
  language?: "english" | "french"
  overrides?: Record<trackIdString, boolean>
  rendererPath?: string

response:
  BundleResult
```

The command uses Tauri's blocking worker pool for parsing, rendering, and
publication. It returns only after the bundle is committed and verified.

### `renderer_status`

```text
request:
  rendererPath?: string

response:
  RendererStatusDto
```

The probe runs on the blocking worker pool.

## `FileResult`

Important fields:

| Field | Meaning |
|---|---|
| `path`, `name` | Display/source identity for the current process |
| `ok` | Analysis/projection succeeded |
| `error` | Structured error when `ok=false` |
| `msg` | Historical compatibility summary |
| `nParts` | Source topology Part count |
| `nVoices` | Source topology voice count |
| `nTracks` | Number of detailed projection/source report lanes |
| `placed` | Count of projected lyric occurrences |
| `parts` | Part-level UI summaries |
| `tracks` | Detailed source/projection lane reports |
| `audioStatus` | Initially `notRendered`; UI enriches after bundle export |
| `requiresVoiceAssignment` | At least one vocal track needs a Synthesizer V voice |
| `warnings` | Aggregated diagnostics |
| `out` | Last successful output path in the UI session |

### `PartInfo`

`PartInfo` aggregates all source lanes owned by one source Part:

- stable Part/source IDs;
- staff/voice counts;
- detailed track IDs;
- vocal-candidate lane IDs;
- source track IDs;
- source note/projected lyric counts;
- source role, lyric status, export representation;
- expected stem presence;
- diagnostics.

The Part toggle sends every `vocalCandidateTrackId` atomically.

### `TrackInfo`

`sourceRole`, `lyricStatus`, and `exportRepresentation` are authoritative.
The `role` string is retained only for compatibility.

Source roles:

```text
vocal | instrumental | percussion | mixed |
lyricsOnly | metadata | ambiguous
```

Export representations:

```text
vocalNotes | referenceMixMember |
vocalNotesAndReferenceMix | sourceOnly
```

## `BundleResult`

The response includes:

- bundle/project/source/manifest/full-score paths;
- all audio paths and stem count;
- renderer identity and capabilities;
- full-score duration, sample rate, and channel count;
- persisted warnings.

The bundle itself, not this response, is the durable authority.

## Structured errors

```ts
type CommandError = {
  code: string;
  message: string;
  remediation?: string | null;
};
```

Primary codes:

| Area | Codes |
|---|---|
| Source | `UNSUPPORTED_FILE`, `SOURCE_TOO_LARGE`, `SOURCE_READ_FAILED`, `SOURCE_PARSE_FAILED` |
| Projection | `CONVERSION_FAILED`, `SERIALIZE_FAILED` |
| Direct output | `INVALID_OUTPUT`, `WRITE_FAILED` |
| Renderer | `RENDERER_NOT_FOUND`, `RENDERER_UNSUPPORTED`, `RENDERER_TIMEOUT`, `RENDERER_FAILED` |
| Stem/ledger | `STEM_PLAN_INVALID`, `PRESERVATION_INCOMPLETE` |
| Bundle | `INVALID_DESTINATION`, `DESTINATION_EXISTS`, `BUNDLE_IO_FAILED`, `BUNDLE_SERIALIZE_FAILED`, `BUNDLE_INTEGRITY_FAILED`, `BUNDLE_COMMIT_FAILED`, `BUNDLE_TASK_FAILED` |

Frontend logic may branch on stable codes, never on English message text.

## Current authority model

The current commands accept user-selected path strings and re-read/reparse the
source during export. The renderer accepts a selected executable path, but
never arbitrary arguments.

The BMAD target architecture proposes backend-issued source/destination
handles, immutable conversion plans, semantic hashes, cancellable jobs, and
typed progress Channels. Those types and commands do **not** exist yet and
must not be treated as public API.

## Frontend workflow behavior

- `busyRef` plus `busy` permits only one UI operation at a time.
- Batch complete-project exports run sequentially.
- Changing target language reanalyzes all files.
- A failed override reanalysis restores the previous frontend override map.
- Renderer path and theme are stored in localStorage; output folder remains
  session-only.
- Tauri dialogs supply source, destination, directory, and renderer choices.

## Change checklist

A command/DTO change requires:

1. Rust DTO/command update;
2. matching TypeScript type and invocation;
3. serialization or frontend helper regression;
4. updated structured error mapping;
5. architecture/contract documentation update;
6. all frontend and Rust gates.
