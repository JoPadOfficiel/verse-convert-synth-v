import { useEffect, useState, useCallback } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { GearIcon, SunIcon, MoonIcon, DesktopIcon, TrashIcon } from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { Dropzone } from "@/components/Dropzone";
import { FileList } from "@/components/FileList";
import { Settings } from "@/components/Settings";
import {
  convertFiles,
  pickFiles,
  isSupported,
  type FileResult,
  type Language,
  type Overrides,
} from "@/lib/tauri";
import { useTheme } from "@/components/theme-provider";

export default function App() {
  const [items, setItems] = useState<FileResult[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [outDir, setOutDir] = useState<string | undefined>(undefined);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [language, setLanguage] = useState<Language>("english");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [overrides, setOverrides] = useState<Overrides>({});
  const { theme, setTheme } = useTheme();

  const addPaths = useCallback(async (paths: string[]) => {
    const supported = paths.filter(isSupported);
    if (!supported.length) return;
    const res = await convertFiles(supported, false);
    setItems((prev) => {
      const seen = new Set(prev.map((p) => p.path));
      return [...prev, ...res.filter((r) => !seen.has(r.path))];
    });
  }, []);

  useEffect(() => {
    // Guard against tiny/off-screen windows (tiling window managers, quirky
    // dev-mode startups): restore a sane size and center once, then re-check
    // shortly after in case a window manager re-tiled us.
    const w = getCurrentWindow();
    const fix = async () => {
      try {
        const s = await w.innerSize();
        const sf = await w.scaleFactor();
        if (s.width / sf < 700) {
          await w.setSize(new LogicalSize(920, 680));
          await w.center();
        }
      } catch {
        /* window API unavailable: nothing to fix */
      }
    };
    void fix();
    const t = setTimeout(() => void fix(), 1200);
    return () => clearTimeout(t);
  }, []);

  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((ev) => {
      const t = ev.payload.type;
      if (t === "enter" || t === "over") setDragging(true);
      else if (t === "leave") setDragging(false);
      else if (t === "drop") {
        setDragging(false);
        void addPaths(ev.payload.paths);
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, [addPaths]);

  async function onAdd() {
    const p = await pickFiles();
    await addPaths(p);
  }

  async function saveMany(paths: string[]) {
    setBusy(true);
    const res = await convertFiles(paths, true, language, outDir, overrides);
    setItems((prev) => prev.map((it) => res.find((r) => r.path === it.path) ?? it));
    setBusy(false);
  }

  async function toggleRole(path: string, trackId: number, sing: boolean) {
    const next: Overrides = { ...overrides, [path]: { ...(overrides[path] ?? {}), [trackId]: sing } };
    setOverrides(next);
    // re-analyze the file with the override to refresh the display
    const res = await convertFiles([path], false, language, undefined, next);
    setItems((prev) => prev.map((it) => res.find((r) => r.path === it.path) ?? it));
  }

  function toggleSelect(path: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const anyOk = items.some((i) => i.ok);
  const cycleTheme = () => setTheme(theme === "dark" ? "light" : theme === "light" ? "system" : "dark");
  const ThemeIcon = theme === "dark" ? MoonIcon : theme === "light" ? SunIcon : DesktopIcon;

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4 p-6">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Verse</h1>
          <p className="text-sm text-muted-foreground">karaoke / MIDI → Synthesizer V</p>
        </div>
        <div className="flex items-center gap-1">
          <Button variant="ghost" size="icon" onClick={cycleTheme} title="Theme">
            <ThemeIcon />
          </Button>
          <Button variant="ghost" size="icon" onClick={() => setShowSettings((s) => !s)} title="Settings">
            <GearIcon />
          </Button>
        </div>
      </header>

      {showSettings ? (
        <Settings outDir={outDir} setOutDir={setOutDir} onClose={() => setShowSettings(false)} />
      ) : (
        <>
          <Dropzone onAdd={onAdd} dragging={dragging} />
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-muted-foreground">Lyrics language</span>
            <div className="inline-flex overflow-hidden rounded-md border">
              <button
                onClick={() => setLanguage("english")}
                className={"px-3 py-1 text-sm " + (language === "english" ? "bg-primary text-primary-foreground" : "hover:bg-accent")}
              >
                English
              </button>
              <button
                onClick={() => setLanguage("french")}
                className={"px-3 py-1 text-sm " + (language === "french" ? "bg-primary text-primary-foreground" : "hover:bg-accent")}
              >
                French
              </button>
            </div>
            <div className="flex-1" />
            {items.length > 0 && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  setItems([]);
                  setSelected(new Set());
                }}
              >
                <TrashIcon />
                Clear
              </Button>
            )}
            {selected.size > 0 && (
              <Button variant="secondary" disabled={busy} onClick={() => saveMany([...selected])}>
                Download selection ({selected.size})
              </Button>
            )}
            <Button disabled={!anyOk || busy} onClick={() => saveMany(items.filter((i) => i.ok).map((i) => i.path))}>
              Convert all
            </Button>
          </div>
          <FileList
            items={items}
            onDownload={(it) => saveMany([it.path])}
            selected={selected}
            onToggleSelect={toggleSelect}
            onToggleRole={toggleRole}
          />
        </>
      )}
    </div>
  );
}
