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
  SCORE_EXTENSIONS,
  MIDI_EXTENSIONS,
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
  pronunciationSnapshotId?: string;
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
  /** Whether a complete bundle can be written for this source. The bundle carries
   *  the selected export target's own project, so this follows that target. */
  bundleReady: boolean;
  warnings: Diagnostic[];
  out: string | null;
};

export type Overrides = Record<string, Record<number, boolean>>;
export type PronunciationProfile =
  | "default"
  | "frenchMillefeuille"
  | "englishArpabet"
  | "spanishDiffSinger"
  | "portugueseDiffSinger"
  | "automatic";
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
  pronunciationProfile: PronunciationProfile = "automatic",
): Promise<string | undefined> {
  const target = await save({
    defaultPath: defaultVocalPath(file.path, exportTarget),
    filters: [VOCAL_TARGET_FILTER[exportTarget]],
  });
  if (!target) return undefined;
  // `target` is the output path and has been since 0.1.0; `exportTarget` is the
  // format. The backend defaults it to `svp`, so the two stay independent.
  return await invoke<string>("pronunciation_export_svp", {
    snapshotId: file.pronunciationSnapshotId ?? analysisSnapshots.get(file.path) ?? null,
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
    exportTarget,
    pronunciationProfile,
  });
}

/**
 * Writes one complete preservation bundle.
 *
 * `target` is the bundle's destination path and has been since the bundle
 * shipped; `exportTarget` is the format of the project inside it. The stems are
 * the same WAVs at the same relative paths either way — only the project file that
 * references them differs, so the bundle's own filename does not follow the
 * format.
 */
export async function exportBundle(
  file: FileResult,
  target: string,
  language: Language,
  overrides?: Record<number, boolean>,
  rendererPath?: string,
  onProgress?: (event: BundleProgressEvent) => void,
  exportTarget: ExportTarget = "svp",
  pronunciationProfile: PronunciationProfile = "automatic",
): Promise<BundleResult> {
  const progress = new Channel<BundleProgressEvent>();
  progress.onmessage = (event) => onProgress?.(event);
  return await invoke<BundleResult>("pronunciation_export_bundle", {
    snapshotId: file.pronunciationSnapshotId ?? analysisSnapshots.get(file.path) ?? null,
    path: file.path,
    target,
    language,
    overrides: overrides ?? null,
    rendererPath: rendererPath?.trim() || null,
    exportTarget,
    pronunciationProfile,
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
const analysisSnapshots = new Map<string, string>();
export async function releaseAnalysisSnapshots(paths: string[]): Promise<void> {
  const snapshotIds = paths.map((path) => analysisSnapshots.get(path)).filter((id): id is string => Boolean(id));
  if (snapshotIds.length) await invoke<void>("pronunciation_release_snapshots", { snapshotIds });
  for (const path of paths) analysisSnapshots.delete(path);
}

export async function convertFiles(
  paths: string[],
  write: boolean,
  language: Language = "english",
  outDir?: string,
  overrides?: Overrides,
  exportTarget: ExportTarget = "svp",
  pronunciationProfile: PronunciationProfile = "automatic",
): Promise<FileResult[]> {
  if (!write) await releaseAnalysisSnapshots(paths);
  const results = await invoke<FileResult[]>("pronunciation_convert_files", {
    snapshotIds: write ? Object.fromEntries(paths.map((path) => [path, analysisSnapshots.get(path) ?? ""])) : null,
    paths,
    write,
    outDir: outDir ?? null,
    language,
    overrides: overrides ?? null,
    exportTarget,
    pronunciationProfile,
  });
  for (const result of results) {
    if (result.ok && result.pronunciationSnapshotId) analysisSnapshots.set(result.path, result.pronunciationSnapshotId);
    else analysisSnapshots.delete(result.path);
  }
  return results;
}

export type CorrectionReading = { aliases?: { member: number; index: number; phone: string }[] | null; authority?: string | null; language: "fr" | "en" | "es" | "pt" | null; phonemizer: string; lexical_reading: string | null; phones: string[] | null; alphabet: string | null };
export type ImportedCorrection = {
  accepted_variant?: string | null;
  id: string; fingerprint: string;
  word: { id: string; key: string; original: string[]; members: string[]; context: string[]; context_target: number; attacks: number; manual: boolean;
    owner: { track: string; part: string | null; staff: string | null; voice: string | null; occurrence: number; segment: number | null; lane: string; verse: number } };
  before: CorrectionReading; after: CorrectionReading;
  scope: "occurrence_only" | "compatible_context";
  voice: { singer: string; inventory_sha256: string; configuration_sha256: string } | null;
  provenance: { source_sha256: string; export_sha256: string; corrected_sha256: string; confirmed_after_listening: boolean; symbol_validation: string; policy: string;
    /** Native identity observed in the corrected copy; does not qualify voice reuse. */
    observed_singer?: string | null;
  };
};
export type CorrectionsReview = { reference_id: string; corrected_sha256: string; proposals: ImportedCorrection[]; diagnostics: { code: string; message: string }[] };
export type PronunciationMemory = { history: { correction: ImportedCorrection; status: string }[]; references: { id: string; exportSha256: string; sourceLabel: string | null; exportLabel: string | null; createdAtUnixSeconds: number | null }[]; baseline: string; layaReason: string };
export const getPronunciationMemory = () => invoke<PronunciationMemory>("pronunciation_memory");
export const comparePronunciation = (referenceId: string, correctedPath: string) => invoke<CorrectionsReview>("pronunciation_compare", { referenceId, correctedPath });
export const confirmPronunciation = (referenceId: string, correctedPath: string, selections: { correction_id: string; scope: "occurrence_only" | "compatible_context"; listened: boolean }[]) => invoke<void>("pronunciation_confirm", { referenceId, correctedPath, selections });
export const setCorrectionStatus = (id: string, status: "active" | "revoked", confirmed = false) => invoke<void>("pronunciation_set_status", { id, status, confirmed });
export const exchangePronunciation = (path: string, importing: boolean) => invoke<void>("pronunciation_exchange", { path, import: importing });
export const pickCorrectedProject = async () => { const path = await open({ multiple: false, filters: [{ name: "Corrected OpenUtau project", extensions: ["ustx"] }] }); return typeof path === "string" ? path : undefined; };
export const pickCorrectionExchange = async (importing: boolean) => {
  const options = { filters: [{ name: "Pronunciation corrections JSON", extensions: ["json"] }] };
  const path = importing ? await open({ ...options, multiple: false }) : await save({ ...options, defaultPath: "pronunciation-corrections.json" });
  return typeof path === "string" ? path : undefined;
};
