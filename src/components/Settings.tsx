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
  type ExportTarget,
  type PronunciationProfile,
} from "@/lib/tauri";

export function Settings({
  outDir,
  setOutDir,
  rendererPath,
  setRendererPath,
  rendererStatus,
  exportTarget,
  pronunciationProfile,
  onPronunciationChange,
  busy,
  error,
  onClose,
}: {
  outDir?: string;
  setOutDir: (directory?: string) => void;
  rendererPath?: string;
  setRendererPath: (path?: string) => void;
  rendererStatus: RendererStatus | null;
  exportTarget: ExportTarget;
  pronunciationProfile: PronunciationProfile;
  onPronunciationChange: (profile: PronunciationProfile) => void;
  busy: boolean;
  error: string | null;
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
          "MuseScore Studio 3.6.2 or 4 with score-parts support is required for complete bundles.";

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="icon" onClick={onClose} title="Back">
          <ChevronLeftIcon />
        </Button>
        <h2 className="text-lg font-semibold">Settings</h2>
      </div>

      <div className="flex flex-col gap-2">
        <Label htmlFor="pronunciation-profile">Pronunciation</Label>
        <select
          id="pronunciation-profile"
          value={pronunciationProfile}
          disabled={busy}
          onChange={(event) => onPronunciationChange(event.target.value as PronunciationProfile)}
          className="rounded-md border bg-background px-3 py-2 text-sm disabled:opacity-50"
        >
          <option value="automatic">Automatic French + English + Spanish + Portuguese</option>
          <option value="default">Default — no pronunciation fixes</option>
          <option value="frenchMillefeuille" disabled={exportTarget !== "ustx"}>
            French DiffSinger Millefeuille
          </option>
          <option value="englishArpabet" disabled={exportTarget !== "ustx"}>
            English DiffSinger ARPAbet
          </option>
          <option value="spanishDiffSinger" disabled={exportTarget !== "ustx"}>
            Spanish DiffSinger
          </option>
          <option value="portugueseDiffSinger" disabled={exportTarget !== "ustx"}>
            Portuguese DiffSinger (Portugal / Brazil)
          </option>
        </select>
        <p className="text-xs text-muted-foreground">
          {pronunciationProfile === "automatic"
            ? exportTarget === "ustx"
              ? "Automatic FR+EN+ES+PT detects complete sung words in context, including songs in a single language. French uses Millefeuille, English uses ARPAbet, and Spanish and Portuguese use their native DiffSinger phonemizers. Use OpenUtau 0.1.569 or newer and assign a compatible multilingual singer; Portuguese dialect depends on the singer and its pronunciation dictionary."
              : "Automatic FR+EN+ES+PT runs fully offline and classifies complete sung words in context, including songs in a single language. Synthesizer V keeps Verse's existing lyric and phoneme serialization; no OpenUtau aliases or phonemizer metadata are injected."
            : pronunciationProfile === "spanishDiffSinger"
              ? "Spanish words use OpenUtau's native DiffSinger Spanish phonemizer. Assign a compatible Spanish DiffSinger singer after export; incomplete source words appear in diagnostics."
            : pronunciationProfile === "portugueseDiffSinger"
              ? "Portuguese lyrics from Portugal and Brazil use OpenUtau's native DiffSinger Portuguese phonemizer. Assign a compatible singer; regional pronunciation depends on its voicebank and dictionary, not automatic dialect detection."
            : pronunciationProfile === "englishArpabet"
            ? "English pronunciation for compatible DiffSinger banks, including UFR. Words or syllable layouts needing review appear in diagnostics."
            : pronunciationProfile === "frenchMillefeuille"
              ? "French pronunciation for compatible DiffSinger Millefeuille banks, including UFR. Words or syllable layouts needing review appear in diagnostics."
              : exportTarget === "ustx"
                ? "Default does not apply Verse's language routing or pronunciation preparation. Assigning a singer later in OpenUtau does not apply Verse's corrections."
                : "Default keeps Synthesizer V's existing lyric and phoneme serialization without automatic language routing."}
          {" "}Your choice is remembered even before importing a file; with files
          loaded, it is saved after reanalysis succeeds.
          {pronunciationProfile === "automatic" && (
            <> Automatic FR+EN+ES+PT uses only bundled local detector data and dictionaries;
            no network lookup or separate installation is required.</>
          )}
        </p>
        <a className="text-xs underline" href="/licenses/french-community-dictionary.txt" target="_blank" rel="noreferrer">
          French community dictionary license
        </a>
        <a className="text-xs underline" href="/licenses/cmudict.txt" target="_blank" rel="noreferrer">
          CMU English dictionary license
        </a>
        <a className="text-xs underline" href="/licenses/spanish-community-dictionary.txt" target="_blank" rel="noreferrer">
          Spanish community dictionary license
        </a>
        <a className="text-xs underline" href="/licenses/portuguese-brazil-community-dictionary.txt" target="_blank" rel="noreferrer">
          Portuguese (Brazil) community dictionary license
        </a>
        <a className="text-xs underline" href="/licenses/portuguese-portugal-community-dictionary.txt" target="_blank" rel="noreferrer">
          Portuguese (Portugal) community dictionary license
        </a>
        <a className="text-xs underline" href="/licenses/mfa-pronunciation-dictionaries-cc-by-4.0.txt" target="_blank" rel="noreferrer">
          MFA Spanish and Portuguese pronunciation dictionaries license (CC BY 4.0)
        </a>
        <a className="text-xs underline" href="/licenses/mfa-pronunciation-attribution.txt" target="_blank" rel="noreferrer">
          MFA pronunciation dictionaries attribution and adaptation notice
        </a>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
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
        <Label htmlFor="renderer-path">
          MuseScore Studio 3.6.2 / 4 renderer
        </Label>
        <div className="flex items-center gap-2">
          <input
            id="renderer-path"
            value={rendererPath ?? ""}
            onChange={(event) => setRendererPath(event.target.value)}
            placeholder="Auto-detect MuseScore Studio 3.6.2 or 4"
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
          blocked when a compatible MuseScore 3.6.2 or 4 renderer cannot be
          detected and its score-parts capability validated; Verse never
          creates fake audio.
        </p>
      </div>

      <div className="rounded-md border bg-muted/40 p-3 text-xs text-muted-foreground">
        A complete <code>.versebundle</code> contains the byte-identical source,
        an auditable manifest, editable vocal notes, one real WAV stem per
        source Part and a muted WAV of the original full score. Assign a voice
        database in Synthesizer V to every vocal track. Vocal-reference stems
        start muted; accompaniment stems start active.
      </div>
    </div>
  );
}
