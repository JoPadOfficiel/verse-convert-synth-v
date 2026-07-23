import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export type TrackInfo = { id: number; track: string; notes: number; role: string; placed: number };

/** Sings/Muted overrides: file path -> (track id -> sings?) */
export type Overrides = Record<string, Record<number, boolean>>;
export type FileResult = {
  path: string;
  name: string;
  ok: boolean;
  msg: string | null;
  nTracks: number;
  placed: number;
  tracks: TrackInfo[];
  out: string | null;
};

const RE = /\.(kar|mid|midi|mxl|xml|musicxml|mscz)$/i;
export const isSupported = (p: string) => RE.test(p);

export async function pickFiles(): Promise<string[]> {
  const res = await open({
    multiple: true,
    filters: [
      { name: "Karaoke / MIDI / Score", extensions: ["kar", "mid", "midi", "mxl", "xml", "musicxml", "mscz"] },
    ],
  });
  if (!res) return [];
  return Array.isArray(res) ? res : [res];
}

export async function pickDirectory(): Promise<string | undefined> {
  const res = await open({ directory: true, multiple: false });
  return typeof res === "string" ? res : undefined;
}

export type Language = "english" | "french";

export async function convertFiles(
  paths: string[],
  write: boolean,
  language: Language = "english",
  outDir?: string,
  overrides?: Overrides
): Promise<FileResult[]> {
  return await invoke<FileResult[]>("convert_files", {
    paths,
    write,
    outDir: outDir ?? null,
    language,
    overrides: overrides ?? null,
  });
}
