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
  FileResult,
  LyricStatus,
  SourceRole,
  TrackInfo,
} from "@/lib/tauri";

const ROLE_LABEL: Record<SourceRole, string> = {
  vocal: "Source vocal",
  instrumental: "Instrument",
  percussion: "Percussion",
  mixed: "Mixed source",
  lyricsOnly: "Lyrics-only source",
  metadata: "Metadata",
  ambiguous: "Unspecified musical role",
};

const REPRESENTATION_LABEL: Record<ExportRepresentation, string> = {
  vocalNotes: "Vocal notes in SVP",
  referenceMixMember: "Included in full-score audio",
  vocalNotesAndReferenceMix: "Vocal notes + full-score audio",
  sourceOnly: "Preserved in source/manifest",
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

function TrackRow({
  track,
  disabled,
  onToggle,
}: {
  track: TrackInfo;
  disabled: boolean;
  onToggle: (trackId: number, enabled: boolean) => void;
}) {
  const vocalExport = exportsVocalNotes(track.exportRepresentation);
  const canToggle =
    track.notes > 0 &&
    !["percussion", "metadata", "lyricsOnly"].includes(track.sourceRole);
  const color =
    track.sourceRole === "vocal"
      ? "text-success"
      : track.sourceRole === "percussion"
        ? "text-warning"
        : "text-muted-foreground";

  return (
    <div className="border-b py-2 last:border-b-0">
      <div className="flex items-center gap-2 text-sm">
        <DotFilledIcon className={`${color} size-4 shrink-0`} />
        <span className="w-44 truncate font-medium" title={track.track}>
          {track.track}
        </span>
        <span className="w-20 text-right tabular-nums text-muted-foreground">
          {track.notes} notes
        </span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">
          · {ROLE_LABEL[track.sourceRole]} · {lyricLabel(track.lyricStatus)}
        </span>
        {canToggle && (
          <button
            disabled={disabled}
            onClick={(event) => {
              event.stopPropagation();
              onToggle(track.id, !vocalExport);
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
            Vocal SVP {vocalExport ? "on" : "off"}
          </button>
        )}
      </div>
      <div className="ml-6 mt-1 text-xs text-muted-foreground">
        {REPRESENTATION_LABEL[track.exportRepresentation]}
        {track.requiresVoiceAssignment &&
          " · Assign a Synthesizer V voice before playback"}
      </div>
      {track.warnings.map((warning) => (
        <div
          key={`${warning.code}-${warning.sourceId ?? ""}`}
          className={
            "ml-6 mt-1 text-xs " +
            (warning.severity === "warning"
              ? "text-warning"
              : "text-muted-foreground")
          }
        >
          {warning.message}
        </div>
      ))}
    </div>
  );
}

function Row({
  item,
  busy,
  exportError,
  onBundle,
  onVocals,
  selected,
  onToggleSelect,
  onToggleVocal,
}: {
  item: FileResult;
  busy: boolean;
  exportError?: string;
  onBundle: (item: FileResult) => void;
  onVocals: (item: FileResult) => void;
  selected: boolean;
  onToggleSelect: (path: string) => void;
  onToggleVocal: (path: string, trackId: number, enabled: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const vocalTracks = item.tracks.filter((track) =>
    exportsVocalNotes(track.exportRepresentation),
  ).length;
  const hasVocalExport = vocalTracks > 0;
  const analysisError = item.error?.message ?? item.msg;
  const audioSummary =
    item.audioStatus.state === "available"
      ? `Audio ready · ${item.audioStatus.channels} ch · ${item.audioStatus.sampleRate} Hz`
      : item.audioStatus.state === "unavailable"
        ? "Audio unavailable"
        : "Audio not rendered yet";

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
                ? `${item.nTracks} source tracks · ${vocalTracks} vocal exports · ${item.placed} projected lyrics`
                : analysisError}
            </div>
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
                A voice database must be assigned in Synthesizer V.
              </div>
            )}
            {item.out && (
              <div className="truncate text-xs text-success" title={item.out}>
                Saved: {item.out}
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
        {item.ok && (
          <div className="flex shrink-0 flex-col gap-1">
            <Button
              size="sm"
              disabled={busy}
              title="Create an auditable bundle with source, SVP, manifest and a real full-score WAV"
              onClick={() => onBundle(item)}
            >
              <DownloadIcon /> Bundle
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
              Vocals .svp
            </Button>
          </div>
        )}
      </div>
      {open && item.ok && (
        <div className="border-t px-4 py-2 pl-11">
          <p className="mb-2 text-xs text-muted-foreground">
            Bundle audio is the original full-score reference mix. It keeps
            piano, instruments and percussion audible, but it is not a
            vocal-removed accompaniment stem.
          </p>
          {item.tracks.map((track) => (
            <TrackRow
              key={track.sourceId}
              track={track}
              disabled={busy}
              onToggle={(trackId, enabled) =>
                onToggleVocal(item.path, trackId, enabled)
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
  onBundle,
  onVocals,
  selected,
  onToggleSelect,
  onToggleVocal,
}: {
  items: FileResult[];
  busy: boolean;
  exportErrors: Record<string, string>;
  onBundle: (item: FileResult) => void;
  onVocals: (item: FileResult) => void;
  selected: Set<string>;
  onToggleSelect: (path: string) => void;
  onToggleVocal: (path: string, trackId: number, enabled: boolean) => void;
}) {
  if (!items.length) {
    return (
      <div className="py-8 text-center text-sm text-muted-foreground">
        No files yet.
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2 overflow-y-auto">
      {items.map((item) => (
        <Row
          key={item.path}
          item={item}
          busy={busy}
          exportError={exportErrors[item.path]}
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
