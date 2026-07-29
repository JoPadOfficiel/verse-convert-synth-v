/**
 * The diagnostic fields display grouping reads. Structural on purpose, so this
 * module stays free of the Tauri DTO surface and can be tested without a
 * webview; `Diagnostic` from `@/lib/tauri` satisfies it.
 */
export type DisplayDiagnostic = {
  code: string;
  severity: "info" | "warning";
  message: string;
};

export type GroupedDiagnostic = DisplayDiagnostic & {
  /** How many source items raised this exact diagnostic. Never below 1. */
  count: number;
};

/**
 * Collapses diagnostics that state the same thing into one row with a count.
 *
 * A per-note diagnostic names the note it belongs to, so a word the target's
 * application reinterprets on 300 notes arrives as 300 diagnostics that differ
 * only by source ID. The backend must keep them apart - each one is evidence
 * about one note, and the audit contract owns that - but repeating one identical
 * sentence 300 times would bury every other diagnostic on the Part. This is
 * presentation only: nothing is dropped, and the count states how many source
 * items are affected.
 *
 * First-appearance order is preserved, so the displayed order stays the
 * backend's deterministic order.
 */
export function groupDiagnostics(
  warnings: readonly DisplayDiagnostic[],
): GroupedDiagnostic[] {
  const grouped: GroupedDiagnostic[] = [];
  const seen = new Map<string, GroupedDiagnostic>();
  for (const warning of warnings) {
    // The message is part of the identity: one code can carry several distinct
    // messages, and two different sentences must never be merged into one row.
    // Encoding the three fields removes any question of a separator appearing
    // inside a message.
    const key = JSON.stringify([
      warning.severity,
      warning.code,
      warning.message,
    ]);
    const existing = seen.get(key);
    if (existing) {
      existing.count += 1;
      continue;
    }
    const entry: GroupedDiagnostic = {
      code: warning.code,
      severity: warning.severity,
      message: warning.message,
      count: 1,
    };
    seen.set(key, entry);
    grouped.push(entry);
  }
  return grouped;
}
