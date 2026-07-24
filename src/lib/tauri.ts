import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  SUPPORTED_EXTENSIONS,
  defaultBundlePath,
  defaultSvpPath,
  type StructuredCommandError,
} from "@/lib/file-utils";

export {
  SUPPORTED_EXTENSIONS,
  commandError,
  commandErrorMessage,
  defaultBundlePath,
  defaultSvpPath,
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

export type FileResult = {
  path: string;
  name: string;
  ok: boolean;
  error: CommandError | null;
  /** Compatibility value supplied by the backend. */
  msg: string | null;
  nTracks: number;
  placed: number;
  tracks: TrackInfo[];
  audioStatus: AudioStatus;
  requiresVoiceAssignment: boolean;
  warnings: Diagnostic[];
  out: string | null;
};

export type Overrides = Record<string, Record<number, boolean>>;
export type Language = "english" | "french";

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
  sourcePath: string;
  manifestPath: string;
  renderer: {
    provider: string;
    version: string;
    executableSha256: string;
    fullScoreMix: true;
  };
  audioDurationSeconds: number;
  audioSampleRate: number;
  audioChannels: number;
  warnings: string[];
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
    title: "Choose the MuseScore Studio 4 executable",
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

export async function exportVocalsWithDialog(
  file: FileResult,
  language: Language,
  overrides?: Record<number, boolean>,
): Promise<string | undefined> {
  const target = await save({
    defaultPath: defaultSvpPath(file.path),
    filters: [{ name: "Synthesizer V vocal project", extensions: ["svp"] }],
  });
  if (!target) return undefined;
  return await invoke<string>("export_svp", {
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
  });
}

export async function exportBundle(
  file: FileResult,
  target: string,
  language: Language,
  overrides?: Record<number, boolean>,
  rendererPath?: string,
): Promise<BundleResult> {
  return await invoke<BundleResult>("export_bundle", {
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
    rendererPath: rendererPath?.trim() || null,
  });
}

export async function getRendererStatus(
  rendererPath?: string,
): Promise<RendererStatus> {
  return await invoke<RendererStatus>("renderer_status", {
    rendererPath: rendererPath?.trim() || null,
  });
}

export async function convertFiles(
  paths: string[],
  write: boolean,
  language: Language = "english",
  outDir?: string,
  overrides?: Overrides,
): Promise<FileResult[]> {
  return await invoke<FileResult[]>("convert_files", {
    paths,
    write,
    outDir: outDir ?? null,
    language,
    overrides: overrides ?? null,
  });
}
