import { useState } from "react";
import {
  ChevronRightIcon,
  DotFilledIcon,
  DownloadIcon,
  ExclamationTriangleIcon,
  SpeakerLoudIcon,
} from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import type {
  ExportRepresentation,
  ExportTarget,
  FileResult,
  LyricStatus,
  PartInfo,
  SourceRole,
} from "@/lib/tauri";
import {
  exportProgressPercent,
  type FileExportProgress,
} from "@/lib/export-progress";
import { groupDiagnostics } from "@/lib/diagnostics";

const ROLE_LABEL: Record<SourceRole, string> = {
  vocal: "Source vocal",
  instrumental: "Instrument",
  percussion: "Percussion",
  mixed: "Mixed source",
  lyricsOnly: "Lyrics-only source",
  metadata: "Metadata",
  ambiguous: "Unspecified musical role",
};

/**
 * The words this copy borrows from the selected export target. Each target names
 * its own file format, its own singing-voice concept — Synthesizer V assigns a
 * voice database, OpenUtau assigns a singer — and its own way of referencing the
 * bundle's rendered audio.
 */
const TARGET_COPY: Record<
  ExportTarget,
  {
    format: string;
    assignVoice: string;
    assignVoiceInApp: string;
    audioReference: string;
  }
> = {
  svp: {
    format: "SVP",
    assignVoice: "Assign a Synthesizer V voice before playback",
    assignVoiceInApp: "A voice database must be assigned in Synthesizer V.",
    audioReference: "audio-backed instrumental track",
  },
  ustx: {
    format: "USTX",
    assignVoice: "Assign an OpenUtau singer before playback",
    assignVoiceInApp: "A singer must be assigned in OpenUtau.",
    audioReference: "wave part",
  },
};

// Kept keyed by representation so a new one cannot be forgotten, and per-target
// only where the label actually names the format.
const REPRESENTATION_LABEL: Record<
  ExportRepresentation,
  (target: ExportTarget) => string
> = {
  vocalNotes: (target) => `Vocal notes in ${TARGET_COPY[target].format}`,
  referenceMixMember: () => "Separate MuseScore Part stem in the bundle",
  vocalNotesAndReferenceMix: () =>
    "Vocal notes + separate muted Part reference stem",
  sourceOnly: () => "Preserved in source/manifest",
};

function lyricLabel(status: LyricStatus): string {
  switch (status.state) {
    case "sourceOwned":
      return `${status.sourceTextCount} source lyric${status.sourceTextCount === 1 ? "" : "s"}`;
    case "explicitEmpty":
      return "Explicit empty lyrics";
    case "metadataOnly":
      return "MIDI Text kept as metadata";
    case "ambiguous":
      return "Lyrics preserved; assignment ambiguous";
    case "unsupported":
      return "Unsupported lyric content preserved";
    default:
      return "No source lyrics";
  }
}

function exportsVocalNotes(representation: ExportRepresentation): boolean {
  return (
    representation === "vocalNotes" ||
    representation === "vocalNotesAndReferenceMix"
  );
}

function PartRow({
  part,
  disabled,
  exportTarget,
  onToggle,
}: {
  part: PartInfo;
  disabled: boolean;
  exportTarget: ExportTarget;
  onToggle: (trackIds: number[], enabled: boolean) => void;
}) {
  const vocalExport = exportsVocalNotes(part.exportRepresentation);
  const canToggle = part.vocalCandidateTrackIds.length > 0;
  const color =
    part.sourceRole === "vocal"
      ? "text-success"
      : part.sourceRole === "percussion"
        ? "text-warning"
        : "text-muted-foreground";

  return (
    <div className="border-b py-2 last:border-b-0">
      <div className="flex items-center gap-2 text-sm">
        <DotFilledIcon className={`${color} size-4 shrink-0`} />
        <span className="w-44 truncate font-medium" title={part.part}>
          {part.part}
        </span>
        <span className="w-20 text-right tabular-nums text-muted-foreground">
          {part.notes} notes
        </span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">
          · {ROLE_LABEL[part.sourceRole]} · {part.staves} staff
          {part.staves === 1 ? "" : "s"} · {part.voices} voice
          {part.voices === 1 ? "" : "s"} · {lyricLabel(part.lyricStatus)}
        </span>
        {canToggle && (
          <button
            disabled={disabled}
            onClick={(event) => {
              event.stopPropagation();
              onToggle(part.vocalCandidateTrackIds, !vocalExport);
            }}
            title={
              vocalExport
                ? "Do not create a vocal-note track for this source track"
                : "Explicitly export these pitched notes as a vocal track"
            }
            className={
              "inline-flex shrink-0 items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs transition-colors disabled:opacity-50 " +
              (vocalExport
                ? "border-transparent bg-secondary font-medium"
                : "border-input text-muted-foreground hover:bg-accent")
            }
          >
            <SpeakerLoudIcon className="size-3" />
            Vocal {TARGET_COPY[exportTarget].format}{" "}
            {vocalExport ? "on" : "off"}
          </button>
        )}
      </div>
      <div className="ml-6 mt-1 text-xs text-muted-foreground">
        {REPRESENTATION_LABEL[part.exportRepresentation](exportTarget)}
        {part.requiresVoiceAssignment &&
          ` · ${TARGET_COPY[exportTarget].assignVoice}`}
      </div>
      {/* Grouped for display only: a per-note diagnostic is raised once per
          affected note, and one identical sentence repeated hundreds of times
          would hide every other diagnostic on this Part. */}
      {groupDiagnostics(part.warnings).map((warning) => (
        <div
          key={JSON.stringify([
            warning.severity,
            warning.code,
            warning.message,
          ])}
          className={
            "ml-6 mt-1 text-xs " +
            (warning.severity === "warning"
              ? "text-warning"
              : "text-muted-foreground")
          }
        >
          {warning.message}
          {/* "Occurrences" rather than "notes": the same grouping serves
              track-level diagnostics, whose source ID is not a note. */}
          {warning.count > 1 && ` · ${warning.count} occurrences`}
        </div>
      ))}
    </div>
  );
}

function Row({
  item,
  busy,
  exportError,
  exportProgress,
  exportTarget,
  onBundle,
  onVocals,
  selected,
  onToggleSelect,
  onToggleVocal,
}: {
  item: FileResult;
  busy: boolean;
  exportError?: string;
  exportProgress?: FileExportProgress;
  exportTarget: ExportTarget;
  onBundle: (item: FileResult) => void;
  onVocals: (item: FileResult) => void;
  selected: boolean;
  onToggleSelect: (path: string) => void;
  onToggleVocal: (
    path: string,
    trackIds: readonly number[],
    enabled: boolean,
  ) => void;
}) {
  const [open, setOpen] = useState(false);
  const vocalTracks = item.parts.filter((part) =>
    exportsVocalNotes(part.exportRepresentation),
  ).length;
  const hasVocalExport = vocalTracks > 0;
  const analysisError = item.error?.message ?? item.msg;
  const audioSummary =
    item.audioStatus.state === "available"
      ? `Audio ready · ${item.audioStatus.channels} ch · ${item.audioStatus.sampleRate} Hz`
      : item.audioStatus.state === "unavailable"
        ? "Audio unavailable"
        : "Audio not rendered yet";
  const progressPercent = exportProgress
    ? exportProgressPercent(exportProgress)
    : 0;
  // Counted after grouping, so one sentence repeated over 300 notes reads as one
  // warning rather than 300 — the same collapse the expanded rows show.
  const warningCount = item.parts.reduce(
    (total, part) => total + groupDiagnostics(part.warnings).length,
    0,
  );

  return (
    <div className="rounded-lg border bg-card">
      <div className="flex items-center gap-3 p-3">
        {item.ok ? (
          <input
            type="checkbox"
            checked={selected}
            disabled={busy}
            onChange={() => onToggleSelect(item.path)}
            className="size-4 shrink-0 accent-primary"
            title="Select for bundle export"
          />
        ) : (
          <ExclamationTriangleIcon className="size-5 shrink-0 text-warning" />
        )}
        <button
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          onClick={() => item.ok && setOpen((shown) => !shown)}
        >
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium">{item.name}</div>
            <div className="truncate text-xs text-muted-foreground">
              {item.ok
                ? `${item.nParts} source Parts · ${item.nVoices} voices · ${vocalTracks} vocal exports · ${item.placed} projected lyrics`
                : analysisError}
            </div>
            {/* Diagnostics used to live only inside the expanded Part rows, so a
                warning about a lyric the target reinterprets went unseen unless the
                row happened to be opened. Surfaced here as a count, with the
                detail still one click away. */}
            {item.ok && warningCount > 0 && (
              <div className="flex items-center gap-1 truncate text-xs text-warning">
                <ExclamationTriangleIcon className="size-3 shrink-0" />
                {warningCount} warning{warningCount === 1 ? "" : "s"} · expand
                for detail
              </div>
            )}
            {item.ok && (
              <div
                className={
                  "truncate text-xs " +
                  (item.audioStatus.state === "unavailable"
                    ? "text-warning"
                    : "text-muted-foreground")
                }
              >
                {audioSummary}
              </div>
            )}
            {item.requiresVoiceAssignment && (
              <div className="truncate text-xs text-warning">
                {TARGET_COPY[exportTarget].assignVoiceInApp}
              </div>
            )}
            {item.out && (
              <div className="truncate text-xs text-success" title={item.out}>
                Saved: {item.out}
              </div>
            )}
            {exportProgress && (
              <div className="mt-1" aria-live="polite">
                <div
                  className={
                    "flex items-center justify-between gap-2 text-xs " +
                    (exportProgress.phase === "failed"
                      ? "text-destructive"
                      : exportProgress.phase === "finished"
                        ? "text-success"
                        : "text-muted-foreground")
                  }
                >
                  <span className="truncate">{exportProgress.message}</span>
                  <span className="shrink-0 tabular-nums">
                    {exportProgress.phase === "finished"
                      ? "Done"
                      : `${exportProgress.completed} / ${exportProgress.total}`}
                  </span>
                </div>
                <div
                  role="progressbar"
                  aria-label={`Complete project progress for ${item.name}`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(progressPercent)}
                  className="mt-1 h-1.5 overflow-hidden rounded-full bg-secondary"
                >
                  <div
                    className={
                      "h-full rounded-full transition-[width] duration-300 " +
                      (exportProgress.phase === "failed"
                        ? "bg-destructive"
                        : exportProgress.phase === "finished"
                          ? "bg-success"
                          : "bg-primary")
                    }
                    style={{ width: `${progressPercent}%` }}
                  />
                </div>
              </div>
            )}
            {exportError && (
              <div className="text-xs text-destructive" role="alert">
                {exportError}
              </div>
            )}
          </div>
          {item.ok && (
            <ChevronRightIcon
              className={
                "size-4 shrink-0 text-muted-foreground transition-transform " +
                (open ? "rotate-90" : "")
              }
            />
          )}
        </button>
        {/* `bundleReady` rather than `item.ok`: the backend answers whether a
            bundle can be written for this source, so the webview never re-derives
            which format's rules the bundle is held to. */}
        {(item.ok || item.bundleReady) && (
          <div className="flex shrink-0 flex-col gap-1">
            <Button
              size="sm"
              disabled={busy}
              // The bundle carries the selected target's project, referencing the
              // same stems the other target would reference.
              title={`Create an auditable bundle with source, a ${TARGET_COPY[exportTarget].format} project, one audio stem per MuseScore Part, and a muted full-score reference`}
              onClick={() => onBundle(item)}
            >
              <DownloadIcon /> Complete project
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy || !hasVocalExport}
              title={
                hasVocalExport
                  ? "Save vocal notes only; instruments require the complete bundle"
                  : "No vocal-note track is selected"
              }
              onClick={() => onVocals(item)}
            >
              Vocals only
            </Button>
          </div>
        )}
      </div>
      {open && item.ok && (
        <div className="border-t px-4 py-2 pl-11">
          <p className="mb-2 text-xs text-muted-foreground">
            Each note-bearing source Part becomes its own MuseScore-rendered{" "}
            {TARGET_COPY[exportTarget].audioReference}. Vocal reference Parts and
            the full-score reference are muted by default; accompaniment Parts
            remain audible.
          </p>
          {item.parts.map((part) => (
            <PartRow
              key={part.sourceId}
              part={part}
              disabled={busy}
              exportTarget={exportTarget}
              onToggle={(trackIds, enabled) =>
                onToggleVocal(item.path, trackIds, enabled)
              }
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function FileList({
  items,
  busy,
  exportErrors,
  exportProgress,
  exportTarget,
  onBundle,
  onVocals,
  selected,
  onToggleSelect,
  onToggleVocal,
}: {
  items: FileResult[];
  busy: boolean;
  exportErrors: Record<string, string>;
  exportProgress: Record<string, FileExportProgress>;
  exportTarget: ExportTarget;
  onBundle: (item: FileResult) => void;
  onVocals: (item: FileResult) => void;
  selected: Set<string>;
  onToggleSelect: (path: string) => void;
  onToggleVocal: (
    path: string,
    trackIds: readonly number[],
    enabled: boolean,
  ) => void;
}) {
  if (!items.length) {
    return (
      <div className="py-8 text-center text-sm text-muted-foreground">
        No files yet.
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto">
      {items.map((item) => (
        <Row
          key={item.path}
          item={item}
          busy={busy}
          exportError={exportErrors[item.path]}
          exportProgress={exportProgress[item.path]}
          exportTarget={exportTarget}
          onBundle={onBundle}
          onVocals={onVocals}
          selected={selected.has(item.path)}
          onToggleSelect={onToggleSelect}
          onToggleVocal={onToggleVocal}
        />
      ))}
    </div>
  );
}
