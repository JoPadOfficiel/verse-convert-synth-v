import { useState } from "react";
import {
  ExclamationTriangleIcon,
  ChevronRightIcon,
  DotFilledIcon,
  DownloadIcon,
  SpeakerLoudIcon,
  SpeakerOffIcon,
} from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import type { FileResult, TrackInfo } from "@/lib/tauri";

function TrackRow({ t, onToggle }: { t: TrackInfo; onToggle: (trackId: number, sing: boolean) => void }) {
  const singing = t.role === "vocal" || t.role === "vocal_synth";
  const color =
    t.role === "vocal" ? "text-success" : t.role === "vocal_synth" ? "text-warning" : "text-muted-foreground";
  const tail =
    t.role === "vocal"
      ? `${t.placed} syllables`
      : t.role === "vocal_synth"
        ? "lyrics at the right timing, melody to adjust"
        : "backing";
  return (
    <div className="flex items-center gap-2 py-0.5 text-sm">
      <DotFilledIcon className={color + " size-4 shrink-0"} />
      <span className={"w-52 truncate " + (singing ? "font-medium" : "text-muted-foreground")}>{t.track}</span>
      <span className="w-20 text-right tabular-nums text-muted-foreground">{t.notes} notes</span>
      <span className="min-w-0 flex-1 truncate text-muted-foreground">· {tail}</span>
      {t.role !== "vocal_synth" && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onToggle(t.id, !singing);
          }}
          title={singing ? "Mute this track" : "Make it sing"}
          className={
            "inline-flex shrink-0 items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs transition-colors " +
            (singing
              ? "border-transparent bg-secondary font-medium"
              : "border-input text-muted-foreground hover:bg-accent")
          }
        >
          {singing ? <SpeakerLoudIcon className="size-3" /> : <SpeakerOffIcon className="size-3" />}
          {singing ? "Sings" : "Muted"}
        </button>
      )}
    </div>
  );
}

function Row({
  item,
  onDownload,
  selected,
  onToggleSelect,
  onToggleRole,
}: {
  item: FileResult;
  onDownload: (it: FileResult) => void;
  selected: boolean;
  onToggleSelect: (path: string) => void;
  onToggleRole: (path: string, trackId: number, sing: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const singing = item.ok ? item.tracks.filter((t) => t.role === "vocal" || t.role === "vocal_synth").length : 0;
  return (
    <div className="rounded-lg border bg-card">
      <div className="flex items-center gap-3 p-3">
        {item.ok ? (
          <input
            type="checkbox"
            checked={selected}
            onChange={() => onToggleSelect(item.path)}
            className="size-4 shrink-0 accent-primary"
            title="Select"
          />
        ) : (
          <ExclamationTriangleIcon className="size-5 shrink-0 text-warning" />
        )}
        <button
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          onClick={() => item.ok && setOpen((o) => !o)}
        >
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium">{item.name}</div>
            <div className="truncate text-xs text-muted-foreground">
              {item.ok
                ? `${item.nTracks} tracks · ${singing} vocal · ${item.placed} syllables`
                : item.msg}
            </div>
            {item.out && (
              <div className="truncate text-xs text-success" title={item.out}>
                Saved: {item.out}
              </div>
            )}
          </div>
          {item.ok && (
            <ChevronRightIcon
              className={"size-4 shrink-0 text-muted-foreground transition-transform " + (open ? "rotate-90" : "")}
            />
          )}
        </button>
        {item.ok && (
          <Button
            size="sm"
            variant={item.out ? "secondary" : "default"}
            title="Convert and save the .svp next to the source file"
            onClick={() => onDownload(item)}
          >
            <DownloadIcon /> Download
          </Button>
        )}
      </div>
      {open && item.ok && (
        <div className="border-t px-4 py-2 pl-11">
          {item.tracks.map((t) => (
            <TrackRow key={t.id} t={t} onToggle={(id, sing) => onToggleRole(item.path, id, sing)} />
          ))}
        </div>
      )}
    </div>
  );
}

export function FileList({
  items,
  onDownload,
  selected,
  onToggleSelect,
  onToggleRole,
}: {
  items: FileResult[];
  onDownload: (it: FileResult) => void;
  selected: Set<string>;
  onToggleSelect: (path: string) => void;
  onToggleRole: (path: string, trackId: number, sing: boolean) => void;
}) {
  if (!items.length) {
    return <div className="py-8 text-center text-sm text-muted-foreground">No files yet.</div>;
  }
  return (
    <div className="flex flex-col gap-2 overflow-y-auto">
      {items.map((it) => (
        <Row
          key={it.path}
          item={it}
          onDownload={onDownload}
          selected={selected.has(it.path)}
          onToggleSelect={onToggleSelect}
          onToggleRole={onToggleRole}
        />
      ))}
    </div>
  );
}
