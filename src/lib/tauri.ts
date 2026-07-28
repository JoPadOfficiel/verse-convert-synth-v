import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  SUPPORTED_EXTENSIONS,
  defaultBundlePath,
  defaultVocalPath,
  type ExportTarget,
  type StructuredCommandError,
} from "@/lib/file-utils";

export {
  SUPPORTED_EXTENSIONS,
  batchBundlePaths,
  commandError,
  commandErrorMessage,
  defaultBundlePath,
  defaultVocalPath,
  isAudioUnavailableErrorCode,
  isSupported,
  uniqueSupportedPaths,
} from "@/lib/file-utils";

export type SourceRole =
  | "vocal"
  | "instrumental"
  | "percussion"
  | "mixed"
  | "lyricsOnly"
  | "metadata"
  | "ambiguous";

export type LyricStatus = {
  state:
    | "sourceOwned"
    | "explicitEmpty"
    | "metadataOnly"
    | "none"
    | "ambiguous"
    | "unsupported";
  sourceTextCount: number;
  projectedTextCount: number;
  explicitEmptyCount: number;
  continuationCount: number;
  unsupportedCount: number;
};

export type ExportRepresentation =
  | "vocalNotes"
  | "referenceMixMember"
  | "vocalNotesAndReferenceMix"
  | "sourceOnly";

export type Diagnostic = {
  code: string;
  severity: "info" | "warning";
  message: string;
  sourceId: string | null;
};

export type AudioStatus =
  | { state: "notRendered" }
  | {
      state: "available";
      path: string;
      durationSeconds: number;
      sampleRate: number;
      channels: number;
      fullScoreMix: true;
    }
  | { state: "unavailable"; code: string; message: string };

export type CommandError = StructuredCommandError;

export type TrackInfo = {
  id: number;
  sourceId: string;
  track: string;
  notes: number;
  /** Compatibility value. Prefer sourceRole/exportRepresentation. */
  role: string;
  placed: number;
  sourceRole: SourceRole;
  lyricStatus: LyricStatus;
  exportRepresentation: ExportRepresentation;
  requiresVoiceAssignment: boolean;
  warnings: Diagnostic[];
};

export type PartInfo = {
  sourceId: string;
  part: string;
  staves: number;
  voices: number;
  trackIds: number[];
  vocalCandidateTrackIds: number[];
  sourceTrackIds: string[];
  notes: number;
  placed: number;
  sourceRole: SourceRole;
  lyricStatus: LyricStatus;
  exportRepresentation: ExportRepresentation;
  requiresVoiceAssignment: boolean;
  hasAudioStem: boolean;
  warnings: Diagnostic[];
};

export type FileResult = {
  path: string;
  name: string;
  ok: boolean;
  error: CommandError | null;
  /** Compatibility value supplied by the backend. */
  msg: string | null;
  nParts: number;
  nVoices: number;
  nTracks: number;
  placed: number;
  parts: PartInfo[];
  tracks: TrackInfo[];
  audioStatus: AudioStatus;
  requiresVoiceAssignment: boolean;
  /** A complete bundle always writes a Synthesizer V project, so this stays true
   *  for a source only the OpenUtau target refuses. */
  bundleReady: boolean;
  warnings: Diagnostic[];
  out: string | null;
};

export type Overrides = Record<string, Record<number, boolean>>;
export type Language = "english" | "french";
/**
 * Which format an export writes. Declared with the path helpers that consume it
 * and surfaced here, beside `Language`, because this is the module the app
 * imports its command types from.
 */
export type { ExportTarget };

export type RendererStatus = {
  state: "available" | "missing" | "unsupported";
  configured: boolean;
  provider: string | null;
  version: string | null;
  fullScoreMix: boolean;
  message: string | null;
};

export type BundleResult = {
  bundlePath: string;
  projectPath: string;
  audioPath: string;
  audioPaths: string[];
  stemCount: number;
  sourcePath: string;
  manifestPath: string;
  renderer: {
    provider: string;
    version: string;
    major: number;
    executableSha256: string;
    fullScoreMix: true;
    capabilities: string[];
  };
  audioDurationSeconds: number;
  audioSampleRate: number;
  audioChannels: number;
  warnings: string[];
};

export type BundleProgressPhase =
  | "preparing"
  | "extractingParts"
  | "renderingReference"
  | "renderingStem"
  | "finalizing"
  | "finished";

export type BundleProgressEvent = {
  phase: BundleProgressPhase;
  completed: number;
  total: number;
  message: string;
  stemId: string | null;
  stemName: string | null;
};

export async function pickFiles(): Promise<string[]> {
  const result = await open({
    multiple: true,
    filters: [
      {
        name: "Karaoke / MIDI / Score",
        extensions: [...SUPPORTED_EXTENSIONS],
      },
    ],
  });
  if (!result) return [];
  return Array.isArray(result) ? result : [result];
}

export async function pickDirectory(): Promise<string | undefined> {
  const result = await open({ directory: true, multiple: false });
  return typeof result === "string" ? result : undefined;
}

export async function pickRenderer(): Promise<string | undefined> {
  const result = await open({
    directory: false,
    multiple: false,
    title: "Choose a MuseScore Studio 3.6.2 or 4 executable",
  });
  return typeof result === "string" ? result : undefined;
}

export async function chooseBundleTarget(
  sourcePath: string,
): Promise<string | undefined> {
  const target = await save({
    defaultPath: defaultBundlePath(sourcePath),
    filters: [
      { name: "Verse preservation bundle", extensions: ["versebundle"] },
    ],
  });
  return target || undefined;
}

/** The save-dialog filter per target, so the dialog offers the target's own name. */
const VOCAL_TARGET_FILTER: Record<
  ExportTarget,
  { name: string; extensions: string[] }
> = {
  svp: { name: "Synthesizer V vocal project", extensions: ["svp"] },
  ustx: { name: "OpenUtau project", extensions: ["ustx"] },
};

export async function exportVocalsWithDialog(
  file: FileResult,
  language: Language,
  overrides?: Record<number, boolean>,
  exportTarget: ExportTarget = "svp",
): Promise<string | undefined> {
  const target = await save({
    defaultPath: defaultVocalPath(file.path, exportTarget),
    filters: [VOCAL_TARGET_FILTER[exportTarget]],
  });
  if (!target) return undefined;
  // `target` is the output path and has been since 0.1.0; `exportTarget` is the
  // format. The backend defaults it to `svp`, so the two stay independent.
  return await invoke<string>("export_svp", {
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
    exportTarget,
  });
}

export async function exportBundle(
  file: FileResult,
  target: string,
  language: Language,
  overrides?: Record<number, boolean>,
  rendererPath?: string,
  onProgress?: (event: BundleProgressEvent) => void,
): Promise<BundleResult> {
  const progress = new Channel<BundleProgressEvent>();
  progress.onmessage = (event) => onProgress?.(event);
  return await invoke<BundleResult>("export_bundle", {
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
    rendererPath: rendererPath?.trim() || null,
    onProgress: progress,
  });
}

export async function getRendererStatus(
  rendererPath?: string,
): Promise<RendererStatus> {
  return await invoke<RendererStatus>("renderer_status", {
    rendererPath: rendererPath?.trim() || null,
  });
}

/**
 * Analyses (`write = false`) or batch-exports (`write = true`) every path.
 *
 * `exportTarget` reaches analysis and not only the writer because the timing a
 * target accepts is part of the convertibility verdict: OpenUtau's 480 ticks per
 * quarter represent a strict subset of what Synthesizer V blicks do, so a source
 * that analyses cleanly for one target can be refused by the other.
 */
export async function convertFiles(
  paths: string[],
  write: boolean,
  language: Language = "english",
  outDir?: string,
  overrides?: Overrides,
  exportTarget: ExportTarget = "svp",
): Promise<FileResult[]> {
  return await invoke<FileResult[]>("convert_files", {
    paths,
    write,
    outDir: outDir ?? null,
    language,
    overrides: overrides ?? null,
    exportTarget,
  });
}
