import type { ReactNode } from "react";
import {
  ChevronLeftIcon,
  DesktopIcon,
  FileIcon,
  MoonIcon,
  SunIcon,
} from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { useTheme, type Theme } from "@/components/theme-provider";
import {
  pickDirectory,
  pickRenderer,
  type RendererStatus,
} from "@/lib/tauri";

export function Settings({
  outDir,
  setOutDir,
  rendererPath,
  setRendererPath,
  rendererStatus,
  onClose,
}: {
  outDir?: string;
  setOutDir: (directory?: string) => void;
  rendererPath?: string;
  setRendererPath: (path?: string) => void;
  rendererStatus: RendererStatus | null;
  onClose: () => void;
}) {
  const { theme, setTheme } = useTheme();
  const themes: { value: Theme; label: string; icon: ReactNode }[] = [
    { value: "system", label: "System", icon: <DesktopIcon /> },
    { value: "light", label: "Light", icon: <SunIcon /> },
    { value: "dark", label: "Dark", icon: <MoonIcon /> },
  ];

  const rendererMessage =
    rendererStatus === null
      ? "Checking MuseScore Studio…"
      : rendererStatus.state === "available"
        ? `${rendererStatus.provider ?? "MuseScore"} ready${rendererStatus.version ? ` · ${rendererStatus.version}` : ""}`
        : rendererStatus.message ??
          "MuseScore Studio 4 is required for complete bundles.";

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="icon" onClick={onClose} title="Back">
          <ChevronLeftIcon />
        </Button>
        <h2 className="text-lg font-semibold">Settings</h2>
      </div>

      <div className="flex flex-col gap-2">
        <Label>Appearance</Label>
        <div className="flex gap-2">
          {themes.map((option) => (
            <Button
              key={option.value}
              variant={theme === option.value ? "default" : "outline"}
              size="sm"
              onClick={() => setTheme(option.value)}
            >
              {option.icon}
              {option.label}
            </Button>
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <Label>Bundle output folder</Label>
        <div className="flex items-center gap-2">
          <div className="flex-1 truncate rounded-md border bg-muted px-3 py-2 text-sm text-muted-foreground">
            {outDir ?? "Next to each source file"}
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              const directory = await pickDirectory();
              if (directory) setOutDir(directory);
            }}
          >
            <FileIcon />
            Choose
          </Button>
          {outDir && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setOutDir(undefined)}
            >
              Reset
            </Button>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <Label htmlFor="renderer-path">MuseScore Studio 4 renderer</Label>
        <div className="flex items-center gap-2">
          <input
            id="renderer-path"
            value={rendererPath ?? ""}
            onChange={(event) => setRendererPath(event.target.value)}
            placeholder="Auto-detect MuseScore Studio 4"
            className="min-w-0 flex-1 rounded-md border bg-background px-3 py-2 text-sm"
          />
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              const path = await pickRenderer();
              if (path) setRendererPath(path);
            }}
          >
            <FileIcon />
            Choose
          </Button>
          {rendererPath && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setRendererPath(undefined)}
            >
              Auto
            </Button>
          )}
        </div>
        <p
          className={
            "text-xs " +
            (rendererStatus?.state === "available"
              ? "text-success"
              : rendererStatus
                ? "text-warning"
                : "text-muted-foreground")
          }
        >
          {rendererMessage}
        </p>
        <p className="text-xs text-muted-foreground">
          MuseScore is not bundled with Verse. Complete bundle export is
          blocked when MuseScore Studio 4 cannot be detected or validated;
          Verse never creates fake audio.
        </p>
      </div>

      <div className="rounded-md border bg-muted/40 p-3 text-xs text-muted-foreground">
        A complete <code>.versebundle</code> contains the byte-identical source,
        an auditable manifest, editable vocal notes and a real WAV of the
        original full score. Assign a voice database in Synthesizer V to every
        vocal track. The WAV is a reference mix, not a clean vocal-removed
        accompaniment.
      </div>
    </div>
  );
}
