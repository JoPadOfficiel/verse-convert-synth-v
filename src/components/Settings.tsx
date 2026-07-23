import type { ReactNode } from "react";
import { SunIcon, MoonIcon, DesktopIcon, ChevronLeftIcon, FileIcon } from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { useTheme, type Theme } from "@/components/theme-provider";
import { pickDirectory } from "@/lib/tauri";

export function Settings({
  outDir,
  setOutDir,
  onClose,
}: {
  outDir?: string;
  setOutDir: (d?: string) => void;
  onClose: () => void;
}) {
  const { theme, setTheme } = useTheme();
  const themes: { value: Theme; label: string; icon: ReactNode }[] = [
    { value: "system", label: "System", icon: <DesktopIcon /> },
    { value: "light", label: "Light", icon: <SunIcon /> },
    { value: "dark", label: "Dark", icon: <MoonIcon /> },
  ];

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
          {themes.map((t) => (
            <Button
              key={t.value}
              variant={theme === t.value ? "default" : "outline"}
              size="sm"
              onClick={() => setTheme(t.value)}
            >
              {t.icon}
              {t.label}
            </Button>
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <Label>.svp output folder</Label>
        <div className="flex items-center gap-2">
          <div className="flex-1 truncate rounded-md border bg-muted px-3 py-2 text-sm text-muted-foreground">
            {outDir ?? "Next to the source file"}
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              const d = await pickDirectory();
              if (d) setOutDir(d);
            }}
          >
            <FileIcon />
            Choose
          </Button>
          {outDir && (
            <Button variant="ghost" size="sm" onClick={() => setOutDir(undefined)}>
              Reset
            </Button>
          )}
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        Verse converts karaoke / MIDI into Synthesizer V projects (.svp) while keeping every track;
        only the real voices receive the lyrics.
      </p>
    </div>
  );
}
