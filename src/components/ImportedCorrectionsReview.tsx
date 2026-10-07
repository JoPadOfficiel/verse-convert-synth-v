import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { UploadIcon } from "@radix-ui/react-icons";
import { Button } from "@/components/ui/button";
import { commandErrorMessage, comparePronunciation, confirmPronunciation, exchangePronunciation, getPronunciationMemory, pickCorrectedProject, pickCorrectionExchange, setCorrectionStatus, type CorrectionsReview, type PronunciationMemory } from "@/lib/tauri";

const LANGUAGE_NAMES: Record<string, string> = { fr: "French", en: "English", es: "Spanish", pt: "Portuguese" };
const languageName = (language: string | null) => language ? LANGUAGE_NAMES[language] ?? language : "Unknown language";

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
  const [notice, setNotice] = useState<{ kind: "saved" | "exported" | "imported"; message: string }>();
  const [showExchangeImport, setShowExchangeImport] = useState(false);
  const confirmedCount = memory?.history.filter(({ correction, status }) => status === "active" && correction.provenance.confirmed_after_listening).length ?? 0;
  const refresh = async () => { try { setMemory(await getPronunciationMemory()); } catch (e) { throw new Error(`Could not reload saved corrections. Click Refresh saved corrections to retry. ${commandErrorMessage(e)}`); } };
  useEffect(() => { let current = true; void getPronunciationMemory().then((value) => { if (current) setMemory(value); }).catch((e) => { if (current) setError(commandErrorMessage(e)); }); return () => { current = false; }; }, []);
  const run = async (action: () => Promise<void>) => { if (busyRef.current) return; busyRef.current = true; setBusy(true); onBusyChange(true); setError(undefined); setNotice(undefined); try { await action(); } catch (e) { setError(commandErrorMessage(e)); } finally { busyRef.current = false; setBusy(false); onBusyChange(false); } };
  const resetReview = useCallback(() => { setReview(undefined); setSelected({}); setListened(false); }, []);
  const loadCorrectedPaths = useCallback((paths: string[]) => {
    if (paths.length !== 1 || !/\.ustx$/i.test(paths[0])) {
      setError("Choose one edited OpenUtau .ustx project. Scores and corrections JSON use separate import actions.");
      return;
    }
    resetReview();
    setNotice(undefined);
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
    <p className="mt-2 text-sm text-muted-foreground">Follow these steps to save pronunciation corrections from a project you edited in OpenUtau.</p>
    <p className="mt-2 text-sm text-muted-foreground">Your edited music project is a <strong>.ustx</strong> file. The <strong>.json</strong> file is exported at the end to share confirmed corrections with the developer.</p>
    {error && <p role="alert" className="sticky top-0 z-10 mt-3 rounded border bg-background p-3 text-sm text-destructive">{error}</p>}
    <ol aria-label="Correction steps" className="mt-5 space-y-5">
    <li className="rounded-lg border p-4">
      <h3 className="font-medium">1. Load your edited OpenUtau file (.ustx)</h3>
      <p className="mt-1 text-sm text-muted-foreground">In OpenUtau, save the project after editing and listening with your assigned voices. Choose that saved copy below.</p>
      <p className="mt-2 text-sm text-muted-foreground">For this comparison, keep the notes, pitches, timing and held syllables unchanged. Use a separate copy for musical edits.</p>
      <button type="button" disabled={busy} onClick={() => void run(async () => { const path = await pickCorrectedProject(); if (path) loadCorrectedPaths([path]); })}
        className={"mt-2 flex w-full flex-col items-center gap-2 rounded-xl border-2 border-dashed bg-card px-4 py-6 text-center transition-colors disabled:cursor-not-allowed disabled:opacity-50 " + (dragging ? "border-ring bg-accent" : "border-input hover:border-ring")}>
        <UploadIcon className="size-6 text-muted-foreground" aria-hidden="true" />
        <span className="font-medium">Drop an edited .ustx file here, or click to browse</span>
        <span className="text-sm text-muted-foreground">Choose the copy saved in OpenUtau after your edits.</span>
      </button>
      {correctedPath && <p role="status" className="mt-2 break-all text-sm">Selected project: {correctedPath}</p>}
    </li>
    <li className="rounded-lg border p-4">
      <h3 className="font-medium">2. Choose the original Verse export</h3>
      <p className="mt-1 text-sm text-muted-foreground">Select the export you opened in OpenUtau before making your changes. Use its song name and export date to identify it.</p>
      <label className="mt-3 block text-sm">Original Verse export<select aria-label="Original export reference" className="mt-1 block w-full rounded border bg-background p-2" value={reference} disabled={busy} onChange={(e) => { setReference(e.target.value); resetReview(); setNotice(undefined); }}><option value="">Choose the original export</option>{memory?.references.map((item) => <option key={item.id} value={item.id}>{item.sourceLabel ?? "Unknown source"} · {item.exportLabel ?? "Unknown export"} · {item.createdAtUnixSeconds ? new Date(item.createdAtUnixSeconds * 1000).toLocaleString() : "Creation time unknown"} · {item.exportSha256.slice(0, 12)}</option>)}</select></label>
      {memory?.references.length === 0 && <p className="mt-2 text-sm text-muted-foreground">No original export is saved yet. Convert your score in Verse and export an OpenUtau project first. You can then edit that project in OpenUtau and return here.</p>}
    </li>
    <li className="rounded-lg border p-4">
      <h3 className="font-medium">3. Compare the two files</h3>
      <p className="mt-1 text-sm text-muted-foreground">Verse checks pronunciation changes against the original export. Notes, timing and mix edits are excluded from correction memory.</p>
      <Button className="mt-3" disabled={busy || !reference || !correctedPath} onClick={() => void run(async () => { resetReview(); setReview(await comparePronunciation(reference, correctedPath)); })}>Compare files</Button>
      {(!reference || !correctedPath) && <p className="mt-2 text-sm text-muted-foreground">Complete steps 1 and 2 to compare.</p>}
    </li>
    <li className="rounded-lg border p-4">
      <h3 className="font-medium">4. Confirm and save the corrections you heard</h3>
      <p className="mt-1 text-sm text-muted-foreground">Check the words you corrected, confirm that you listened in OpenUtau, then save them to Verse memory.</p>
      {!review && <p className="mt-3 text-sm text-muted-foreground">Compare the files in step 3 to see the changes here.</p>}
      {review && <div className="mt-4">
      <p className="text-sm">{review.proposals.length} pronunciation changes available for review.</p>
      {review.proposals.length === 0 && <p className="mt-2 text-sm text-muted-foreground">No pronunciation correction is available to save. If you expected changes, check that you chose the edited copy and its matching original export.</p>}
      {review.diagnostics.length > 0 && <details className="mt-2 text-sm"><summary>Refused associations ({review.diagnostics.length})</summary><ul>{review.diagnostics.map((d, i) => <li key={i}>{d.code}: {d.message}</li>)}</ul></details>}
      <div className="mt-3 space-y-3">{review.proposals.map((c) => <div key={c.id} className="rounded border p-3 text-sm">
        <details className="mb-2"><summary className="cursor-pointer">Technical details for {c.word.key}</summary>
        <p>Before: {c.before.language ?? "unknown"} · {c.before.phonemizer} · {c.before.lexical_reading ?? "unknown"} · phones {c.before.phones?.join(" ") ?? "unknown"}</p>
        <p>After: {c.after.language ?? "unknown"} · {c.after.phonemizer} · {c.after.lexical_reading ?? "unknown"} · phones {c.after.phones?.join(" ") ?? "unknown"}</p>
        <p>Complete occurrence: track {c.word.owner.track}, Part {c.word.owner.part ?? "unknown"}, staff {c.word.owner.staff ?? "unknown"}, voice {c.word.owner.voice ?? "unknown"}, row {c.word.owner.lane}, verse {c.word.owner.verse}, occurrence {c.word.owner.occurrence}, segment {c.word.owner.segment ?? "unknown"}; members {c.word.members.join(", ")}; fragments {c.word.original.join(" | ")}</p>
        <p>Observed singer: {c.provenance.observed_singer ?? "Not recorded"}. This identity does not qualify inventory or reuse.</p>
        <p>Native aliases: {c.after.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join("; ") ?? "none"}</p>
        <p>Native aliases before: {c.before.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join("; ") ?? "none"}</p>
        </details>
        <label className="flex gap-2"><input type="checkbox" disabled={busy} checked={c.id in selected} onChange={(e) => setSelected((old) => { const next = { ...old }; if (e.target.checked) next[c.id] = "occurrence_only"; else delete next[c.id]; return next; })} /><span className="min-w-0 break-words"><strong>{c.word.key}</strong> · {c.word.context.join(" ")}<br />Source part: {c.word.owner.part ?? "Unknown"}<br />Before: {languageName(c.before.language)} · {c.before.lexical_reading ?? c.word.key} · {c.before.phones?.join(" ") ?? "No phone sequence recorded"}<br />After: {languageName(c.after.language)} · {c.after.lexical_reading ?? c.word.key} · {c.after.phones?.join(" ") ?? "No phone sequence recorded"}
        {c.before.phonemizer !== c.after.phonemizer && <><br />Method before: {c.before.phonemizer}<br />Method after: {c.after.phonemizer}</>}
        {JSON.stringify(c.before.aliases ?? []) !== JSON.stringify(c.after.aliases ?? []) && <><br />Overrides before: {c.before.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join(" · ") || "None"}<br />Overrides after: {c.after.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join(" · ") || "None"}</>}
        </span></label>
        <details className="mt-2"><summary className="cursor-pointer">Where to reuse this correction</summary>
        <label className="mt-2 block">Reuse scope<select aria-label={`Reuse scope for ${c.word.key}`} className="ml-2 rounded border bg-background p-1" disabled={busy || !(c.id in selected)} value={selected[c.id] ?? "occurrence_only"} onChange={(e) => setSelected((old) => ({ ...old, [c.id]: e.target.value as "occurrence_only" | "compatible_context" }))}><option value="occurrence_only">This source occurrence only</option><option value="compatible_context" disabled={Boolean((c.after.phones || c.after.aliases || c.accepted_variant) && !c.after.authority)}>Compatible context across songs</option></select></label>
        {c.after.phones && !c.after.authority && <p className="mt-1 text-xs text-muted-foreground">Phone reuse requires qualified singer inventories. This record is retained for review; unqualified phones are not applied automatically.</p>}
        </details>
      </div>)}</div>
      {review.proposals.length > 0 && <><label className="mt-3 flex gap-2 text-sm"><input type="checkbox" disabled={busy} checked={listened} onChange={(e) => setListened(e.target.checked)} />I listened with the assigned voices and confirm the selected corrections.</label>
      <Button className="mt-3" disabled={busy || !listened || !Object.keys(selected).length} onClick={() => void run(async () => { await confirmPronunciation(reference, correctedPath, Object.entries(selected).map(([correction_id, scope]) => ({ correction_id, scope, listened }))); resetReview(); setNotice({ kind: "saved", message: "Corrections saved to Verse memory." }); await refresh(); })}>Save confirmed corrections</Button></>}
    </div>}
      {notice?.kind === "saved" && <p role="status" className="mt-3 rounded border bg-accent p-3 text-sm">{notice.message}</p>}
    </li>
    <li className="rounded-lg border p-4">
      <h3 className="font-medium">5. Send corrections to the developer (.json, optional)</h3>
      <p className="mt-1 text-sm text-muted-foreground">Export your confirmed pronunciation corrections, including their word context and source details. Send this JSON to the developer to help improve Verse.</p>
      <p className="mt-2 text-sm text-muted-foreground">The JSON contains correction records, not the complete music project or its audio. If the developer needs to inspect the full project, also send the edited .ustx, the matching original .ustx exported by Verse, and the original score or MIDI.</p>
      <Button className="mt-3" variant="outline" disabled={busy || confirmedCount === 0} onClick={() => void run(async () => { const path = await pickCorrectionExchange(false); if (path) { await exchangePronunciation(path, false); setNotice({ kind: "exported", message: `JSON saved: ${path}. Send this file to the developer.` }); } })}>Export JSON for the developer</Button>
      {notice?.kind === "exported" && <p role="status" className="mt-3 break-all rounded border bg-accent p-3 text-sm">{notice.message}</p>}
      <p className="mt-2 text-sm text-muted-foreground">{confirmedCount === 0 ? "Save at least one correction in step 4 before exporting." : `${confirmedCount} confirmed correction${confirmedCount === 1 ? "" : "s"} in memory. The JSON includes compatible confirmed corrections from all songs.`}</p>
    </li>
    </ol>
    <details className="mt-5 rounded-lg border p-4" open={showExchangeImport} onToggle={(event) => setShowExchangeImport(event.currentTarget.open)}>
      <summary className="cursor-pointer font-medium">Have a corrections JSON from someone else?</summary>
      <p className="mt-2 text-sm text-muted-foreground">Use this only for a Verse corrections .json, not an OpenUtau project. Imported corrections stay pending until you review and confirm them below.</p>
      <Button className="mt-3" variant="outline" disabled={busy} onClick={() => void run(async () => { const path = await pickCorrectionExchange(true); if (path) { await exchangePronunciation(path, true); setNotice({ kind: "imported", message: "JSON imported. Review the pending corrections below and confirm only those you listened to." }); await refresh(); } })}>Import corrections JSON</Button>
      {notice?.kind === "imported" && <p role="status" className="mt-3 rounded border bg-accent p-3 text-sm">{notice.message}</p>}
    </details>
    <h3 className="mt-5 font-medium">Memory history</h3><p className="text-xs text-muted-foreground">References survive restarts and updates. Removed corrections remain in history and are excluded from reuse and contribution exports. Imported records await local confirmation. Reanalyse loaded songs after memory changes.</p>
    <Button className="mt-3" variant="outline" disabled={busy || !hasLoadedSongs} onClick={() => void run(onReanalyse)}>Reanalyse loaded songs</Button>
    <Button className="ml-2 mt-3" variant="outline" disabled={busy} onClick={() => void run(refresh)}>Refresh saved corrections</Button>
    <ul className="mt-2 space-y-2">{memory?.history.map(({ correction: c, status }) => <li key={c.id} className="rounded border p-3 text-sm">
      <p><strong>{c.word.key}</strong> · {c.scope} · {status} · symbols: {c.provenance.symbol_validation}</p>
      <p>Context: {c.word.context.map((word, i) => i === c.word.context_target ? `[${word}]` : word).join(" ")}</p>
      <details className="mt-2"><summary className="cursor-pointer">Correction details</summary>
      <p>Owner: Part {c.word.owner.part ?? "unknown"}, staff {c.word.owner.staff ?? "unknown"}, voice {c.word.owner.voice ?? "unknown"}, row {c.word.owner.lane}, verse {c.word.owner.verse}, occurrence {c.word.owner.occurrence}, segment {c.word.owner.segment ?? "unknown"}.</p>
      <p>Before: {c.before.language ?? "unknown"} · {c.before.phonemizer} · reading {c.before.lexical_reading ?? "unknown"} · phones {c.before.phones?.join(" ") ?? "unknown"} · alphabet {c.before.alphabet ?? "unknown"} · authority {c.before.authority ?? "unqualified"}</p>
      <p>After: {c.after.language ?? "unknown"} · {c.after.phonemizer} · reading {c.after.lexical_reading ?? "unknown"} · phones {c.after.phones?.join(" ") ?? "unknown"} · alphabet {c.after.alphabet ?? "unknown"} · authority {c.after.authority ?? "unqualified"} · aliases {c.after.aliases?.map((a) => `${a.member}:${a.index} ${a.phone}`).join("; ") ?? "none"}</p>
      <p>Observed singer: {c.provenance.observed_singer ?? "Not recorded"}. This identity does not qualify inventory or reuse.</p>
      <p>Qualified voice constraint: {c.voice ? `${c.voice.singer}; inventory ${c.voice.inventory_sha256}; configuration ${c.voice.configuration_sha256}` : "Unknown; no qualified voice constraint"}</p>
      <p className="text-xs text-muted-foreground">Source {c.provenance.source_sha256.slice(0, 16)} · export {c.provenance.export_sha256.slice(0, 16)} · corrected project {c.provenance.corrected_sha256.slice(0, 16)}. {c.after.phones && !c.after.authority && "Unqualified phones remain excluded from automatic reuse."}</p>
      </details>
      {status === "pending" && <div className="mt-2"><label className="flex gap-2"><input type="checkbox" disabled={busy} checked={Boolean(importListened[c.id])} onChange={(e) => setImportListened((old) => ({ ...old, [c.id]: e.target.checked }))} />I reviewed this word, context and voice scope, listened with the assigned voices, and confirm this imported correction.</label><Button className="mt-2" variant="outline" disabled={busy || !importListened[c.id]} onClick={() => void run(async () => { await setCorrectionStatus(c.id, "active", Boolean(importListened[c.id])); await refresh(); })}>Activate reviewed correction</Button></div>}
      {status !== "revoked" && <Button className="mt-2" variant="outline" disabled={busy} onClick={() => void run(async () => { await setCorrectionStatus(c.id, "revoked"); await refresh(); })}>Remove from reuse</Button>}
    </li>)}</ul>
    {memory && <details className="mt-4 text-xs text-muted-foreground"><summary className="cursor-pointer">About correction memory</summary><p className="mt-2">Baseline: {memory.baseline}.</p></details>}
  </section>;
}
