export const SUPPORTED_EXTENSIONS = [
  "kar",
  "mid",
  "midi",
  "mxl",
  "xml",
  "musicxml",
  "mscz",
  "mscx",
] as const;

// A notated score states what a MIDI file can only imply: which note owns a
// syllable, which verse it belongs to, where a word is held, and what the part
// is. Verse never guesses the difference, so the same song converts more
// completely from a score. Ordered best first; the two lists together must
// cover SUPPORTED_EXTENSIONS.
export const SCORE_EXTENSIONS = [
  "mxl",
  "musicxml",
  "xml",
  "mscz",
  "mscx",
] as const;

export const MIDI_EXTENSIONS = ["kar", "mid", "midi"] as const;

export type StructuredCommandError = {
  code: string;
  message: string;
  remediation?: string | null;
};

/**
 * Mirrors `engine::target::ExportTarget`. The values are the Rust serde values,
 * so they travel to the backend as they are written here. Declared beside the
 * path helpers because those are what read it, and because they are tested
 * without a webview.
 */
export type ExportTarget = "svp" | "ustx";

/**
 * The output extension per target, spelled out rather than derived from the
 * target name: the two happen to be the same string today, and a target whose
 * name is not its extension must not silently inherit the wrong one.
 */
const TARGET_EXTENSION: Record<ExportTarget, string> = {
  svp: "svp",
  ustx: "ustx",
};

const supportedPattern = new RegExp(
  `\\.(${SUPPORTED_EXTENSIONS.join("|")})$`,
  "i",
);

export const isSupported = (path: string) => supportedPattern.test(path);

export function uniqueSupportedPaths(paths: string[]): string[] {
  return [...new Set(paths.filter(isSupported))];
}

const separator = (path: string) => (path.includes("\\") ? "\\" : "/");

function splitSourcePath(sourcePath: string) {
  const sep = separator(sourcePath);
  const index = sourcePath.lastIndexOf(sep);
  const directory = sourcePath.slice(0, index + 1);
  const file = sourcePath.slice(index + 1);
  const stem = file.replace(/\.[^.]+$/, "");
  return { sep, directory, stem };
}

/**
 * The vocal-only default filename, mirroring `vocal_out_path` in Rust: the
 * `_LYRICS` stem is unchanged and only the extension follows the target.
 */
export function defaultVocalPath(
  sourcePath: string,
  target: ExportTarget = "svp",
): string {
  const { directory, stem } = splitSourcePath(sourcePath);
  return `${directory}${stem}_LYRICS.${TARGET_EXTENSION[target]}`;
}

export function defaultBundlePath(
  sourcePath: string,
  outputDirectory?: string,
): string {
  const { sep, directory, stem } = splitSourcePath(sourcePath);
  if (!outputDirectory) return `${directory}${stem}.versebundle`;
  const trimmed = outputDirectory.replace(/[\\/]+$/, "");
  return `${trimmed}${sep}${stem}.versebundle`;
}

export function batchBundlePaths(
  sourcePaths: string[],
  outputDirectory?: string,
): Map<string, string> {
  const defaults = sourcePaths.map((path) =>
    defaultBundlePath(path, outputDirectory),
  );
  const defaultCounts = new Map<string, number>();
  for (const path of defaults) {
    const key = path.toLocaleLowerCase();
    defaultCounts.set(key, (defaultCounts.get(key) ?? 0) + 1);
  }

  const used = new Set<string>();
  const result = new Map<string, string>();
  for (let index = 0; index < sourcePaths.length; index += 1) {
    const sourcePath = sourcePaths[index];
    const defaultPath = defaults[index];
    let candidate = defaultPath;
    if ((defaultCounts.get(defaultPath.toLocaleLowerCase()) ?? 0) > 1) {
      const file = sourcePath.slice(sourcePath.lastIndexOf(separator(sourcePath)) + 1);
      const extension = file.includes(".")
        ? file.slice(file.lastIndexOf(".") + 1).toLocaleLowerCase()
        : "source";
      candidate = defaultPath.replace(
        /\.versebundle$/i,
        `.${extension}.versebundle`,
      );
    }

    const baseCandidate = candidate;
    let suffix = 2;
    while (used.has(candidate.toLocaleLowerCase())) {
      candidate = baseCandidate.replace(
        /\.versebundle$/i,
        `.${suffix}.versebundle`,
      );
      suffix += 1;
    }
    used.add(candidate.toLocaleLowerCase());
    result.set(sourcePath, candidate);
  }
  return result;
}

export function commandError(error: unknown): StructuredCommandError {
  if (typeof error === "string") {
    try {
      return commandError(JSON.parse(error));
    } catch {
      return { code: "UNKNOWN_ERROR", message: error };
    }
  }
  if (error && typeof error === "object") {
    const candidate = error as Partial<StructuredCommandError>;
    if (typeof candidate.message === "string") {
      return {
        code:
          typeof candidate.code === "string"
            ? candidate.code
            : "UNKNOWN_ERROR",
        message: candidate.message,
        remediation:
          typeof candidate.remediation === "string"
            ? candidate.remediation
            : null,
      };
    }
  }
  return {
    code: "UNKNOWN_ERROR",
    message: "An unexpected error occurred.",
  };
}

export function commandErrorMessage(error: unknown): string {
  const parsed = commandError(error);
  return parsed.remediation
    ? `${parsed.message} ${parsed.remediation}`
    : parsed.message;
}

export function isAudioUnavailableErrorCode(code: string): boolean {
  return code.startsWith("RENDERER_");
}
