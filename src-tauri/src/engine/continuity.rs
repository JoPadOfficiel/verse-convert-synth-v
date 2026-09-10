//! Source-proven continuity, resolved before linguistic transforms and filtering.
//! Only typed adapter evidence authorizes recovery. Lane geometry allocates an
//! already proven owner; it never establishes lyric ownership.
use super::{report_warning, DiagnosticSeverity, SourceNote, TrackReport};
use crate::engine::midi::{LyricState, NoteSource, SourceExtension, SourceNoteRef};
use crate::engine::projection::{
    ContinuationOwner, MergedTieSource, NoteOrigin, ProjectedLyric, ProjectedNote, ProjectedTrack,
};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "continuity_tests.rs"]
mod tests;

pub const OWNER_AMBIGUOUS: &str = "SOURCE_CONTINUITY_OWNER_AMBIGUOUS";
pub const ROUTE_UNRESOLVED: &str = "SOURCE_CONTINUITY_ROUTE_UNRESOLVED";
pub const LINK_INVALID: &str = "SOURCE_CONTINUITY_LINK_INVALID";

type Domain = (String, String, String, u32);

pub const LIMIT: &str = "SOURCE_CONTINUITY_LIMIT";
const MAX_WORK: usize = 2_000_000;
const MAX_NOTES: usize = 200_000;
#[derive(Default)]
pub(super) struct Budget {
    work: std::cell::Cell<usize>,
    reserved_bytes: std::cell::Cell<usize>,
    diagnostics: usize,
    diagnostic_bytes: usize,
}
impl Budget {
    pub(super) fn charge(&self, count: usize) -> Result<(), String> {
        if self.reserved_bytes.get() > 128 * 1024 * 1024 {
            return Err(format!(
                "{LIMIT}: continuity indexes exceed bounded storage"
            ));
        }
        let total = self.work.get().saturating_add(count);
        self.work.set(total);
        if total > MAX_WORK {
            Err(format!(
                "{LIMIT}: continuity traversal exceeds {MAX_WORK} operations"
            ))
        } else {
            Ok(())
        }
    }
    pub(super) fn reserve(&self, bytes: usize) -> Result<(), String> {
        let total = self.reserved_bytes.get().saturating_add(bytes);
        self.reserved_bytes.set(total);
        if total > 128 * 1024 * 1024 {
            return Err(format!(
                "{LIMIT}: continuity indexes exceed bounded storage"
            ));
        }
        Ok(())
    }
    fn diagnostic(&mut self, bytes: usize) -> Result<(), String> {
        self.diagnostics += 1;
        self.diagnostic_bytes = self.diagnostic_bytes.saturating_add(bytes);
        // Multi-verse scores can legitimately need hundreds of source-scoped
        // warnings. Keep all three hard bounds without refusing ordinary reports.
        if self.diagnostics > 1024 || bytes > 4096 || self.diagnostic_bytes > 512 * 1024 {
            Err(format!(
                "{LIMIT}: continuity diagnostics exceed their bounded count or text size (count {}, total bytes {}, record bytes {bytes})", self.diagnostics, self.diagnostic_bytes
            ))
        } else {
            Ok(())
        }
    }
}

/// Charge serialized evidence size before copying a cumulative tie context.
/// The writer retains no bytes; long chains cannot multiply provenance without
/// consuming the same bounded planner budget used for ownership traversal.
fn charge_intensity_copy(
    intensity: &crate::engine::score_intensity::NoteIntensity,
    budget: &Budget,
) -> Result<(), String> {
    struct Counter<'a>(&'a Budget);
    impl std::io::Write for Counter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.charge(bytes.len()).map_err(std::io::Error::other)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    budget.charge(intensity.source_id.len())?;
    let failure =
        |error| format!("{LIMIT}: tie intensity evidence copy exceeds bounded work: {error}");
    serde_json::to_writer(Counter(budget), &intensity.provenance).map_err(failure)?;
    budget.charge(intensity.issues.len())?;
    for issue in &intensity.issues {
        serde_json::to_writer(Counter(budget), &(&issue.provenance, &issue.message))
            .map_err(failure)?;
    }
    Ok(())
}

type CandidateProjection = (usize, Vec<String>, BTreeSet<u32>);

type RefKey = (String, u32, u32);
fn ref_key(reference: &SourceNoteRef) -> RefKey {
    (
        reference.source_id.clone(),
        reference.occurrence,
        reference.playback_segment,
    )
}
fn source_key(source: &NoteSource) -> RefKey {
    (
        source.id.clone(),
        source.occurrence,
        source.continuity.as_ref().map_or(0, |c| c.playback_segment),
    )
}
fn accepts_row(group: &[String], row: &str) -> bool {
    group.is_empty() || group.iter().any(|candidate| candidate == row)
}
fn source_row<'a>(atom: &'a Atom, rows: &'a BTreeMap<String, String>) -> Option<&'a str> {
    atom.note
        .lyric
        .source_identity()
        .or_else(|| {
            atom.origin()
                .continuation
                .as_ref()
                .map(|c| c.lyric_owner_id.as_str())
        })
        .and_then(|id| rows.get(id))
        .map(String::as_str)
}

fn domain(source: &NoteSource) -> Option<Domain> {
    Some((
        source.part_id.clone()?,
        source.staff_id.clone()?,
        source.voice.clone()?,
        source.continuity.as_ref()?.playback_segment,
    ))
}

fn has_domain(source: &NoteSource) -> bool {
    source.part_id.is_some()
        && source.staff_id.is_some()
        && source.voice.is_some()
        && source.continuity.is_some()
}

fn same_domain(source: &NoteSource, domain: &Domain) -> bool {
    source.part_id.as_deref() == Some(domain.0.as_str())
        && source.staff_id.as_deref() == Some(domain.1.as_str())
        && source.voice.as_deref() == Some(domain.2.as_str())
        && source
            .continuity
            .as_ref()
            .is_some_and(|c| c.playback_segment == domain.3)
}

fn reserve_domain(source: &NoteSource, budget: &Budget) -> Result<(), String> {
    let bytes = [&source.part_id, &source.staff_id, &source.voice]
        .into_iter()
        .flatten()
        .fold(256usize, |total, s| total.saturating_add(s.len()));
    budget.reserve(bytes)?;
    budget.charge(bytes / 8 + 1)
}

fn reserve_source_key(source: &NoteSource, budget: &Budget) -> Result<(), String> {
    budget.reserve(source.id.len().saturating_add(256))?;
    budget.charge(source.id.len() / 8 + 1)
}

struct Atom {
    note: ProjectedNote,
    original_lane: usize,
    destination: usize,
}

impl Atom {
    fn origin(&self) -> &NoteOrigin {
        self.note
            .source_evidence
            .as_ref()
            .unwrap()
            .origin
            .as_ref()
            .unwrap()
    }
    fn id(&self) -> &str {
        &self.note.source_evidence.as_ref().unwrap().note_id
    }
    fn end(&self) -> Option<u32> {
        self.note.onset_ticks.checked_add(self.note.duration_ticks)
    }
    fn sung_head(&self) -> bool {
        !self.origin().lyric_conflict
            && (matches!(&self.note.lyric,
            ProjectedLyric::Source(lyric) if matches!(&lyric.state, LyricState::Text(text) if !text.trim().is_empty()))
                || self.origin().continuation.is_some())
    }
    fn matches_ref(&self, reference: &SourceNoteRef) -> bool {
        let source = &self.origin().source;
        source.id == reference.source_id
            && source.occurrence == reference.occurrence
            && source
                .continuity
                .as_ref()
                .is_some_and(|c| c.playback_segment == reference.playback_segment)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OwnerKey {
    start: u32,
    chord: String,
    lyric: String,
    row: String,
}

struct Owner {
    key: OwnerKey,
    heads: Vec<usize>,
    extension: SourceExtension,
    valid: bool,
}

/// Follow only adapter-validated merged intermediates. Their pitchless IR
/// records are still source notes, not missing notes to reconstruct geometrically.
fn retained_tie_head(
    reference: &SourceNoteRef,
    scope: &Domain,
    projected: &BTreeMap<RefKey, Vec<usize>>,
    sources: &BTreeMap<RefKey, Vec<&SourceNote>>,
    accepts: impl Fn(usize) -> bool,
    budget: &Budget,
) -> Option<usize> {
    visit_retained_tie_head(
        reference,
        scope,
        projected,
        sources,
        accepts,
        |_| Ok(()),
        budget,
    )
}

/// The selected continuation captures exactly the raw links walked by the same
/// ownership lookup. Other discovery queries neither copy nor retain them.
fn visit_retained_tie_head(
    reference: &SourceNoteRef,
    scope: &Domain,
    projected: &BTreeMap<RefKey, Vec<usize>>,
    sources: &BTreeMap<RefKey, Vec<&SourceNote>>,
    accepts: impl Fn(usize) -> bool,
    mut visit: impl FnMut(&SourceNote) -> Result<(), String>,
    budget: &Budget,
) -> Option<usize> {
    let mut reference = reference;
    let mut visited = BTreeSet::new();
    loop {
        // One retained visited key and up to two temporary owned lookup keys.
        // Reserve before either allocation; limit errors poison the shared
        // budget so the outer planner cannot mistake refusal for absent proof.
        if budget.charge(reference.source_id.len() / 4 + 1).is_err()
            || budget
                .reserve(
                    reference
                        .source_id
                        .len()
                        .saturating_mul(2)
                        .saturating_add(384),
                )
                .is_err()
            || !visited.insert((
                reference.source_id.as_str(),
                reference.occurrence,
                reference.playback_segment,
            ))
        {
            return None;
        }
        if let Some(projected) = projected.get(&ref_key(reference)) {
            if budget.charge(projected.len()).is_err() {
                return None;
            }
            let mut matches = projected.iter().copied().filter(|i| accepts(*i));
            if let Some(head) = matches.next() {
                return matches.next().is_none().then_some(head);
            }
        }
        let raw = sources.get(&ref_key(reference))?;
        if budget.charge(raw.len()).is_err() {
            return None;
        }
        let mut matches = raw.iter().filter(|n| same_domain(&n.source, scope));
        let source = matches.next()?;
        if matches.next().is_some() || source.pitch.is_some() {
            return None;
        }
        let tie = source.source.continuity.as_ref()?.incoming_tie.as_ref()?;
        if &tie.tail != reference || tie.contact_tick != source.onset {
            return None;
        }
        if visit(source).is_err() {
            return None;
        }
        reference = &tie.head;
    }
}

fn push_merged_tie_source(
    collected: &mut Vec<MergedTieSource>,
    note: &SourceNote,
    budget: &Budget,
) -> Result<(), String> {
    let source = &note.source;
    let mut bytes = source.id.len();
    for field in [
        &source.part_id,
        &source.staff_id,
        &source.voice,
        &source.chord_id,
        &source.instrument_id,
    ]
    .into_iter()
    .flatten()
    {
        bytes = bytes.saturating_add(field.len());
    }
    if let Some(unpitched) = &source.unpitched {
        for field in [&unpitched.instrument_id, &unpitched.display_step]
            .into_iter()
            .flatten()
        {
            bytes = bytes.saturating_add(field.len());
        }
    }
    // Source continuity/XML is Arc-backed and stays shared. Four record slots
    // per entry conservatively cover the Vec's initial and amortized growth.
    bytes = bytes.saturating_add(std::mem::size_of::<MergedTieSource>().saturating_mul(4));
    budget.reserve(bytes)?;
    budget.charge(bytes / 8 + 1)?;
    collected.push(MergedTieSource {
        source: source.clone(),
        onset_ticks: note.onset,
        duration_ticks: note.duration,
    });
    Ok(())
}

fn diagnose(
    budget: &mut Budget,
    report: &mut [TrackReport],
    pending: &[(usize, Vec<String>, ProjectedTrack)],
    atom: &Atom,
    code: &str,
    message: impl AsRef<str>,
) -> Result<(), String> {
    budget.diagnostic(message.as_ref().len() + atom.id().len() + 180)?;
    let source = &atom.origin().source;
    report[pending[atom.original_lane].0].warnings.push(report_warning(code,
        DiagnosticSeverity::Warning,
        format!("{}; note {}, occurrence {}, interval {}–{}. Original source and applicable Part stems preserve unresolved notation.",
            message.as_ref(), atom.id(), source.occurrence, atom.note.onset_ticks,
            u64::from(atom.note.onset_ticks) + u64::from(atom.note.duration_ticks)), atom.id()));
    Ok(())
}

/// Candidate admission is separate from vocal policy. These frames expose only
/// otherwise-unadmitted source notes to the proof planner; the caller discards
/// every unresolved remainder. Explicitly disabled source tracks never enter.
pub(super) fn candidate_projections(
    pending: &[(usize, Vec<String>, ProjectedTrack)],
    source_notes: &[Vec<SourceNote>],
    disabled: &BTreeSet<usize>,
    budget: &Budget,
) -> Result<Vec<CandidateProjection>, String> {
    // These indexes live only during read-only discovery. Borrow source keys
    // and selected row groups instead of multiplying long source identities.
    type Scope<'a> = (&'a str, &'a str, &'a str, u32);
    type Key<'a> = (&'a str, u32, u32);
    fn scope(source: &NoteSource) -> Option<Scope<'_>> {
        Some((
            source.part_id.as_deref()?,
            source.staff_id.as_deref()?,
            source.voice.as_deref()?,
            source.continuity.as_ref()?.playback_segment,
        ))
    }
    fn key(source: &NoteSource) -> Key<'_> {
        (
            &source.id,
            source.occurrence,
            source.continuity.as_ref().map_or(0, |c| c.playback_segment),
        )
    }
    fn reference_key(reference: &SourceNoteRef) -> Key<'_> {
        (
            &reference.source_id,
            reference.occurrence,
            reference.playback_segment,
        )
    }
    fn lookup_work(
        domain: Scope<'_>,
        id: &str,
        entries: usize,
        budget: &Budget,
    ) -> Result<(), String> {
        let bytes = domain
            .0
            .len()
            .saturating_add(domain.1.len())
            .saturating_add(domain.2.len())
            .saturating_add(id.len());
        budget.charge((bytes / 8 + 1).saturating_mul(entries.max(1).ilog2() as usize + 1))
    }
    let mut has_continuity = false;
    for (_, _, track) in pending {
        budget.charge(track.notes.len())?;
        has_continuity |= track.notes.iter().any(|n| {
            n.source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
                .is_some_and(|o| o.source.continuity.is_some())
        });
    }
    if !has_continuity {
        return Ok(Vec::new());
    }
    budget.reserve(pending.len().saturating_mul(128))?;
    let admitted: BTreeSet<_> = pending.iter().map(|entry| entry.0).collect();
    let mut heads = BTreeMap::<(Scope<'_>, Key<'_>), BTreeSet<&[String]>>::new();
    let mut extensions = BTreeMap::<Scope<'_>, BTreeMap<u32, Vec<(u32, &[String])>>>::new();
    for (_, group, track) in pending {
        budget.charge(track.notes.len())?;
        for note in &track.notes {
            let Some(origin) = note
                .source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
            else {
                continue;
            };
            let Some(domain) = scope(&origin.source) else {
                continue;
            };
            let ProjectedLyric::Source(lyric) = &note.lyric else {
                continue;
            };
            if origin.lyric_conflict
                || !matches!(&lyric.state, LyricState::Text(text) if !text.trim().is_empty())
            {
                continue;
            }
            lookup_work(domain, &origin.source.id, heads.len(), budget)?;
            budget.charge(
                group
                    .iter()
                    .fold(0usize, |n, s| n.saturating_add(s.len() / 8 + 1)),
            )?;
            budget.reserve(512)?;
            heads
                .entry((domain, key(&origin.source)))
                .or_default()
                .insert(group);
            let continuity = origin.source.continuity.as_ref().unwrap();
            budget.charge(continuity.extensions.len())?;
            for extension in &continuity.extensions {
                if extension.lyric_id != lyric.id
                    || extension.lane != lyric.lane
                    || extension.raw_ticks == Some(1)
                    || extension.start_tick != note.onset_ticks
                    || extension.chord_id != continuity.chord_id
                    || extension.occurrence != origin.source.occurrence
                {
                    continue;
                }
                if let Some(end) = extension.end_tick.filter(|end| *end > note.onset_ticks) {
                    budget.reserve(512)?;
                    extensions
                        .entry(domain)
                        .or_default()
                        .entry(note.onset_ticks)
                        .or_default()
                        .push((end, group));
                }
            }
        }
    }
    if heads.is_empty() {
        return Ok(Vec::new());
    }
    budget.reserve(heads.len().saturating_mul(128))?;
    let scopes: BTreeSet<_> = heads.keys().map(|(domain, _)| *domain).collect();
    let mut raw_refs = BTreeMap::<Key<'_>, Vec<&SourceNote>>::new();
    let mut relevant_count = 0usize;
    for notes in source_notes {
        budget.charge(notes.len())?;
        for note in notes {
            if !scope(&note.source).is_some_and(|domain| scopes.contains(&domain)) {
                continue;
            }
            relevant_count = relevant_count.saturating_add(1);
            if relevant_count > MAX_NOTES {
                return Err(format!(
                    "{LIMIT}: continuity input exceeds {MAX_NOTES} notes"
                ));
            }
            lookup_work(
                scope(&note.source).unwrap(),
                &note.source.id,
                raw_refs.len(),
                budget,
            )?;
            budget.reserve(256)?;
            raw_refs.entry(key(&note.source)).or_default().push(note);
        }
    }
    let mut candidates = BTreeMap::<(usize, &[String]), BTreeSet<u32>>::new();
    for (index, notes) in source_notes.iter().enumerate() {
        if disabled.contains(&index) || admitted.contains(&index) {
            continue;
        }
        budget.charge(notes.len())?;
        for note in notes {
            let Some(domain) = scope(&note.source) else {
                continue;
            };
            if note.pitch.is_none() || note.duration == 0 || !scopes.contains(&domain) {
                continue;
            }
            let continuity = note.source.continuity.as_ref().unwrap();
            let mut groups = BTreeSet::new();
            if let Some(tie) = &continuity.incoming_tie {
                let mut reference = &tie.head;
                let mut visited = BTreeSet::new();
                loop {
                    lookup_work(
                        domain,
                        &reference.source_id,
                        heads.len().max(raw_refs.len()),
                        budget,
                    )?;
                    budget.reserve(128)?;
                    if !visited.insert(reference_key(reference)) {
                        break;
                    }
                    if let Some(owners) = heads.get(&(domain, reference_key(reference))) {
                        budget.charge(owners.len())?;
                        budget.reserve(owners.len().saturating_mul(128))?;
                        groups.extend(owners.iter().copied());
                        break;
                    }
                    let Some(raw) = raw_refs.get(&reference_key(reference)) else {
                        break;
                    };
                    let [raw] = raw.as_slice() else { break };
                    if scope(&raw.source) != Some(domain) {
                        break;
                    }
                    let Some(link) = raw
                        .source
                        .continuity
                        .as_ref()
                        .and_then(|c| c.incoming_tie.as_ref())
                    else {
                        break;
                    };
                    if &link.tail != reference {
                        break;
                    }
                    reference = &link.head;
                }
            }
            if let Some(starts) = extensions.get(&domain) {
                for (_, ends) in starts.range(..note.onset) {
                    budget.charge(ends.len())?;
                    for (end, group) in ends {
                        if note.onset <= *end {
                            budget.reserve(128)?;
                            groups.insert(*group);
                        }
                    }
                }
            }
            for group in groups {
                budget.reserve(256)?;
                candidates
                    .entry((index, group))
                    .or_default()
                    .insert(note.source_order);
            }
        }
    }
    budget.reserve(
        candidates
            .len()
            .saturating_mul(std::mem::size_of::<CandidateProjection>()),
    )?;
    let mut result = Vec::with_capacity(candidates.len());
    for ((index, group), orders) in candidates {
        let bytes = group.iter().fold(
            group.len().saturating_mul(std::mem::size_of::<String>()),
            |n, s| n.saturating_add(s.len()),
        );
        budget.reserve(bytes)?;
        budget.charge(group.len().saturating_add(bytes / 8))?;
        result.push((index, group.to_vec(), orders));
    }
    Ok(result)
}

/// Build the identity-keyed plan against raw selected lyrics, then move each
/// original atom once. Original event keys and performance ownership are never
/// rebuilt; a proven score tie may inherit its predecessor's intensity context.
#[cfg(test)]
pub(super) fn resolve(
    pending: &mut [(usize, Vec<String>, ProjectedTrack)],
    source_notes: &[Vec<SourceNote>],
    report: &mut [TrackReport],
    performance: &crate::engine::performance::PerformanceIndex,
) -> Result<(), String> {
    resolve_bounded(
        pending,
        source_notes,
        report,
        performance,
        &mut Budget::default(),
    )
}

pub(super) fn resolve_bounded(
    pending: &mut [(usize, Vec<String>, ProjectedTrack)],
    source_notes: &[Vec<SourceNote>],
    report: &mut [TrackReport],
    performance: &crate::engine::performance::PerformanceIndex,
    budget: &mut Budget,
) -> Result<(), String> {
    // Source diagnostics survive even when no editable atom participates.
    // Only the proof planner's allocations/count ceiling depend on participation.
    for (track_index, notes) in source_notes.iter().enumerate() {
        for source in notes {
            if let Some(continuity) = source
                .source
                .continuity
                .as_ref()
                .filter(|c| !c.issues.is_empty())
            {
                let id = super::note_instance_id(
                    &report[track_index].source_id,
                    &source.source,
                    source.source_order,
                );
                for issue in &continuity.issues {
                    budget.diagnostic(issue.message.len() + id.len() + 180)?;
                    report[track_index].warnings.push(report_warning(issue.code, DiagnosticSeverity::Warning,
                    format!("{}; occurrence {}, source interval {}–{}. Original source and applicable Part stems preserve unresolved notation.", issue.message, source.source.occurrence, source.onset, u64::from(source.onset) + u64::from(source.duration)), &id));
                }
            }
        }
    }
    let mut participating = BTreeSet::new();
    let mut count = 0usize;
    for (_, _, track) in pending.iter() {
        budget.charge(track.notes.len())?;
        for note in &track.notes {
            let Some(source) = note
                .source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
                .map(|o| &o.source)
            else {
                continue;
            };
            if !has_domain(source) {
                continue;
            }
            count = count.saturating_add(1);
            if count > MAX_NOTES {
                return Err(format!(
                    "{LIMIT}: continuity input exceeds {MAX_NOTES} notes"
                ));
            }
            reserve_domain(source, budget)?;
            participating.insert(domain(source).unwrap());
        }
    }
    if participating.is_empty() {
        return Ok(());
    }
    // Matching uses borrowed components; repeated membership tests do not clone
    // the source's strings or charge hypothetical copied timelines.
    budget.reserve(participating.len().saturating_mul(256))?;
    let domain_keys: BTreeSet<_> = participating
        .iter()
        .map(|d| (d.0.as_str(), d.1.as_str(), d.2.as_str(), d.3))
        .collect();
    let relevant = |source: &NoteSource| match (
        &source.part_id,
        &source.staff_id,
        &source.voice,
        &source.continuity,
    ) {
        (Some(p), Some(s), Some(v), Some(c)) => {
            domain_keys.contains(&(p.as_str(), s.as_str(), v.as_str(), c.playback_segment))
        }
        _ => false,
    };
    let mut raw_count = 0usize;
    for notes in source_notes {
        budget.charge(
            notes
                .len()
                .saturating_mul(participating.len().max(1).ilog2() as usize + 1),
        )?;
        for source in notes {
            if !relevant(&source.source) {
                continue;
            }
            raw_count = raw_count.saturating_add(1);
            if raw_count > MAX_NOTES {
                return Err(format!(
                    "{LIMIT}: continuity input exceeds {MAX_NOTES} notes"
                ));
            }
        }
    }
    let restored_slots = pending.iter().fold(0usize, |n, (_, _, track)| {
        n.saturating_add(track.notes.len())
    });
    budget.reserve(
        count
            .saturating_mul(std::mem::size_of::<Atom>())
            .saturating_add(restored_slots.saturating_mul(std::mem::size_of::<ProjectedNote>())),
    )?;
    let mut source_refs = BTreeMap::<RefKey, Vec<&SourceNote>>::new();
    let mut source_onsets = BTreeMap::<Domain, BTreeMap<u32, Vec<&SourceNote>>>::new();
    let mut unmapped: BTreeMap<Domain, BTreeSet<u32>> = BTreeMap::new();
    for notes in source_notes {
        for source in notes {
            if !relevant(&source.source) {
                continue;
            }
            reserve_source_key(&source.source, budget)?;
            source_refs
                .entry(source_key(&source.source))
                .or_default()
                .push(source);
            reserve_domain(&source.source, budget)?;
            if let Some(scope) = domain(&source.source) {
                budget.reserve(256)?;
                source_onsets
                    .entry(scope)
                    .or_default()
                    .entry(source.onset)
                    .or_default()
                    .push(source);
            }
            // A bare tail already absorbed into its source head is not an unmapped
            // pitch. Its validated relation and source provenance remain intact.
            let merged_tail = source.pitch.is_none()
                && source
                    .source
                    .continuity
                    .as_ref()
                    .is_some_and(|c| c.incoming_tie.is_some());
            if (source.pitch.is_none() && !merged_tail) || source.duration == 0 {
                reserve_domain(&source.source, budget)?;
                if let Some(scope) = domain(&source.source) {
                    budget.reserve(128)?;
                    unmapped.entry(scope).or_default().insert(source.onset);
                }
            }
        }
    }
    let mut atoms = Vec::new();
    for (lane, (_, _, track)) in pending.iter_mut().enumerate() {
        for note in std::mem::take(&mut track.notes) {
            if !note
                .source_evidence
                .as_ref()
                .and_then(|e| e.origin.as_ref())
                .is_some_and(|o| relevant(&o.source))
            {
                track.notes.push(note);
                continue;
            }
            atoms.push(Atom {
                note,
                original_lane: lane,
                destination: lane,
            });
        }
    }
    // Source scope is independent of sibling projection inventory sizes.
    // Actual selected rows constrain each ownership query below.
    let mut rows = BTreeMap::<String, String>::new();
    let mut domains = BTreeMap::<Domain, Vec<usize>>::new();
    let mut lane_spans = BTreeMap::<usize, BTreeMap<(u32, usize), u32>>::new();
    for (index, atom) in atoms.iter().enumerate() {
        budget.reserve(512)?;
        if let ProjectedLyric::Source(lyric) = &atom.note.lyric {
            budget.reserve(
                lyric
                    .id
                    .len()
                    .saturating_add(lyric.lane.len())
                    .saturating_add(128),
            )?;
            rows.insert(lyric.id.clone(), lyric.lane.clone());
        }
        lane_spans.entry(atom.destination).or_default().insert(
            (atom.note.onset_ticks, index),
            atom.end().unwrap_or(u32::MAX),
        );
        reserve_domain(&atom.origin().source, budget)?;
        if let Some(scope) = domain(&atom.origin().source) {
            domains.entry(scope).or_default().push(index);
        }
    }
    let mut lane_monophonic = BTreeMap::new();
    for (lane, spans) in &lane_spans {
        budget.charge(spans.len())?;
        let mut end = 0;
        let mut monophonic = true;
        for ((start, _), next_end) in spans {
            if *start < end {
                monophonic = false;
            }
            end = end.max(*next_end);
        }
        lane_monophonic.insert(*lane, monophonic);
    }
    for (scope, mut indices) in domains {
        budget.charge(
            indices
                .len()
                .saturating_mul((indices.len().max(1).ilog2() as usize) + 1),
        )?;
        indices.sort_by(|a, b| {
            (atoms[*a].note.onset_ticks, atoms[*a].id())
                .cmp(&(atoms[*b].note.onset_ticks, atoms[*b].id()))
        });
        let mut onsets = BTreeMap::<u32, Vec<usize>>::new();
        let mut ends = BTreeMap::<u32, Vec<usize>>::new();
        let mut projected_refs = BTreeMap::<RefKey, Vec<usize>>::new();
        let mut selected_onsets = BTreeMap::<usize, BTreeSet<u32>>::new();
        let mut selected_words = BTreeMap::<u32, Vec<usize>>::new();
        for &i in &indices {
            let atom = &atoms[i];
            budget.reserve(1280)?;
            reserve_source_key(&atom.origin().source, budget)?;
            onsets.entry(atom.note.onset_ticks).or_default().push(i);
            if let Some(end) = atom.end() {
                ends.entry(end).or_default().push(i);
            }
            projected_refs
                .entry(source_key(&atom.origin().source))
                .or_default()
                .push(i);
            if matches!(atom.note.lyric, ProjectedLyric::Source(_)) {
                selected_words
                    .entry(atom.note.onset_ticks)
                    .or_default()
                    .push(i);
                selected_onsets
                    .entry(atom.original_lane)
                    .or_default()
                    .insert(atom.note.onset_ticks);
            }
        }
        let mut owners: BTreeMap<OwnerKey, Owner> = BTreeMap::new();
        for &index in &indices {
            let atom = &atoms[index];
            let ProjectedLyric::Source(lyric) = &atom.note.lyric else {
                continue;
            };
            let continuity = atom.origin().source.continuity.as_ref().unwrap();
            budget.charge(continuity.extensions.len())?;
            for extension in continuity
                .extensions
                .iter()
                .filter(|e| e.lyric_id == lyric.id && e.lane == lyric.lane)
            {
                // No, zero and temporary single-chord lengths authorize no
                // later chord. Invalid values are already adapter diagnostics.
                let Some(end) = extension
                    .end_tick
                    .filter(|end| *end > atom.note.onset_ticks)
                else {
                    continue;
                };
                if extension.raw_ticks == Some(1) {
                    continue;
                }
                let key_bytes = extension
                    .chord_id
                    .len()
                    .saturating_add(lyric.id.len())
                    .saturating_add(lyric.lane.len());
                budget.reserve(
                    key_bytes
                        .saturating_mul(2)
                        .saturating_add(extension.lyric_id.len())
                        .saturating_add(extension.lane.len())
                        .saturating_add(extension.chord_id.len())
                        .saturating_add(extension.evidence.source_id.len())
                        .saturating_add(512),
                )?;
                let key = OwnerKey {
                    start: atom.note.onset_ticks,
                    chord: extension.chord_id.clone(),
                    lyric: lyric.id.clone(),
                    row: lyric.lane.clone(),
                };
                let valid = atom.sung_head()
                    && extension.start_tick == atom.note.onset_ticks
                    && extension.chord_id == continuity.chord_id
                    && atom.origin().source.chord_id.as_deref()
                        == Some(continuity.chord_id.as_str())
                    && extension.occurrence == atom.origin().source.occurrence
                    && extension.playback_segment == scope.3
                    && (onsets.contains_key(&end)
                        || source_onsets
                            .get(&scope)
                            .and_then(|onsets| onsets.get(&end))
                            .into_iter()
                            .flatten()
                            .any(|n| {
                                n.pitch.is_none()
                                    && n.source
                                        .continuity
                                        .as_ref()
                                        .and_then(|c| c.incoming_tie.as_ref())
                                        .is_some_and(|tie| {
                                            if tie.tail.source_id != n.source.id
                                                || tie.tail.occurrence != n.source.occurrence
                                                || n.source.continuity.as_ref().is_none_or(|c| {
                                                    c.playback_segment != tie.tail.playback_segment
                                                })
                                                || tie.contact_tick != n.onset
                                            {
                                                return false;
                                            }
                                            retained_tie_head(
                                                &tie.head,
                                                &scope,
                                                &projected_refs,
                                                &source_refs,
                                                |head| {
                                                    accepts_row(
                                                        &pending[atoms[head].original_lane].1,
                                                        &lyric.lane,
                                                    ) && source_row(&atoms[head], &rows)
                                                        .is_none_or(|row| row == lyric.lane)
                                                },
                                                budget,
                                            )
                                            .is_some_and(|head| {
                                                let retained = &atoms[head];
                                                // The authored endpoint may have been merged
                                                // into a later chord inside this melisma.
                                                // This proves endpoint existence only; the
                                                // planner still proves every touching owner
                                                // link before retaining any blank note.
                                                let same_head = retained.origin().source.chord_id
                                                    == atom.origin().source.chord_id
                                                    && retained.note.onset_ticks
                                                        == atom.note.onset_ticks;
                                                let later_head = retained.note.onset_ticks
                                                    > atom.note.onset_ticks
                                                    && retained.note.onset_ticks < end
                                                    && !retained.origin().lyric_conflict
                                                    && matches!(
                                                        retained.note.lyric,
                                                        ProjectedLyric::Absent
                                                            | ProjectedLyric::Extension
                                                    );
                                                (same_head || later_head)
                                                    && retained.note.pitch == tie.pitch
                                                    && retained.origin().source.occurrence
                                                        == atom.origin().source.occurrence
                                                    && atoms[head].end().is_some_and(|head_end| {
                                                        n.onset
                                                            .checked_add(n.duration)
                                                            .is_some_and(|end| head_end >= end)
                                                    })
                                            })
                                        })
                            }))
                    && unmapped.get(&scope).is_none_or(|ticks| {
                        ticks.range(atom.note.onset_ticks..=end).next().is_none()
                    });
                let owner = owners.entry(key.clone()).or_insert_with(|| Owner {
                    key,
                    heads: Vec::new(),
                    extension: extension.clone(),
                    valid,
                });
                owner.valid &= valid && owner.extension == *extension;
                owner.heads.push(index);
            }
        }
        for owner in owners.values_mut() {
            owner.heads.sort_unstable();
            owner.heads.dedup();
            // Inspect every selected representation, including zero/invalid
            // extension copies that could not create a candidate on their own.
            let coeval = &onsets[&owner.key.start];
            budget.charge(coeval.len())?;
            let conflicting_copy = coeval.iter().any(|i| {
                let atom = &atoms[*i];
                if !accepts_row(&pending[atom.original_lane].1, &owner.key.row) {
                    return false;
                }
                let ProjectedLyric::Source(lyric) = &atom.note.lyric else {
                    return false;
                };
                let continuity = atom.origin().source.continuity.as_ref().unwrap();
                // Independent source chords are not copies of this owner.
                if continuity.chord_id != owner.key.chord {
                    return false;
                }
                lyric.id != owner.key.lyric
                    || lyric.lane != owner.key.row
                    || continuity.chord_id != owner.key.chord
                    || atom.origin().lyric_conflict
                    || !continuity
                        .extensions
                        .iter()
                        .any(|extension| extension == &owner.extension)
                    || continuity.extensions.iter().any(|extension| {
                        extension.lyric_id == owner.key.lyric
                            && extension.lane == owner.key.row
                            && extension != &owner.extension
                    })
            });
            if conflicting_copy {
                owner.valid = false;
                for &head in &owner.heads {
                    diagnose(
                        budget,
                        report,
                        pending,
                        &atoms[head],
                        OWNER_AMBIGUOUS,
                        format!(
                            "Selected chord/lyric representations conflict with owner {}",
                            owner.key.lyric
                        ),
                    )?;
                }
            }
            if !owner.valid {
                diagnose(budget, report, pending, &atoms[owner.heads[0]], LINK_INVALID,
                    format!("Extension owner {} on chord {} has conflicting evidence, an unmapped note or a missing endpoint", owner.key.lyric, owner.key.chord))?;
            }
        }
        budget.reserve(owners.len().saturating_mul(384))?;
        let owner_list: Vec<_> = owners.values().collect();
        let mut next_owner = 0usize;
        let mut active = BTreeSet::<usize>::new();
        let mut expirations = BTreeSet::<(u32, usize)>::new();
        for &tail_index in &indices {
            budget.charge(1)?;
            if !matches!(
                atoms[tail_index].note.lyric,
                ProjectedLyric::Absent | ProjectedLyric::Extension
            ) || atoms[tail_index].origin().lyric_conflict
            {
                continue;
            }
            let onset = atoms[tail_index].note.onset_ticks;
            while next_owner < owner_list.len() && owner_list[next_owner].key.start < onset {
                let owner = owner_list[next_owner];
                if let Some(end) = owner.extension.end_tick {
                    active.insert(next_owner);
                    expirations.insert((end, next_owner));
                }
                next_owner += 1;
                budget.charge(1)?;
            }
            while let Some((end, id)) = expirations.first().copied() {
                if end >= onset {
                    break;
                }
                expirations.remove(&(end, id));
                active.remove(&id);
                budget.charge(1)?;
            }
            let source = &atoms[tail_index].origin().source;
            if source.chord_id.as_deref() != source.continuity.as_ref().map(|c| c.chord_id.as_str())
            {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    LINK_INVALID,
                    "Source chord identity contradicts its continuity evidence",
                )?;
                continue;
            }
            let incoming = atoms[tail_index]
                .origin()
                .source
                .continuity
                .as_ref()
                .unwrap()
                .incoming_tie
                .as_ref();
            let mut tie_head = None;
            if let Some(tie) = incoming {
                budget.reserve(32)?;
                let heads: Vec<_> = retained_tie_head(
                    &tie.head,
                    &scope,
                    &projected_refs,
                    &source_refs,
                    |head| {
                        source_row(&atoms[head], &rows).is_none_or(|row| {
                            accepts_row(&pending[atoms[tail_index].original_lane].1, row)
                        })
                    },
                    budget,
                )
                .into_iter()
                .collect();
                if heads.len() != 1
                    || !atoms[tail_index].matches_ref(&tie.tail)
                    || tie.contact_tick != onset
                    || tie.pitch != atoms[tail_index].note.pitch
                    || heads.first().is_some_and(|i| {
                        atoms[*i].note.pitch != tie.pitch || atoms[*i].end() != Some(onset)
                    })
                {
                    diagnose(budget, report, pending, &atoms[tail_index], LINK_INVALID,
                        format!("Incoming tie from {} has a stale, non-touching or contradictory source link", tie.head.source_id))?;
                    continue;
                }
                if atoms[heads[0]].sung_head() {
                    if selected_onsets
                        .get(&atoms[heads[0]].destination)
                        .is_some_and(|ticks| {
                            ticks
                                .range((
                                    std::ops::Bound::Excluded(atoms[heads[0]].note.onset_ticks),
                                    std::ops::Bound::Excluded(onset),
                                ))
                                .next()
                                .is_some()
                        })
                    {
                        diagnose(
                            budget,
                            report,
                            pending,
                            &atoms[tail_index],
                            LINK_INVALID,
                            format!(
                                "An intervening selected lyric contradicts tie head {}",
                                atoms[heads[0]].id()
                            ),
                        )?;
                        continue;
                    }
                    tie_head = Some(heads[0]);
                }
            }
            budget.charge(active.len())?;
            budget.reserve(active.len().saturating_mul(16))?;
            let covering: Vec<_> = active
                .iter()
                .map(|i| owner_list[*i])
                .filter(|owner| {
                    accepts_row(&pending[atoms[tail_index].original_lane].1, &owner.key.row)
                })
                .filter(|owner| {
                    tie_head.is_none_or(|head| {
                        // An explicit source link is not overridden by a parallel
                        // voice's extension. Only this exact chain may contradict it.
                        owner.heads.contains(&head)
                            || (owner.key.start == atoms[head].note.onset_ticks
                                && atoms[head].origin().source.chord_id.as_deref()
                                    == Some(owner.key.chord.as_str()))
                            || atoms[head]
                                .origin()
                                .continuation
                                .as_ref()
                                .is_some_and(|link| link.lyric_owner_id == owner.key.lyric)
                    })
                })
                .collect();
            // Distinct source owners never become one because their text or
            // pitch happens to match. An intervening selected word also blocks.
            if covering.len() > 1 {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    OWNER_AMBIGUOUS,
                    format!(
                        "Competing extension owners: {}",
                        covering
                            .iter()
                            .take(8)
                            .map(|o| o.key.lyric.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )?;
                continue;
            }
            let owner = covering.first().copied();
            let mut candidates = Vec::new();
            let mut lyric_owner = None;
            if let Some(owner) = owner {
                if !owner.valid {
                    diagnose(
                        budget,
                        report,
                        pending,
                        &atoms[tail_index],
                        LINK_INVALID,
                        format!(
                            "Conflicting or incomplete source evidence for owner {}",
                            owner.key.lyric
                        ),
                    )?;
                    continue;
                }
                budget.charge(owner.heads.len())?;
                let blocked = if let Some(head) = tie_head {
                    selected_onsets
                        .get(&atoms[head].destination)
                        .is_some_and(|ticks| {
                            ticks
                                .range((
                                    std::ops::Bound::Excluded(owner.key.start),
                                    std::ops::Bound::Excluded(onset),
                                ))
                                .next()
                                .is_some()
                        })
                } else {
                    let mut blocked = false;
                    for (_, coeval) in selected_words.range((
                        std::ops::Bound::Excluded(owner.key.start),
                        std::ops::Bound::Excluded(onset),
                    )) {
                        budget.charge(coeval.len())?;
                        if coeval.iter().any(|i| {
                            matches!(atoms[*i].note.lyric, ProjectedLyric::Source(_))
                                && accepts_row(&pending[atoms[*i].original_lane].1, &owner.key.row)
                        }) {
                            blocked = true;
                            break;
                        }
                    }
                    blocked
                };
                if blocked {
                    diagnose(
                        budget,
                        report,
                        pending,
                        &atoms[tail_index],
                        LINK_INVALID,
                        format!(
                            "An intervening selected lyric blocks extension {}",
                            owner.key.lyric
                        ),
                    )?;
                    continue;
                }
                budget.reserve(
                    owner
                        .key
                        .lyric
                        .len()
                        .saturating_add(owner.heads.len().saturating_mul(16)),
                )?;
                lyric_owner = Some(owner.key.lyric.clone());
                candidates.extend(
                    owner
                        .heads
                        .iter()
                        .copied()
                        .filter(|i| atoms[*i].end() == Some(onset)),
                );
                let touching = ends.get(&onset).map(Vec::as_slice).unwrap_or(&[]);
                budget.charge(touching.len())?;
                budget.reserve(touching.len().saturating_mul(16))?;
                candidates.extend(touching.iter().copied().filter(|i| {
                    atoms[*i]
                        .origin()
                        .continuation
                        .as_ref()
                        .is_some_and(|link| link.lyric_owner_id == owner.key.lyric)
                }));
                candidates.sort_unstable();
                candidates.dedup();
            }
            let predecessor = if let Some(head) = tie_head {
                if owner.is_some() && !candidates.contains(&head) {
                    diagnose(
                        budget,
                        report,
                        pending,
                        &atoms[tail_index],
                        OWNER_AMBIGUOUS,
                        format!(
                            "Tie head {} contradicts the selected extension owner",
                            atoms[head].id()
                        ),
                    )?;
                    continue;
                }
                Some(head)
            } else if owner.is_some() {
                // A source-proven same-lane melisma keeps its existing pitch
                // contour. Crossing technical lanes requires one touching
                // same-pitch member of the already established owner.
                budget.reserve(candidates.len().saturating_mul(32))?;
                let local: Vec<_> = candidates
                    .iter()
                    .copied()
                    .filter(|i| atoms[*i].destination == atoms[tail_index].destination)
                    .collect();
                if candidates.len() == 1
                    && owner.is_some_and(|o| o.heads.len() == 1)
                    && local.len() == 1
                    && onsets[&onset].len() == 1
                {
                    Some(local[0])
                } else {
                    let matching: Vec<_> = candidates
                        .iter()
                        .copied()
                        .filter(|i| atoms[*i].note.pitch == atoms[tail_index].note.pitch)
                        .collect();
                    if matching.len() == 1 {
                        Some(matching[0])
                    } else {
                        None
                    }
                }
            } else {
                continue;
            };
            let Some(head_index) = predecessor else {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    ROUTE_UNRESOLVED,
                    format!(
                        "Owner {} has no unique touching predecessor (candidates: {})",
                        lyric_owner.as_deref().unwrap_or("tie"),
                        candidates
                            .iter()
                            .take(8)
                            .map(|i| atoms[*i].id())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )?;
                continue;
            };
            let destination = atoms[head_index].destination;
            // Two distinct endpoints competing for one predecessor cannot be
            // resolved by the order their IDs happen to sort in.
            budget.charge(onsets[&onset].len())?;
            if onsets[&onset].iter().any(|i| {
                if *i == tail_index
                    || atoms[*i].note.pitch != atoms[tail_index].note.pitch
                    || !matches!(
                        atoms[*i].note.lyric,
                        ProjectedLyric::Absent | ProjectedLyric::Extension
                    )
                {
                    return false;
                }
                let other = &atoms[*i];
                if let Some(tie) = other
                    .origin()
                    .source
                    .continuity
                    .as_ref()
                    .and_then(|c| c.incoming_tie.as_ref())
                {
                    retained_tie_head(
                        &tie.head,
                        &scope,
                        &projected_refs,
                        &source_refs,
                        |head| {
                            source_row(&atoms[head], &rows)
                                .is_none_or(|row| accepts_row(&pending[other.original_lane].1, row))
                        },
                        budget,
                    ) == Some(head_index)
                } else {
                    tie_head.is_none()
                        && owner.is_some_and(|owner| {
                            accepts_row(&pending[other.original_lane].1, &owner.key.row)
                        })
                }
            }) {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    OWNER_AMBIGUOUS,
                    format!(
                        "Distinct source endpoints compete for predecessor {}",
                        atoms[head_index].id()
                    ),
                )?;
                continue;
            }
            let Some(end) = atoms[tail_index].end() else {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    LINK_INVALID,
                    "Continuation interval overflows source ticks",
                )?;
                continue;
            };
            if unmapped.get(&scope).is_some_and(|ticks| {
                ticks
                    .range(atoms[head_index].note.onset_ticks..end)
                    .next()
                    .is_some()
            }) {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    LINK_INVALID,
                    format!(
                        "An unmapped source note or rest contradicts predecessor {}",
                        atoms[head_index].id()
                    ),
                )?;
                continue;
            }
            // Indexed by destination and onset. Remaining overlap traversal
            // is charged even for a malformed nonmonophonic input lane.
            let mut occupied = false;
            if let Some(spans) = lane_spans.get(&destination) {
                for ((_, i), other_end) in spans.range(..(end, 0)).rev() {
                    budget.charge(1)?;
                    if *i == tail_index {
                        continue;
                    }
                    if *other_end > onset {
                        occupied = true;
                        break;
                    }
                    // Earlier intervals of a proven monophonic lane cannot
                    // overlap after its latest earlier interval has ended.
                    if lane_monophonic[&destination] {
                        break;
                    }
                }
            }
            if occupied {
                diagnose(
                    budget,
                    report,
                    pending,
                    &atoms[tail_index],
                    ROUTE_UNRESOLVED,
                    format!(
                        "Destination of predecessor {} is occupied over {}–{}",
                        atoms[head_index].id(),
                        onset,
                        end
                    ),
                )?;
                continue;
            }
            let owner_bytes = atoms[head_index]
                .origin()
                .continuation
                .as_ref()
                .map(|c| c.lyric_owner_id.len())
                .or_else(|| match &atoms[head_index].note.lyric {
                    ProjectedLyric::Source(l) => Some(l.id.len()),
                    _ => None,
                })
                .unwrap_or(0);
            budget.reserve(
                owner_bytes
                    .saturating_add(atoms[head_index].id().len())
                    .saturating_add(pending[destination].2.source_track_id.len())
                    .saturating_add(384),
            )?;
            let lyric_owner_id = lyric_owner.unwrap_or_else(|| {
                atoms[head_index]
                    .origin()
                    .continuation
                    .as_ref()
                    .map(|c| c.lyric_owner_id.clone())
                    .or_else(|| match &atoms[head_index].note.lyric {
                        ProjectedLyric::Source(l) => Some(l.id.clone()),
                        _ => None,
                    })
                    .unwrap()
            });
            let intensity_attack_note_id = if tie_head == Some(head_index) {
                let root = atoms[head_index]
                    .origin()
                    .continuation
                    .as_ref()
                    .and_then(|link| link.intensity_attack_note_id.as_deref())
                    .unwrap_or(atoms[head_index].id());
                budget.reserve(root.len())?;
                Some(root.to_string())
            } else {
                None
            };
            let merged_tie_sources = if tie_head == Some(head_index) {
                let mut collected = Vec::new();
                let resolved = visit_retained_tie_head(
                    &incoming.unwrap().head,
                    &scope,
                    &projected_refs,
                    &source_refs,
                    |head| {
                        source_row(&atoms[head], &rows).is_none_or(|row| {
                            accepts_row(&pending[atoms[tail_index].original_lane].1, row)
                        })
                    },
                    |source| push_merged_tie_source(&mut collected, source, budget),
                    budget,
                );
                // A poisoned shared budget is a refusal, never missing proof.
                budget.charge(0)?;
                if resolved != Some(head_index) {
                    return Err(format!(
                        "{LINK_INVALID}: selected tie no longer reaches its retained predecessor"
                    ));
                }
                collected
            } else {
                Vec::new()
            };
            let link = ContinuationOwner {
                kind: if tie_head == Some(head_index) {
                    crate::engine::projection::ContinuationKind::Tie
                } else {
                    crate::engine::projection::ContinuationKind::Extension
                },
                intensity_attack_note_id,
                merged_tie_sources,
                predecessor_id: atoms[head_index].id().to_string(),
                lyric_owner_id,
                destination_track_id: pending[destination].2.source_track_id.clone(),
            };
            // Only the validated incoming tie to this selected, retained
            // predecessor authorizes performance inheritance. Extension-only
            // melismas and equal pitch are not performance ties. A new selected
            // syllable never enters this recovery branch.
            if tie_head == Some(head_index) {
                let (head_atom, tail_atom) = if head_index < tail_index {
                    let (before, after) = atoms.split_at_mut(tail_index);
                    (&before[head_index], &mut after[0])
                } else {
                    let (before, after) = atoms.split_at_mut(head_index);
                    (&after[0], &mut before[tail_index])
                };
                match (&mut tail_atom.note.performance, head_atom.note.performance.as_ref()) {
                    (Some(tail), Some(head))
                        if matches!((&tail.owner, &head.owner),
                            (crate::engine::performance::PerformanceOwner::Score { .. },
                             crate::engine::performance::PerformanceOwner::Score { .. })) => {
                        for intensity in [head.intensity.as_deref(), tail.intensity.as_deref()].into_iter().flatten() {
                            charge_intensity_copy(intensity,budget)?;
                        }
                        performance.continue_tied_intensity(tail, head)?;
                    }
                    // MIDI channel/attack payloads move unchanged. Scores with
                    // no expression have no optional performance on either end.
                    (None, None) => {}
                    (Some(tail), Some(head))
                        if tail.channel_key().is_some() && head.channel_key().is_some() => {}
                    (None, Some(head)) if head.channel_key().is_some() => {}
                    (Some(tail), None) if tail.channel_key().is_some() => {}
                    _ => return Err(format!("{LINK_INVALID}: typed score tie is missing its source-normalized intensity context")),
                }
            }
            let atom = &mut atoms[tail_index];
            lane_spans
                .get_mut(&atom.destination)
                .unwrap()
                .remove(&(onset, tail_index));
            lane_spans
                .entry(destination)
                .or_default()
                .insert((onset, tail_index), end);
            atom.destination = destination;
            atom.note.lyric = ProjectedLyric::Extension;
            atom.note
                .source_evidence
                .as_mut()
                .unwrap()
                .origin
                .as_mut()
                .unwrap()
                .continuation = Some(link);
        }
    }
    // A plan consumes actual original atoms; it never re-identifies a note by
    // pitch/time or borrows the predecessor's identity/channel payload.
    for atom in atoms {
        pending[atom.destination].2.notes.push(atom.note);
    }
    for (_, _, track) in pending {
        budget.charge(
            track
                .notes
                .len()
                .saturating_mul(track.notes.len().max(1).ilog2() as usize + 1),
        )?;
        budget.reserve(
            track
                .notes
                .len()
                .saturating_mul(std::mem::size_of::<ProjectedNote>())
                / 2,
        )?;
        track.notes.sort_by_key(|note| note.onset_ticks);
    }
    budget.charge(0)?;
    Ok(())
}
