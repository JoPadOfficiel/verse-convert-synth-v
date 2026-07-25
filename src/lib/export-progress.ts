import type {
  BundleProgressEvent,
  BundleProgressPhase,
} from "@/lib/tauri";

export type FileExportProgress = Omit<BundleProgressEvent, "phase"> & {
  phase: BundleProgressPhase | "queued" | "failed";
};

export function queuedExportProgress(): FileExportProgress {
  return {
    phase: "queued",
    completed: 0,
    total: 1,
    message: "Waiting for the previous title",
    stemId: null,
    stemName: null,
  };
}

export function failedExportProgress(
  previous?: FileExportProgress,
): FileExportProgress {
  return {
    phase: "failed",
    completed: previous?.completed ?? 0,
    total: previous?.total ?? 1,
    message: "Complete project failed",
    stemId: previous?.stemId ?? null,
    stemName: previous?.stemName ?? null,
  };
}

export function exportProgressPercent(progress: FileExportProgress): number {
  if (progress.phase === "finished") return 100;
  if (!Number.isFinite(progress.total) || progress.total <= 0) return 0;
  const completed = Number.isFinite(progress.completed)
    ? progress.completed
    : 0;
  return Math.max(0, Math.min(100, (completed / progress.total) * 100));
}
