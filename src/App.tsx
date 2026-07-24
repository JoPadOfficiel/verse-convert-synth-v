import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import {
  DesktopIcon,
  GearIcon,
  MoonIcon,
  SunIcon,
  TrashIcon,
} from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { Dropzone } from "@/components/Dropzone";
import { FileList } from "@/components/FileList";
import { Settings } from "@/components/Settings";
import {
  chooseBundleTarget,
  commandError,
  commandErrorMessage,
  convertFiles,
  defaultBundlePath,
  exportBundle,
  exportVocalsWithDialog,
  getRendererStatus,
  isAudioUnavailableErrorCode,
  pickFiles,
  uniqueSupportedPaths,
  type FileResult,
  type Language,
  type Overrides,
  type RendererStatus,
} from "@/lib/tauri";
import { useTheme } from "@/components/theme-provider";

const RENDERER_PATH_KEY = "verse.rendererPath";

function storedRendererPath(): string | undefined {
  try {
    return localStorage.getItem(RENDERER_PATH_KEY) || undefined;
  } catch {
    return undefined;
  }
}

export default function App() {
  const [items, setItems] = useState<FileResult[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [outDir, setOutDir] = useState<string | undefined>(undefined);
  const [rendererPath, setRendererPathState] = useState<string | undefined>(
    storedRendererPath,
  );
  const [rendererStatus, setRendererStatus] = useState<RendererStatus | null>(
    null,
  );
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [language, setLanguage] = useState<Language>("english");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [overrides, setOverrides] = useState<Overrides>({});
  const [exportErrors, setExportErrors] = useState<Record<string, string>>({});
  const [globalError, setGlobalError] = useState<string | null>(null);
  const { theme, setTheme } = useTheme();

  const beginBusy = useCallback(() => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    return true;
  }, []);

  const endBusy = useCallback(() => {
    busyRef.current = false;
    setBusy(false);
  }, []);

  const setRendererPath = useCallback((path?: string) => {
    const normalized = path?.trim() || undefined;
    setRendererPathState(normalized);
    try {
      if (normalized) localStorage.setItem(RENDERER_PATH_KEY, normalized);
      else localStorage.removeItem(RENDERER_PATH_KEY);
    } catch {
      // Storage may be disabled; the setting still works for this session.
    }
  }, []);

  useEffect(() => {
    let current = true;
    setRendererStatus(null);
    const timer = setTimeout(() => {
      void getRendererStatus(rendererPath)
        .then((status) => {
          if (current) setRendererStatus(status);
        })
        .catch((error) => {
          if (current) {
            setRendererStatus({
              state: "missing",
              configured: Boolean(rendererPath),
              provider: null,
              version: null,
              fullScoreMix: false,
              message: commandErrorMessage(error),
            });
          }
        });
    }, 300);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [rendererPath]);

  const addPaths = useCallback(
    async (paths: string[]) => {
      const supported = uniqueSupportedPaths(paths);
      if (!supported.length) return;
      if (!beginBusy()) return;
      setGlobalError(null);
      try {
        const results = await convertFiles(
          supported,
          false,
          language,
          undefined,
          overrides,
        );
        setItems((previous) => {
          const seen = new Set(previous.map((item) => item.path));
          return [
            ...previous,
            ...results.filter((result) => !seen.has(result.path)),
          ];
        });
      } catch (error) {
        setGlobalError(commandErrorMessage(error));
      } finally {
        endBusy();
      }
    },
    [beginBusy, endBusy, language, overrides],
  );

  useEffect(() => {
    const window = getCurrentWindow();
    const fix = async () => {
      try {
        const size = await window.innerSize();
        const scaleFactor = await window.scaleFactor();
        if (size.width / scaleFactor < 700) {
          await window.setSize(new LogicalSize(920, 680));
          await window.center();
        }
      } catch {
        // Window APIs are unavailable in a browser-only preview.
      }
    };
    void fix();
    const timer = setTimeout(() => void fix(), 1200);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const type = event.payload.type;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "leave") setDragging(false);
      else if (type === "drop") {
        setDragging(false);
        void addPaths(event.payload.paths);
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [addPaths]);

  async function onAdd() {
    try {
      await addPaths(await pickFiles());
    } catch (error) {
      setGlobalError(commandErrorMessage(error));
    }
  }

  async function changeLanguage(nextLanguage: Language) {
    if (nextLanguage === language) return;
    if (!items.length) {
      setLanguage(nextLanguage);
      return;
    }
    if (!beginBusy()) return;
    setGlobalError(null);
    try {
      const results = await convertFiles(
        items.map((item) => item.path),
        false,
        nextLanguage,
        undefined,
        overrides,
      );
      setLanguage(nextLanguage);
      setItems(results);
      setSelected(
        (previous) =>
          new Set(
            [...previous].filter((path) =>
              results.some((result) => result.path === path && result.ok),
            ),
          ),
      );
    } catch (error) {
      setGlobalError(commandErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  async function runBundleExport(item: FileResult, target: string) {
    setExportErrors((previous) => {
      const next = { ...previous };
      delete next[item.path];
      return next;
    });
    try {
      const result = await exportBundle(
        item,
        target,
        language,
        overrides[item.path],
        rendererPath,
      );
      setItems((previous) =>
        previous.map((candidate) =>
          candidate.path === item.path
            ? {
                ...candidate,
                out: result.bundlePath,
                audioStatus: {
                  state: "available",
                  path: result.audioPath,
                  durationSeconds: result.audioDurationSeconds,
                  sampleRate: result.audioSampleRate,
                  channels: result.audioChannels,
                  fullScoreMix: true,
                },
              }
            : candidate,
        ),
      );
    } catch (error) {
      const parsed = commandError(error);
      setExportErrors((previous) => ({
        ...previous,
        [item.path]: commandErrorMessage(parsed),
      }));
      if (isAudioUnavailableErrorCode(parsed.code)) {
        setItems((previous) =>
          previous.map((candidate) =>
            candidate.path === item.path
              ? {
                  ...candidate,
                  audioStatus: {
                    state: "unavailable",
                    code: parsed.code,
                    message: parsed.message,
                  },
                }
              : candidate,
          ),
        );
      }
    }
  }

  async function exportOneBundle(item: FileResult) {
    if (!beginBusy()) return;
    setGlobalError(null);
    try {
      const target = outDir
        ? defaultBundlePath(item.path, outDir)
        : await chooseBundleTarget(item.path);
      if (target) await runBundleExport(item, target);
    } catch (error) {
      setGlobalError(commandErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  async function exportManyBundles(paths: string[]) {
    if (!beginBusy()) return;
    setGlobalError(null);
    try {
      for (const path of paths) {
        const item = items.find((candidate) => candidate.path === path);
        if (!item?.ok) continue;
        await runBundleExport(item, defaultBundlePath(path, outDir));
      }
    } catch (error) {
      setGlobalError(commandErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  async function exportVocals(item: FileResult) {
    if (!beginBusy()) return;
    setGlobalError(null);
    try {
      const saved = await exportVocalsWithDialog(
        item,
        language,
        overrides[item.path],
      );
      if (saved) {
        setItems((previous) =>
          previous.map((candidate) =>
            candidate.path === item.path
              ? { ...candidate, out: saved }
              : candidate,
          ),
        );
      }
    } catch (error) {
      setExportErrors((previous) => ({
        ...previous,
        [item.path]: commandErrorMessage(error),
      }));
    } finally {
      endBusy();
    }
  }

  async function toggleVocalExport(
    path: string,
    trackId: number,
    enabled: boolean,
  ) {
    if (!beginBusy()) return;
    const previousOverrides = overrides;
    const next: Overrides = {
      ...overrides,
      [path]: { ...(overrides[path] ?? {}), [trackId]: enabled },
    };
    setOverrides(next);
    setGlobalError(null);
    try {
      const results = await convertFiles(
        [path],
        false,
        language,
        undefined,
        next,
      );
      setItems((previous) =>
        previous.map(
          (item) =>
            results.find((result) => result.path === item.path) ?? item,
        ),
      );
    } catch (error) {
      setOverrides(previousOverrides);
      setGlobalError(commandErrorMessage(error));
    } finally {
      endBusy();
    }
  }

  function toggleSelect(path: string) {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const validItems = items.filter((item) => item.ok);
  const cycleTheme = () =>
    setTheme(
      theme === "dark" ? "light" : theme === "light" ? "system" : "dark",
    );
  const ThemeIcon =
    theme === "dark" ? MoonIcon : theme === "light" ? SunIcon : DesktopIcon;

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4 p-6">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Verse</h1>
          <p className="text-sm text-muted-foreground">
            MIDI / score → Synthesizer V vocals + audible reference mix
          </p>
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            onClick={cycleTheme}
            title="Theme"
          >
            <ThemeIcon />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setShowSettings((shown) => !shown)}
            title="Settings"
          >
            <GearIcon />
          </Button>
        </div>
      </header>

      {showSettings ? (
        <Settings
          outDir={outDir}
          setOutDir={setOutDir}
          rendererPath={rendererPath}
          setRendererPath={setRendererPath}
          rendererStatus={rendererStatus}
          onClose={() => setShowSettings(false)}
        />
      ) : (
        <>
          <Dropzone onAdd={onAdd} dragging={dragging} disabled={busy} />
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-muted-foreground">
              Vocal language
            </span>
            <div className="inline-flex overflow-hidden rounded-md border">
              <button
                disabled={busy}
                onClick={() => void changeLanguage("english")}
                className={
                  "px-3 py-1 text-sm disabled:opacity-50 " +
                  (language === "english"
                    ? "bg-primary text-primary-foreground"
                    : "hover:bg-accent")
                }
              >
                English
              </button>
              <button
                disabled={busy}
                onClick={() => void changeLanguage("french")}
                className={
                  "px-3 py-1 text-sm disabled:opacity-50 " +
                  (language === "french"
                    ? "bg-primary text-primary-foreground"
                    : "hover:bg-accent")
                }
              >
                French
              </button>
            </div>
            <div className="flex-1" />
            {items.length > 0 && (
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => {
                  setItems([]);
                  setSelected(new Set());
                  setOverrides({});
                  setExportErrors({});
                  setGlobalError(null);
                }}
              >
                <TrashIcon />
                Clear
              </Button>
            )}
            {selected.size > 0 && (
              <Button
                variant="secondary"
                disabled={busy}
                onClick={() => void exportManyBundles([...selected])}
              >
                Export selected bundles ({selected.size})
              </Button>
            )}
            <Button
              disabled={!validItems.length || busy}
              onClick={() =>
                void exportManyBundles(validItems.map((item) => item.path))
              }
            >
              {busy ? "Working…" : "Export all bundles"}
            </Button>
          </div>
          {globalError && (
            <div
              role="alert"
              className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
            >
              {globalError}
            </div>
          )}
          <FileList
            items={items}
            busy={busy}
            exportErrors={exportErrors}
            onBundle={(item) => void exportOneBundle(item)}
            onVocals={(item) => void exportVocals(item)}
            selected={selected}
            onToggleSelect={toggleSelect}
            onToggleVocal={toggleVocalExport}
          />
        </>
      )}
    </div>
  );
}
