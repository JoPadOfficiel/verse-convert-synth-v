//! Shared bounded traversal; source values stay target-neutral here.
use crate::engine::performance::{
    Dimension, HeldPoint, PerformanceIssue, PerformanceTransfer, TransferStatus,
};
use crate::engine::projection::{ProjectedNote, ProjectedTrack};
use std::collections::BTreeMap;
use std::ops::Range;

/// Per adaptation/report pass. Charge BEFORE copying references or text.
/// A refusal is deterministic and cannot publish a partial performance report.
#[derive(Default)]
pub(crate) struct Budget {
    work: usize,
    references: usize,
    bytes: usize,
}
impl Budget {
    pub fn work(&mut self, count: usize) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(count)
            .ok_or("MIDI_PERFORMANCE_LIMIT: work overflow")?;
        if self.work > 2_000_000 {
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
        if self.bytes > 32 * 1024 * 1024 {
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
            if previous.performance.as_ref().map(|p| p.key)
                == note.performance.as_ref().map(|p| p.key)
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

pub(crate) fn note_ids(
    notes: &[(usize, &ProjectedNote)],
    start: u32,
    end: u32,
    budget: &mut Budget,
) -> Result<Vec<String>, String> {
    let first = notes.partition_point(|(_, n)| {
        u64::from(n.onset_ticks) + u64::from(n.duration_ticks) <= u64::from(start)
    });
    let last = notes.partition_point(|(_, n)| n.onset_ticks < end);
    budget.work(64 + last - first)?;
    let mut result = Vec::new();
    for (_, note) in &notes[first..last] {
        if let Some(p) = &note.performance {
            budget.references(1)?;
            budget.text(p.source_id.len())?;
            result.push(p.source_id.clone());
        }
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
        (Dimension, &str),
        (u32, u32, Vec<String>, std::collections::BTreeSet<String>),
    >::new();
    for issue in issues {
        budget.references(1)?;
        let entry = groups
            .entry((issue.dimension, issue.reason.as_str()))
            .or_insert((issue.tick, issue.tick, Vec::new(), Default::default()));
        entry.1 = issue.tick;
        budget.ids(&issue.source_ids)?;
        entry.2.extend(issue.source_ids.iter().cloned());
        let end = issue
            .tick
            .checked_add(1)
            .ok_or("Performance issue endpoint overflows source ticks")?;
        entry.3.extend(note_ids(notes, issue.tick, end, budget)?);
    }
    let mut reports = Vec::new();
    for ((dimension, reason), (start, last, mut source_ids, note_ids)) in groups {
        let end = last
            .checked_add(1)
            .ok_or("Performance issue endpoint overflows source ticks")?;
        source_ids.sort();
        source_ids.dedup();
        budget.text(reason.len())?;
        reports.push(PerformanceTransfer {
            track_id: track.source_track_id.clone(),
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
