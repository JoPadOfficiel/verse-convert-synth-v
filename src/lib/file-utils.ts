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

export type StructuredCommandError = {
  code: string;
  message: string;
  remediation?: string | null;
};

const supportedPattern = new RegExp(
  `\\.(${SUPPORTED_EXTENSIONS.join("|")})$`,
  "i",
);

export const isSupported = (path: string) => supportedPattern.test(path);

const separator = (path: string) => (path.includes("\\") ? "\\" : "/");

function splitSourcePath(sourcePath: string) {
  const sep = separator(sourcePath);
  const index = sourcePath.lastIndexOf(sep);
  const directory = sourcePath.slice(0, index + 1);
  const file = sourcePath.slice(index + 1);
  const stem = file.replace(/\.[^.]+$/, "");
  return { sep, directory, stem };
}

export function defaultSvpPath(sourcePath: string): string {
  const { directory, stem } = splitSourcePath(sourcePath);
  return `${directory}${stem}_LYRICS.svp`;
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
