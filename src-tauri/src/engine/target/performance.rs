//! Shared bounded traversal; source values stay target-neutral here.
use crate::engine::performance::{
    exact_source_tick_bounds, ChannelKey, ChannelPerformance, Dimension, HeldPoint,
    PerformanceIndex, PerformanceIssue, PerformanceNote, PerformanceTransfer, TransferStatus,
};
use crate::engine::projection::{ProjectedNote, ProjectedTrack};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

/// Per adaptation/report pass. Charge BEFORE copying references or text.
/// A refusal is deterministic and cannot publish a partial performance report.
pub(crate) struct Budget {
    work: usize,
    references: usize,
    bytes: usize,
    max_bytes: usize,
    max_work: usize,
    #[cfg(test)]
    breakpoint_capacity: usize,
    /// One adaptation/report pass spans every destination lane. The original
    /// channel key must resolve to one complete timeline, including provenance.
    timelines: BTreeMap<ChannelKey, Arc<ChannelPerformance>>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            work: 0,
            references: 0,
            bytes: 0,
            max_bytes: 32 * 1024 * 1024,
            max_work: 2_000_000,
            #[cfg(test)]
            breakpoint_capacity: 0,
            timelines: BTreeMap::new(),
        }
    }
}
impl Budget {
    #[cfg(test)]
    pub(crate) fn with_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            ..Self::default()
        }
    }
    #[cfg(test)]
    pub(crate) fn with_limits(max_bytes: usize, max_work: usize) -> Self {
        Self {
            max_bytes,
            max_work,
            ..Self::default()
        }
    }
    #[cfg(test)]
    pub(crate) fn breakpoint_capacity(&self) -> usize {
        self.breakpoint_capacity
    }

    /// Reserve actual breakpoint storage, filling, in-place sorting and
    /// compaction before allocating. The caller already charged range searches
    /// and its borrowed counting pass; unrelated timeline entries cost nothing.
    pub(crate) fn breakpoints(
        &mut self,
        count: usize,
    ) -> Result<Vec<crate::engine::score_intensity::Time>, String> {
        let depth = usize::BITS as usize - count.leading_zeros() as usize;
        self.work(count.saturating_mul(depth.saturating_add(3)))?;
        self.references(count)?;
        self.text(
            count.saturating_mul(std::mem::size_of::<crate::engine::score_intensity::Time>()),
        )?;
        let bounds = Vec::with_capacity(count);
        #[cfg(test)]
        {
            self.breakpoint_capacity = self.breakpoint_capacity.saturating_add(bounds.capacity());
        }
        Ok(bounds)
    }
    /// Measure borrowed serialization through a refusing sink. No JSON tree or
    /// complete serialized string exists until the entire preflight succeeds.
    pub(crate) fn json<T: serde::Serialize>(
        &mut self,
        value: &T,
    ) -> Result<serde_json::Value, String> {
        struct Counter<'a>(&'a mut Budget);
        impl std::io::Write for Counter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .work(1 + bytes.len() / 64)
                    .and_then(|_| self.0.text(bytes.len()))
                    .map_err(std::io::Error::other)?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(Counter(self), value).map_err(|error| error.to_string())?;
        serde_json::to_value(value).map_err(|error| error.to_string())
    }

    pub(crate) fn collect_ids<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a String>,
    ) -> Result<Vec<String>, String> {
        let mut unique = std::collections::BTreeSet::new();
        for id in ids {
            self.work(1 + id.len() / 64)?;
            self.references(1)?;
            unique.insert(id);
        }
        for id in &unique {
            self.text(id.len())?;
        }
        Ok(unique.into_iter().cloned().collect())
    }

    pub(crate) fn report_text(
        &mut self,
        track: &str,
        note: Option<&str>,
        message: &str,
    ) -> Result<(), String> {
        self.references(1 + usize::from(note.is_some()))?;
        self.text(
            track
                .len()
                .saturating_add(note.map_or(0, str::len))
                .saturating_add(message.len()),
        )
    }
    fn timeline(&mut self, performance: &PerformanceNote) -> Result<(), String> {
        self.work(1)?;
        let crate::engine::performance::PerformanceOwner::Midi { key, timeline } =
            &performance.owner
        else {
            return Ok(());
        };
        let Some(existing) = self.timelines.get(key).cloned() else {
            self.references(1)?;
            self.timelines.insert(*key, Arc::clone(timeline));
            return Ok(());
        };
        if Arc::ptr_eq(&existing, timeline) {
            return Ok(());
        }
        // Independently allocated but equal timelines are compatible. Bound
        // deep comparison work, including event IDs and issue text, before Eq.
        // The normal shared-Arc path does not scan the channel for each note.
        for timeline in [existing.as_ref(), timeline.as_ref()] {
            self.work(
                timeline
                    .pitch_cents
                    .len()
                    .saturating_add(timeline.linear_gain.len())
                    .saturating_add(timeline.issues.len()),
            )?;
            for point in timeline.pitch_cents.iter().chain(&timeline.linear_gain) {
                self.comparison_ids(&point.source_ids)?;
            }
            for issue in &timeline.issues {
                self.work(issue.reason.len())?;
                self.comparison_ids(&issue.source_ids)?;
            }
        }
        if existing.as_ref() != timeline.as_ref() {
            return Err(format!("MIDI_PERFORMANCE_OWNER_CONFLICT: source port {}, channel {} carries contradictory timelines", key.port, key.channel));
        }
        Ok(())
    }

    fn comparison_ids(&mut self, ids: &[String]) -> Result<(), String> {
        self.work(ids.len())?;
        for id in ids {
            self.work(id.len())?;
        }
        Ok(())
    }

    pub fn work(&mut self, count: usize) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(count)
            .ok_or("MIDI_PERFORMANCE_LIMIT: work overflow")?;
        if self.work > self.max_work {
            return Err(
                "MIDI_PERFORMANCE_LIMIT: adapter exceeds two million traversal operations".into(),
            );
        }
        Ok(())
    }
    pub fn references(&mut self, count: usize) -> Result<(), String> {
        self.references = self
            .references
            .checked_add(count)
            .ok_or("MIDI_PERFORMANCE_LIMIT: reference overflow")?;
        if self.references > 250_000 {
            return Err("MIDI_PERFORMANCE_LIMIT: report exceeds 250000 references".into());
        }
        Ok(())
    }
    pub fn text(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or("MIDI_PERFORMANCE_LIMIT: text overflow")?;
        if self.bytes > self.max_bytes {
            return Err("MIDI_PERFORMANCE_LIMIT: report exceeds 32 MiB of text".into());
        }
        Ok(())
    }
    pub fn ids(&mut self, ids: &[String]) -> Result<(), String> {
        self.references(ids.len())?;
        for id in ids {
            self.text(id.len())?;
        }
        Ok(())
    }
}

pub(crate) struct Region {
    pub notes: Range<usize>,
    pub start: u32,
    pub end: u32,
}
pub(crate) struct Regions<'a> {
    pub notes: Vec<(usize, &'a ProjectedNote)>,
    pub spans: Vec<Region>,
}

pub(crate) fn regions<'a>(
    track: &'a ProjectedTrack,
    budget: &mut Budget,
) -> Result<Regions<'a>, String> {
    budget.work(track.notes.len())?;
    budget.references(track.notes.len())?;
    let mut notes: Vec<_> = track.notes.iter().enumerate().collect();
    notes.sort_by_key(|(_, n)| n.onset_ticks);
    let mut spans = Vec::<Region>::new();
    for (position, (_, note)) in notes.iter().enumerate() {
        if let Some(performance) = &note.performance {
            budget.timeline(performance)?;
        }
        let end = note
            .onset_ticks
            .checked_add(note.duration_ticks)
            .ok_or("Performance note endpoint overflows source ticks")?;
        if end <= note.onset_ticks {
            return Err("Performance note has no positive sounding span".into());
        }
        if position > 0 {
            let previous = notes[position - 1].1;
            let previous_end = previous
                .onset_ticks
                .checked_add(previous.duration_ticks)
                .ok_or("Performance note endpoint overflows source ticks")?;
            if previous_end > note.onset_ticks {
                return Err("Performance notes overlap in a monophonic target lane".into());
            }
            if previous.performance.as_ref().and_then(|p| p.channel_key())
                == note.performance.as_ref().and_then(|p| p.channel_key())
            {
                let span = spans.last_mut().unwrap();
                span.notes.end = position + 1;
                span.end = end;
                continue;
            }
        }
        spans.push(Region {
            notes: position..position + 1,
            start: note.onset_ticks,
            end,
        });
    }
    Ok(Regions { notes, spans })
}

/// Two binary searches, never rescan the entire channel for each ownership run.
/// Include exactly one applicable predecessor, exclude the exclusive endpoint.
pub(crate) fn points_in_span(points: &[HeldPoint], start: u32, end: u32) -> &[HeldPoint] {
    let upper = points.partition_point(|p| p.tick < end);
    let first_after_start = points[..upper].partition_point(|p| p.tick <= start);
    &points[first_after_start.saturating_sub(1)..upper]
}

/// Attribution boundaries do not divide the performance region: one validated
/// channel timeline still generates each target curve once. Only report spans
/// split when the retained notes have different original adapter owners.
pub(crate) fn original_track_id<'a>(track: &'a ProjectedTrack, note: &'a ProjectedNote) -> &'a str {
    note.source_evidence
        .as_ref()
        .and_then(|e| e.origin.as_ref())
        .map_or(track.source_track_id.as_str(), |origin| {
            origin.track_id.as_str()
        })
}

pub(crate) fn original_note_id<'a>(
    note: &'a ProjectedNote,
    performance: &'a PerformanceNote,
) -> &'a str {
    note.source_evidence
        .as_ref()
        .map_or(performance.source_id.as_str(), |e| e.note_id.as_str())
}

pub(crate) struct Attribution {
    pub track_id: String,
    pub start: u32,
    pub end: u32,
    pub note_ids: Vec<String>,
}

pub(crate) fn attributions(
    track: &ProjectedTrack,
    notes: &[(usize, &ProjectedNote)],
    start: u32,
    end: u32,
    budget: &mut Budget,
) -> Result<Vec<Attribution>, String> {
    let first = notes.partition_point(|(_, n)| {
        u64::from(n.onset_ticks) + u64::from(n.duration_ticks) <= u64::from(start)
    });
    let last = notes.partition_point(|(_, n)| n.onset_ticks < end);
    budget.work(64 + last - first)?;
    let mut result = Vec::<Attribution>::new();
    for (_, note) in &notes[first..last] {
        let Some(performance) = &note.performance else {
            continue;
        };
        let source_track = note
            .source_evidence
            .as_ref()
            .and_then(|e| e.origin.as_ref())
            .map_or(track.source_track_id.as_str(), |origin| {
                origin.track_id.as_str()
            });
        if result
            .last()
            .is_none_or(|previous| previous.track_id != source_track)
        {
            let boundary = if let Some(previous) = result.last_mut() {
                previous.end = note.onset_ticks;
                note.onset_ticks
            } else {
                start
            };
            budget.references(1)?;
            budget.text(source_track.len())?;
            result.push(Attribution {
                track_id: source_track.to_string(),
                start: boundary,
                end,
                note_ids: Vec::new(),
            });
        }
        let id = note
            .source_evidence
            .as_ref()
            .map_or(performance.source_id.as_str(), |e| e.note_id.as_str());
        budget.references(1)?;
        budget.text(id.len())?;
        result.last_mut().unwrap().note_ids.push(id.to_string());
    }
    Ok(result)
}

pub(crate) fn issues_in_span(
    issues: &[PerformanceIssue],
    start: u32,
    end: u32,
) -> &[PerformanceIssue] {
    let first = issues.partition_point(|p| p.tick < start);
    let last = issues.partition_point(|p| p.tick < end);
    &issues[first..last]
}

/// Historical opaque hazards travel in the unknown held state's contributor
/// IDs. Instant issues belong only to notes sounding at the issue's own tick.
/// Group equal reasons instead of cloning all note IDs for every event.
pub(crate) fn issue_reports(
    track: &ProjectedTrack,
    target_track: usize,
    notes: &[(usize, &ProjectedNote)],
    issues: &[PerformanceIssue],
    budget: &mut Budget,
) -> Result<Vec<PerformanceTransfer>, String> {
    budget.work(issues.len())?;
    let mut groups = BTreeMap::<
        (String, Dimension, &str),
        (u32, u32, Vec<String>, std::collections::BTreeSet<String>),
    >::new();
    for issue in issues {
        let end = issue
            .tick
            .checked_add(1)
            .ok_or("Performance issue endpoint overflows source ticks")?;
        for attribution in attributions(track, notes, issue.tick, end, budget)? {
            budget.references(1)?;
            let entry = groups
                .entry((attribution.track_id, issue.dimension, issue.reason.as_str()))
                .or_insert((issue.tick, issue.tick, Vec::new(), Default::default()));
            entry.1 = issue.tick;
            budget.ids(&issue.source_ids)?;
            entry.2.extend(issue.source_ids.iter().cloned());
            entry.3.extend(attribution.note_ids);
        }
    }
    let mut reports = Vec::new();
    for ((track_id, dimension, reason), (start, last, mut source_ids, note_ids)) in groups {
        let end = last
            .checked_add(1)
            .ok_or("Performance issue endpoint overflows source ticks")?;
        source_ids.sort();
        source_ids.dedup();
        budget.text(reason.len())?;
        reports.push(PerformanceTransfer {
            intensity: None,
            track_id,
            target_track: Some(target_track),
            dimension,
            start_tick: start,
            end_tick: end,
            source_ids,
            note_ids: note_ids.into_iter().collect(),
            status: TransferStatus::Unsupported,
            message: reason.into(),
        });
    }
    Ok(reports)
}

/// Preserve specific MIDI diagnostics that have no sounding-note attribution.
/// The normalization table owns their coordinates, including controller-only
/// tracks and notes routed to another destination. Never infer an event owner
/// from a neighboring note, destination lane, channel number or source-ID text.
pub(crate) fn source_only_issue_reports(
    performance: &PerformanceIndex,
    transfers: &[PerformanceTransfer],
    ppq: u16,
    budget: &mut Budget,
) -> Result<Vec<PerformanceTransfer>, String> {
    let mut reports = source_only_score_issues(performance, transfers, ppq, budget)?;
    budget.work(performance.channels.len())?;
    if performance
        .channels
        .values()
        .all(|channel| channel.issues.is_empty())
    {
        return Ok(reports);
    }
    let mut reported = std::collections::BTreeSet::new();
    budget.work(transfers.len())?;
    for transfer in transfers
        .iter()
        .filter(|t| t.intensity.is_none() && t.status == TransferStatus::Unsupported)
    {
        for id in &transfer.source_ids {
            budget.references(1)?;
            budget.work(id.len().saturating_add(transfer.message.len()))?;
            reported.insert((id.as_str(), transfer.dimension, transfer.message.as_str()));
        }
    }
    for channel in performance.channels.values() {
        budget.work(channel.issues.len())?;
        for issue in &channel.issues {
            // Input::ids stores the causing event first, followed by contextual
            // contributors such as the original port declaration.
            let id = issue
                .source_ids
                .first()
                .ok_or("MIDI_PERFORMANCE_OWNER_MISSING: source diagnostic has no causing event")?;
            budget.references(1)?;
            budget.work(id.len().saturating_add(issue.reason.len()))?;
            if !reported.insert((id.as_str(), issue.dimension, issue.reason.as_str())) {
                continue;
            }
            let event = performance.events.get(id).ok_or(
                "MIDI_PERFORMANCE_OWNER_MISSING: source diagnostic has no original event owner",
            )?;
            let end_tick = event
                .tick
                .checked_add(1)
                .ok_or("Performance issue endpoint overflows source ticks")?;
            budget.references(1)?;
            budget.ids(&issue.source_ids)?;
            budget.text(event.track_id.len().saturating_add(issue.reason.len()))?;
            reports.push(PerformanceTransfer {
                intensity: None,
                track_id: event.track_id.clone(),
                target_track: None,
                dimension: issue.dimension,
                start_tick: event.tick,
                end_tick,
                source_ids: issue.source_ids.clone(),
                note_ids: Vec::new(),
                status: TransferStatus::Unsupported,
                message: issue.reason.clone(),
            });
        }
    }
    Ok(reports)
}

/// Both adapters retain the same localized score issue coordinates and reasons.
pub(crate) fn intensity_issue_reports(
    track: &ProjectedTrack,
    note: &ProjectedNote,
    target_track: usize,
    ppq: u16,
    terminal: bool,
    budget: &mut Budget,
) -> Result<Vec<PerformanceTransfer>, String> {
    use crate::engine::score_intensity::{self as si, source::time};
    let Some(owner) = &note.performance else {
        return Ok(Vec::new());
    };
    let Some(intensity) = &owner.intensity else {
        return Ok(Vec::new());
    };
    let start = time(i64::from(note.onset_ticks), ppq)?;
    let end = time(
        i64::from(note.onset_ticks) + i64::from(note.duration_ticks),
        ppq,
    )?;
    budget.work(
        intensity
            .issues
            .len()
            .saturating_add(intensity.timeline.issues.len()),
    )?;
    let mut reports = Vec::new();
    for issue in intensity
        .issues
        .iter()
        .chain(&intensity.timeline.issues)
        .filter(|i| si::intersects(i.start, i.end, start, end, terminal))
    {
        let lo = issue.start.max(start);
        let hi = issue.end.min(end);
        let terminal_endpoint = lo == end && lo == hi;
        let source_ids =
            budget.collect_ids(issue.provenance.evidence.iter().flat_map(|e| &e.source_ids))?;
        let note_id = (!terminal_endpoint).then(|| original_note_id(note, owner));
        let track_id = original_track_id(track, note);
        budget.report_text(track_id, note_id, &issue.message)?;
        let evidence = issue_evidence(issue, lo, hi, terminal_endpoint, budget)?;
        let note_ids = note_id.into_iter().map(str::to_string).collect();
        let (start_tick, end_tick) = exact_source_tick_bounds(lo, hi, ppq)?;
        reports.push(PerformanceTransfer {
            intensity: Some(evidence),
            track_id: original_track_id(track, note).to_string(),
            target_track: (!terminal_endpoint).then_some(target_track),
            dimension: Dimension::LinearGain,
            start_tick,
            end_tick,
            source_ids,
            note_ids,
            status: TransferStatus::Unsupported,
            message: issue.message.clone(),
        });
    }
    Ok(reports)
}

/// A declaration exactly at the final sounding endpoint owns a terminal point,
/// never the preceding half-open note. Both adapters retain that distinction.
pub(crate) fn intensity_terminal_report(
    track: &ProjectedTrack,
    target_track: usize,
    ppq: u16,
    emitted: bool,
    budget: &mut Budget,
) -> Result<Option<PerformanceTransfer>, String> {
    use crate::engine::score_intensity::{self as si, source::time};
    let Some(note) = track.notes.last() else {
        return Ok(None);
    };
    let Some(intensity) = note.performance.as_ref().and_then(|p| p.intensity.as_ref()) else {
        return Ok(None);
    };
    let tick = note
        .onset_ticks
        .checked_add(note.duration_ticks)
        .ok_or("Performance note endpoint overflows source ticks")?;
    let end = time(i64::from(tick), ppq)?;
    budget.work(
        usize::BITS as usize - intensity.timeline.segments.len().leading_zeros() as usize + 1,
    )?;
    let Some(segment) = intensity
        .timeline
        .segment_at(end)
        .filter(|s| s.start == end)
    else {
        return Ok(None);
    };
    let Some(provenance) = &segment.provenance else {
        return Ok(None);
    };
    let source_ids = budget.collect_ids(provenance.evidence.iter().flat_map(|e| &e.source_ids))?;
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TerminalEvidence<'a> {
        policy: &'static str,
        start: si::Time,
        end: si::Time,
        terminal_endpoint: bool,
        provenance: &'a si::Provenance,
        curve: &'a si::Curve,
    }
    let evidence = budget.json(&TerminalEvidence {
        policy: si::POLICY,
        start: end,
        end,
        terminal_endpoint: true,
        provenance,
        curve: &segment.curve,
    })?;
    budget.references(1)?;
    budget.text(original_track_id(track, note).len().saturating_add(200))?;
    Ok(Some(PerformanceTransfer {
        intensity:Some(evidence),track_id:original_track_id(track,note).to_string(),target_track:Some(target_track),dimension:Dimension::LinearGain,
        start_tick:tick,end_tick:tick,source_ids,note_ids:Vec::new(),
        status:if emitted {TransferStatus::RepresentationLimit} else {TransferStatus::Unsupported},
        message:if emitted {"Explicit terminal score endpoint emitted as a DYN point with no positive sounding interval; no mapped span or preceding note ownership is claimed"} else {"Explicit terminal score endpoint retained without an editable target point; no preceding note owns this declaration"}.into(),
    }))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueEvidence<'a> {
    policy: &'static str,
    provenance: &'a crate::engine::score_intensity::Provenance,
    start: crate::engine::score_intensity::Time,
    end: crate::engine::score_intensity::Time,
    terminal_endpoint: bool,
}

fn issue_evidence(
    issue: &crate::engine::score_intensity::Issue,
    start: crate::engine::score_intensity::Time,
    end: crate::engine::score_intensity::Time,
    terminal_endpoint: bool,
    budget: &mut Budget,
) -> Result<serde_json::Value, String> {
    budget.json(&IssueEvidence {
        policy: crate::engine::score_intensity::POLICY,
        provenance: &issue.provenance,
        start,
        end,
        terminal_endpoint,
    })
}

/// Subtract exact issue coverage, not all reports that mention the same field.
/// A repeated source ID can own several disjoint performed diagnostics.
fn source_only_score_issues(
    performance: &PerformanceIndex,
    transfers: &[PerformanceTransfer],
    ppq: u16,
    budget: &mut Budget,
) -> Result<Vec<PerformanceTransfer>, String> {
    use crate::engine::score_intensity::{self as si, Time};
    type Key<'a> = (&'a si::ScoreVoice, &'a str, &'a str, u64, u64);
    let mut coverage = BTreeMap::<Key<'_>, Vec<&PerformanceTransfer>>::new();
    budget.work(transfers.len())?;
    for transfer in transfers {
        if transfer.status != TransferStatus::Unsupported
            || transfer.dimension != Dimension::LinearGain
        {
            continue;
        }
        let Some(provenance) = transfer
            .intensity
            .as_ref()
            .and_then(|v| v.get("provenance"))
            .filter(|p| p.is_object())
        else {
            continue;
        };
        let (Some(occurrence), Some(pass)) = (
            provenance["occurrence"].as_u64(),
            provenance["repeat_pass"].as_u64(),
        ) else {
            continue;
        };
        for voice in performance
            .score_track_owners
            .get(&transfer.track_id)
            .into_iter()
            .flatten()
        {
            for id in &transfer.source_ids {
                budget.work(1 + id.len() / 64 + transfer.message.len() / 64)?;
                budget.references(1)?;
                coverage
                    .entry((voice, id, &transfer.message, occurrence, pass))
                    .or_default()
                    .push(transfer);
            }
        }
    }
    let mut reports = Vec::new();
    for owner in &performance.score_issues {
        budget.work(owner.timeline.issues.len())?;
        for issue in &owner.timeline.issues {
            if !issue.provenance.scope.applies(&owner.voice)
                && !matches!(&issue.provenance.scope, si::Scope::Unsupported { part, .. } if part == &owner.voice.part)
            {
                return Err("Score diagnostic no longer belongs to its typed source owner".into());
            }
            let source_ids =
                budget.collect_ids(issue.provenance.evidence.iter().flat_map(|e| &e.source_ids))?;
            let mut covered = Vec::new();
            if let Some(candidates) = source_ids.first().and_then(|id| {
                coverage.get(&(
                    &owner.voice,
                    id.as_str(),
                    issue.message.as_str(),
                    u64::from(issue.provenance.occurrence),
                    u64::from(issue.provenance.repeat_pass),
                ))
            }) {
                // This comparison is only needed for candidate issue reports.
                // Count the borrowed payload before constructing this one value.
                let before = budget.bytes;
                let provenance = budget.json(&issue.provenance)?;
                let comparison_work = (budget.bytes - before) / 64 + 1;
                for candidate in candidates {
                    budget.work(comparison_work)?;
                    let evidence = candidate.intensity.as_ref().unwrap();
                    if evidence.get("provenance") != Some(&provenance) {
                        continue;
                    }
                    budget.references(1)?;
                    covered.push((
                        evidence_time(&evidence["start"])?,
                        evidence_time(&evidence["end"])?,
                    ));
                }
            }
            covered.sort_unstable();
            let mut remaining = Vec::new();
            if issue.start == issue.end {
                if !covered
                    .iter()
                    .any(|(lo, hi)| *lo <= issue.start && issue.start <= *hi)
                {
                    remaining.push((issue.start, issue.end));
                }
            } else {
                let mut cursor = issue.start;
                for (lo, hi) in covered {
                    let lo = lo.max(issue.start);
                    let hi = hi.min(issue.end);
                    if lo >= hi || hi <= cursor {
                        continue;
                    }
                    if cursor < lo {
                        remaining.push((cursor, lo));
                    }
                    cursor = cursor.max(hi);
                }
                if cursor < issue.end {
                    remaining.push((cursor, issue.end));
                }
            }
            for (lo, hi) in remaining {
                budget.ids(&source_ids)?;
                budget.report_text(&owner.track_id, None, &issue.message)?;
                let evidence = issue_evidence(
                    issue,
                    lo,
                    hi,
                    lo == hi && Some(hi) == owner.terminal,
                    budget,
                )?;
                let (start_tick, end_tick) = exact_source_tick_bounds(lo, hi, ppq)?;
                reports.push(PerformanceTransfer {
                    intensity: Some(evidence),
                    track_id: owner.track_id.clone(),
                    target_track: None,
                    dimension: Dimension::LinearGain,
                    start_tick,
                    end_tick,
                    source_ids: source_ids.clone(),
                    note_ids: Vec::new(),
                    status: TransferStatus::Unsupported,
                    message: issue.message.clone(),
                });
            }
        }
    }
    fn evidence_time(value: &serde_json::Value) -> Result<Time, String> {
        let number = |key| {
            value[key]
                .as_i64()
                .map(i128::from)
                .or_else(|| value[key].as_u64().map(i128::from))
                .or_else(|| {
                    let text = value[key].as_str()?;
                    let digits = text.strip_prefix('-').unwrap_or(text);
                    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| text.parse::<i128>().ok())
                        .flatten()
                })
                .ok_or_else(|| "Score issue report lost its exact rational coordinates".to_string())
        };
        Time::wide(number("numerator")?, number("denominator")?)
    }
    Ok(reports)
}

#[cfg(test)]
pub(crate) mod second_review_tests {
    use super::*;

    pub(crate) fn project(large_id: bool) -> crate::engine::projection::ProjectedProject {
        let source = crate::engine::musicxml::parse(br#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><dynamics><p/></dynamics></direction-type></direction><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><lyric><text>la</text></lyric></note></measure></part></score-partwise>"#).unwrap();
        let outcome = crate::engine::convert::convert_midi_with_target(
            &source,
            "english",
            None,
            crate::engine::target::ExportTarget::Ustx,
        );
        assert!(outcome.ok, "{:?}", outcome.msg);
        let mut project = outcome.svp.unwrap();
        let intensity = Arc::make_mut(
            project.tracks[0].notes[0]
                .performance
                .as_mut()
                .unwrap()
                .intensity
                .as_mut()
                .unwrap(),
        );
        let provenance = Arc::make_mut(&mut intensity.timeline)
            .segments
            .iter_mut()
            .find_map(|s| s.provenance.as_mut())
            .unwrap();
        if large_id {
            provenance.evidence[0].source_ids = vec!["oversized-field-".repeat(1024)];
        } else {
            // Quotes and newlines exercise the encoded size, not just raw UTF-8 length.
            provenance.evidence[0]
                .raw_fields
                .insert("adversarial".into(), "\"\n".repeat(8192));
        }
        project
    }

    #[test]
    fn borrowed_json_refuses_before_value_serialization_and_preserves_shape() {
        struct Traced<'a> {
            calls: &'a std::cell::Cell<usize>,
            text: &'a str,
        }
        impl serde::Serialize for Traced<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.calls.set(self.calls.get() + 1);
                serializer.serialize_str(self.text)
            }
        }
        let calls = std::cell::Cell::new(0);
        let text = "\"\n".repeat(4096);
        let value = Traced {
            calls: &calls,
            text: &text,
        };
        let error = Budget::with_bytes(32).json(&value).unwrap_err();
        assert!(error.contains("MIDI_PERFORMANCE_LIMIT"));
        assert_eq!(
            calls.get(),
            1,
            "refusal never starts the allocating Value pass"
        );
        let ordinary = serde_json::json!({"policy":"verse-score-intensity-v1","start":{"numerator":1,"denominator":3},"provenance":[],"curve":null});
        assert_eq!(Budget::default().json(&ordinary).unwrap(), ordinary);
    }

    #[test]
    fn source_issue_coverage_preserves_existing_wide_integer_time_encoding() {
        use crate::engine::score_intensity::Time;
        let source = crate::engine::musicxml::parse(br#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes><direction><direction-type><wedge type="stop"/></direction-type></direction><note><pitch><step>C</step><octave>4</octave></pitch><duration>960</duration><lyric><text>la</text></lyric></note></measure></part></score-partwise>"#).unwrap();
        let mut raw = crate::engine::performance::normalize(&source).unwrap();
        let owner = &mut raw.score_issues[0];
        let issue = &mut Arc::make_mut(&mut owner.timeline).issues[0];
        let scale = 100_000_000_000_000_000_000i128;
        issue.start = Time::wide(scale + 1, scale).unwrap();
        issue.end = Time::integer(2);
        let boundary = Time::wide(scale + 3, scale).unwrap();
        let mut budget = Budget::default();
        let evidence = issue_evidence(issue, issue.start, boundary, false, &mut budget).unwrap();
        assert!(evidence["start"]["numerator"].is_string());
        assert!(evidence["start"]["denominator"].is_string());
        let transfer = PerformanceTransfer {
            intensity: Some(evidence),
            track_id: owner.track_id.clone(),
            target_track: Some(0),
            dimension: Dimension::LinearGain,
            start_tick: 480,
            end_tick: 481,
            source_ids: issue.provenance.evidence[0].source_ids.clone(),
            note_ids: vec!["covered-note".into()],
            status: TransferStatus::Unsupported,
            message: issue.message.clone(),
        };
        let reports = source_only_score_issues(
            &raw,
            std::slice::from_ref(&transfer),
            480,
            &mut Budget::default(),
        )
        .unwrap();
        assert_eq!(reports.len(), 1);
        let remaining = reports[0].intensity.as_ref().unwrap();
        assert_eq!(remaining["start"], serde_json::to_value(boundary).unwrap());
        assert_eq!(
            remaining["end"],
            serde_json::to_value(Time::integer(2)).unwrap()
        );
        assert_eq!(reports[0].target_track, None);
        assert!(reports[0].note_ids.is_empty());
        for invalid in [
            "0",
            "-1",
            "+1",
            "1.0",
            "999999999999999999999999999999999999999999999",
        ] {
            let mut invalid_transfer = transfer.clone();
            invalid_transfer.intensity.as_mut().unwrap()["end"]["denominator"] = invalid.into();
            assert!(
                source_only_score_issues(&raw, &[invalid_transfer], 480, &mut Budget::default())
                    .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn id_copies_and_repeated_evidence_share_one_small_report_budget() {
        let ids = ["x".repeat(64), "y".repeat(64)];
        assert!(Budget::with_bytes(100)
            .collect_ids(&ids)
            .unwrap_err()
            .contains("MIDI_PERFORMANCE_LIMIT"));
        let mut budget = Budget::with_bytes(100);
        let value = "x".repeat(60);
        assert_eq!(
            budget.json(&value).unwrap(),
            serde_json::Value::String(value.clone())
        );
        assert!(budget
            .json(&value)
            .unwrap_err()
            .contains("MIDI_PERFORMANCE_LIMIT"));
    }
}
