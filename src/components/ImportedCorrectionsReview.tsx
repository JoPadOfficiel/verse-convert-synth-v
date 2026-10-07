import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { UploadIcon } from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { commandErrorMessage, comparePronunciation, confirmPronunciation, exchangePronunciation, getPronunciationMemory, pickCorrectedProject, pickCorrectionExchange, setCorrectionStatus, type CorrectionsReview, type PronunciationMemory } from "@/lib/tauri";

export function ImportedCorrectionsReview({ onClose, onReanalyse, onBusyChange, hasLoadedSongs }: { onClose: () => void; onReanalyse: () => Promise<void>; onBusyChange: (busy: boolean) => void; hasLoadedSongs: boolean }) {
  const [memory, setMemory] = useState<PronunciationMemory>();
  const [reference, setReference] = useState("");
  const [correctedPath, setCorrectedPath] = useState("");
  const [review, setReview] = useState<CorrectionsReview>();
  const [selected, setSelected] = useState<Record<string, "occurrence_only" | "compatible_context">>({});
  const [listened, setListened] = useState(false);
  const [importListened, setImportListened] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string>();
  const refresh = async () => setMemory(await getPronunciationMemory());
  useEffect(() => { let current = true; void getPronunciationMemory().then((value) => { if (current) setMemory(value); }).catch((e) => { if (current) setError(commandErrorMessage(e)); }); return () => { current = false; }; }, []);
  const run = async (action: () => Promise<void>) => { if (busyRef.current) return; busyRef.current = true; setBusy(true); onBusyChange(true); setError(undefined); try { await action(); } catch (e) { setError(commandErrorMessage(e)); } finally { busyRef.current = false; setBusy(false); onBusyChange(false); } };
  const resetReview = useCallback(() => { setReview(undefined); setSelected({}); setListened(false); }, []);
  const loadCorrectedPaths = useCallback((paths: string[]) => {
    if (paths.length !== 1 || !/\.ustx$/i.test(paths[0])) {
      setError("Choose one edited OpenUtau .ustx project. Scores and corrections JSON use separate import actions.");
      return;
    }
    resetReview();
    setCorrectedPath(paths[0]);
    setError(undefined);
  }, [resetReview]);
  useEffect(() => {
    let current = true;
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      if (!current) return;
      const type = event.payload.type;
      if (type === "leave" || type === "drop") setDragging(false);
      if (busyRef.current) return;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "drop") loadCorrectedPaths(event.payload.paths);
    });
    void unlisten.catch((e) => { if (current) setError(commandErrorMessage(e)); });
    return () => { current = false; void unlisten.then((dispose) => dispose()).catch(() => {}); };
  }, [loadCorrectedPaths]);
  return <section aria-label="Pronunciation correction memory" className="min-h-0 overflow-auto rounded-lg border p-4">
    <div className="flex items-center justify-between gap-3"><h2 className="text-lg font-semibold">Pronunciation corrections</h2><Button variant="outline" disabled={busy} onClick={onClose}>Close</Button></div>
    <p className="mt-2 text-sm text-muted-foreground">Assign voices, listen and edit in OpenUtau. Compare a corrected copy here, then confirm only the corrections you heard. Pitch, timing and mix edits are excluded.</p>
    {memory && <p className="mt-2 text-xs text-muted-foreground">Baseline: {memory.baseline}.</p>}
    {error && <p role="alert" className="mt-3 text-sm text-destructive">{error}</p>}
    <Button className="mt-3" variant="outline" disabled={busy || !hasLoadedSongs} onClick={() => void run(onReanalyse)}>Reanalyse loaded songs</Button>
    <div className="mt-4">
      <h3 className="font-medium">Edited OpenUtau project</h3>
      <button type="button" disabled={busy} onClick={() => void run(async () => { const path = await pickCorrectedProject(); if (path) loadCorrectedPaths([path]); })}
        className={"mt-2 flex w-full flex-col items-center gap-2 rounded-xl border-2 border-dashed bg-card px-4 py-6 text-center transition-colors disabled:cursor-not-allowed disabled:opacity-50 " + (dragging ? "border-ring bg-accent" : "border-input hover:border-ring")}>
        <UploadIcon className="size-6 text-muted-foreground" aria-hidden="true" />
        <span className="font-medium">Drop an edited .ustx file here, or click to browse</span>
        <span className="text-sm text-muted-foreground">Choose the copy saved in OpenUtau after your edits.</span>
      </button>
      {correctedPath && <p role="status" className="mt-2 break-all text-sm">Selected project: {correctedPath}</p>}
    </div>
    <label className="mt-4 block text-sm">Original export reference<select aria-label="Original export reference" className="mt-1 block w-full rounded border bg-background p-2" value={reference} disabled={busy} onChange={(e) => { setReference(e.target.value); resetReview(); }}><option value="">Select an immutable saved export</option>{memory?.references.map((item) => <option key={item.id} value={item.id}>{item.sourceLabel ?? "Unknown source"} · {item.exportLabel ?? "Unknown export"} · {item.createdAtUnixSeconds ? new Date(item.createdAtUnixSeconds * 1000).toLocaleString() : "Creation time unknown"} · {item.exportSha256.slice(0, 12)}</option>)}</select></label>
    <p className="mt-2 text-xs text-muted-foreground">{memory?.references.length === 0 ? "Export an OpenUtau project from Verse first to create an original reference, then compare your edited copy here." : "Load your edited file and select the Verse export it came from, then compare. Only corrections you confirm after listening are saved to memory."}</p>
    <div className="mt-3 flex flex-wrap gap-2"><Button disabled={busy || !reference || !correctedPath} onClick={() => void run(async () => { resetReview(); setReview(await comparePronunciation(reference, correctedPath)); })}>Compare corrected USTX copy</Button><Button variant="outline" disabled={busy} onClick={() => void run(async () => { const path = await pickCorrectionExchange(true); if (path) { await exchangePronunciation(path, true); await refresh(); } })}>Import corrections JSON</Button><Button variant="outline" disabled={busy} onClick={() => void run(async () => { const path = await pickCorrectionExchange(false); if (path) await exchangePronunciation(path, false); })}>Export confirmed corrections JSON</Button></div>
    {review && <div className="mt-4">
      <p className="text-sm">{review.proposals.length} pronunciation changes available for review.</p>
      {review.diagnostics.length > 0 && <details className="mt-2 text-sm"><summary>Refused associations ({review.diagnostics.length})</summary><ul>{review.diagnostics.map((d, i) => <li key={i}>{d.code}: {d.message}</li>)}</ul></details>}
      <div className="mt-3 space-y-3">{review.proposals.map((c) => <div key={c.id} className="rounded border p-3 text-sm">
        <p>Before: {c.before.language ?? "unknown"} · {c.before.phonemizer} · {c.before.lexical_reading ?? "unknown"} · phones {c.before.phones?.join(" ") ?? "unknown"}</p>
        <p>After: {c.after.language ?? "unknown"} · {c.after.phonemizer} · {c.after.lexical_reading ?? "unknown"} · phones {c.after.phones?.join(" ") ?? "unknown"}</p>
        <p>Complete occurrence: track {c.word.owner.track}, Part {c.word.owner.part ?? "unknown"}, staff {c.word.owner.staff ?? "unknown"}, voice {c.word.owner.voice ?? "unknown"}, row {c.word.owner.lane}, verse {c.word.owner.verse}, occurrence {c.word.owner.occurrence}, segment {c.word.owner.segment ?? "unknown"}; members {c.word.members.join(", ")}; fragments {c.word.original.join(" | ")}</p>
        <label className="flex gap-2"><input type="checkbox" disabled={busy} checked={c.id in selected} onChange={(e) => setSelected((old) => { const next = { ...old }; if (e.target.checked) next[c.id] = "occurrence_only"; else delete next[c.id]; return next; })} /><span><strong>{c.word.key}</strong> · {c.word.context.join(" ")}<br />Part {c.word.owner.part ?? "unknown"}, staff {c.word.owner.staff ?? "unknown"}, verse {c.word.owner.verse}<br />{c.before.language ?? "unknown"} → {c.after.language ?? "unknown"}<br />{c.after.phonemizer}<br />{c.after.phones?.join(" ") ?? "No changed phone sequence"}<br />Native aliases: {c.after.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join("; ") ?? "none"}</span></label>
        <p className="mt-1 text-xs text-muted-foreground">Observed singer: {c.provenance.observed_singer ?? "Not recorded"}. This identity does not qualify inventory or reuse.</p>
        <label className="mt-2 block">Reuse scope<select aria-label={`Reuse scope for ${c.word.key}`} className="ml-2 rounded border bg-background p-1" disabled={busy || !(c.id in selected)} value={selected[c.id] ?? "occurrence_only"} onChange={(e) => setSelected((old) => ({ ...old, [c.id]: e.target.value as "occurrence_only" | "compatible_context" }))}><option value="occurrence_only">This source occurrence only</option><option value="compatible_context" disabled={Boolean((c.after.phones || c.after.aliases || c.accepted_variant) && !c.after.authority)}>Compatible context across songs</option></select></label>
        {c.after.phones && !c.after.authority && <p className="mt-1 text-xs text-muted-foreground">Phone reuse requires qualified singer inventories. This record is retained for review; unqualified phones are not applied automatically.</p>}
      </div>)}</div>
      <label className="mt-3 flex gap-2 text-sm"><input type="checkbox" disabled={busy} checked={listened} onChange={(e) => setListened(e.target.checked)} />I listened with the assigned voices and confirm the selected corrections.</label>
      <Button className="mt-3" disabled={busy || !listened || !Object.keys(selected).length} onClick={() => void run(async () => { await confirmPronunciation(reference, correctedPath, Object.entries(selected).map(([correction_id, scope]) => ({ correction_id, scope, listened }))); resetReview(); await refresh(); })}>Save confirmed corrections</Button>
    </div>}
    <h3 className="mt-5 font-medium">Memory history</h3><p className="text-xs text-muted-foreground">References survive restarts and updates. Removed corrections remain in history and are excluded from reuse and contribution exports. Imported records await local confirmation. Reanalyse loaded songs after memory changes.</p>
    <ul className="mt-2 space-y-2">{memory?.history.map(({ correction: c, status }) => <li key={c.id} className="rounded border p-3 text-sm">
      <p><strong>{c.word.key}</strong> · {c.scope} · {status} · symbols: {c.provenance.symbol_validation}</p>
      <p>Context: {c.word.context.map((word, i) => i === c.word.context_target ? `[${word}]` : word).join(" ")}</p>
      <p>Owner: Part {c.word.owner.part ?? "unknown"}, staff {c.word.owner.staff ?? "unknown"}, voice {c.word.owner.voice ?? "unknown"}, row {c.word.owner.lane}, verse {c.word.owner.verse}, occurrence {c.word.owner.occurrence}, segment {c.word.owner.segment ?? "unknown"}.</p>
      <p>Before: {c.before.language ?? "unknown"} · {c.before.phonemizer} · reading {c.before.lexical_reading ?? "unknown"} · phones {c.before.phones?.join(" ") ?? "unknown"} · alphabet {c.before.alphabet ?? "unknown"} · authority {c.before.authority ?? "unqualified"}</p>
      <p>After: {c.after.language ?? "unknown"} · {c.after.phonemizer} · reading {c.after.lexical_reading ?? "unknown"} · phones {c.after.phones?.join(" ") ?? "unknown"} · alphabet {c.after.alphabet ?? "unknown"} · authority {c.after.authority ?? "unqualified"} · aliases {c.after.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join("; ") ?? "none"}</p>
      <p>Observed singer: {c.provenance.observed_singer ?? "Not recorded"}. This identity does not qualify inventory or reuse.</p>
      <p>Qualified voice constraint: {c.voice ? `${c.voice.singer}; inventory ${c.voice.inventory_sha256}; configuration ${c.voice.configuration_sha256}` : "Unknown; no qualified voice constraint"}</p>
      <p className="text-xs text-muted-foreground">Source {c.provenance.source_sha256.slice(0, 16)} · export {c.provenance.export_sha256.slice(0, 16)} · corrected project {c.provenance.corrected_sha256.slice(0, 16)}. {c.after.phones && !c.after.authority && "Unqualified phones remain excluded from automatic reuse."}</p>
      {status === "pending" && <div className="mt-2"><label className="flex gap-2"><input type="checkbox" disabled={busy} checked={Boolean(importListened[c.id])} onChange={(e) => setImportListened((old) => ({ ...old, [c.id]: e.target.checked }))} />I reviewed this word, context and voice scope, listened with the assigned voices, and confirm this imported correction.</label><Button className="mt-2" variant="outline" disabled={busy || !importListened[c.id]} onClick={() => void run(async () => { await setCorrectionStatus(c.id, "active", Boolean(importListened[c.id])); await refresh(); })}>Activate reviewed correction</Button></div>}
      {status !== "revoked" && <Button className="mt-2" variant="outline" disabled={busy} onClick={() => void run(async () => { await setCorrectionStatus(c.id, "revoked"); await refresh(); })}>Remove from reuse</Button>}
    </li>)}</ul>
  </section>;
}
