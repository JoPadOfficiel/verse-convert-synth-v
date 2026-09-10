//! Unsupported expression reporting with original source attribution.
use crate::engine::projection::ProjectedProject;

pub(super) fn report(
    project: &ProjectedProject,
) -> Result<Vec<crate::engine::performance::PerformanceTransfer>, String> {
    report_bounded(project, &mut super::super::performance::Budget::default())
}

fn report_bounded(
    project: &ProjectedProject,
    budget: &mut super::super::performance::Budget,
) -> Result<Vec<crate::engine::performance::PerformanceTransfer>, String> {
    use super::super::performance as traversal;
    use crate::engine::performance::{Dimension, PerformanceTransfer, TransferStatus};
    let mut reports = Vec::new();
    for (target_track, track) in project.tracks.iter().enumerate() {
        for (note_index, note) in track.notes.iter().enumerate() {
            let Some(owner) = &note.performance else {
                continue;
            };
            let Some(intensity) = &owner.intensity else {
                continue;
            };
            let start = crate::engine::score_intensity::source::time(
                i64::from(note.onset_ticks),
                project.ticks_per_beat,
            )?;
            let end = crate::engine::score_intensity::source::time(
                i64::from(note.onset_ticks) + i64::from(note.duration_ticks),
                project.ticks_per_beat,
            )?;
            // Two indexed range searches share the full immutable timeline;
            // only intersecting segments are traversed or serialized per note.
            let depth = usize::BITS as usize
                - intensity.timeline.segments.len().leading_zeros() as usize
                + 1;
            budget.work(depth.saturating_mul(2))?;
            reports.extend(traversal::intensity_issue_reports(
                track,
                note,
                target_track,
                project.ticks_per_beat,
                note_index + 1 == track.notes.len(),
                budget,
            )?);
            let first = intensity
                .timeline
                .segments
                .partition_point(|s| s.end <= start);
            let last = intensity
                .timeline
                .segments
                .partition_point(|s| s.start < end)
                .max(first);
            let segments = &intensity.timeline.segments[first..last];
            budget.work(segments.len().saturating_mul(2))?;
            let used_provenance = intensity
                .provenance
                .iter()
                .chain(segments.iter().filter_map(|s| s.provenance.as_ref()));
            let provenance_count = used_provenance.clone().count();
            budget.references(provenance_count)?;
            budget.text(provenance_count.saturating_mul(std::mem::size_of::<
                &crate::engine::score_intensity::Provenance,
            >()))?;
            let mut provenance = Vec::with_capacity(provenance_count);
            provenance.extend(used_provenance);
            let source_ids = budget.collect_ids(
                provenance
                    .iter()
                    .flat_map(|p| p.evidence.iter().flat_map(|e| &e.source_ids)),
            )?;
            if !source_ids.is_empty() {
                #[derive(serde::Serialize)]
                struct ScoreEvidence<'a> {
                    policy: &'static str,
                    start: crate::engine::score_intensity::Time,
                    end: crate::engine::score_intensity::Time,
                    provenance: &'a [&'a crate::engine::score_intensity::Provenance],
                    segments: &'a [crate::engine::score_intensity::Segment],
                }
                budget.report_text(traversal::original_track_id(track, note), Some(traversal::original_note_id(note, owner)),
                    "Synthesizer V score/attack intensity transfer is unsupported; exact neutral intent is retained and no loudness automation is claimed")?;
                let evidence = budget.json(&ScoreEvidence {
                    policy: crate::engine::score_intensity::POLICY,
                    start,
                    end,
                    provenance: &provenance,
                    segments,
                })?;
                reports.push(PerformanceTransfer { intensity:Some(evidence),track_id:traversal::original_track_id(track,note).to_string(),target_track:Some(target_track),dimension:Dimension::LinearGain,
                start_tick:note.onset_ticks,end_tick:note.onset_ticks.checked_add(note.duration_ticks).ok_or("Performance note endpoint overflows source ticks")?,source_ids,note_ids:vec![traversal::original_note_id(note,owner).to_string()],status:TransferStatus::Unsupported,
                message:"Synthesizer V score/attack intensity transfer is unsupported; exact neutral intent is retained and no loudness automation is claimed".into() });
            }
        }
        if let Some(terminal) = traversal::intensity_terminal_report(
            track,
            target_track,
            project.ticks_per_beat,
            false,
            budget,
        )? {
            reports.push(terminal);
        }
        if !track.notes.iter().any(|n| n.performance.is_some()) {
            continue;
        }
        let regions = traversal::regions(track, budget)?;
        for region in &regions.spans {
            budget.work(128)?;
            let notes = &regions.notes[region.notes.clone()];
            let Some(owner) = notes[0].1.performance.as_ref() else {
                continue;
            };
            let Some(timeline) = owner.channel() else {
                continue;
            };
            for (dimension, points) in [
                (Dimension::PitchCents, &timeline.pitch_cents),
                (Dimension::LinearGain, &timeline.linear_gain),
            ] {
                let points = traversal::points_in_span(points, region.start, region.end);
                budget.work(points.len())?;
                for (point_index, point) in points.iter().enumerate() {
                    let start = point.tick.max(region.start);
                    let end = points
                        .get(point_index + 1)
                        .map_or(region.end, |p| p.tick.min(region.end));
                    if start >= end {
                        continue;
                    }
                    for attribution in traversal::attributions(track, notes, start, end, budget)? {
                        budget.references(1)?;
                        budget.ids(&point.source_ids)?;
                        budget.text(200 + attribution.track_id.len())?;
                        reports.push(PerformanceTransfer { intensity: None,
                            track_id: attribution.track_id, target_track: Some(target_track), dimension,
                            start_tick: attribution.start, end_tick: attribution.end, source_ids: point.source_ids.clone(),
                            note_ids: attribution.note_ids, status: TransferStatus::Unsupported,
                            message: "Synthesizer V editable performance transfer is unsupported; the raw source remains retained, but no pitchDelta or loudness automation was transferred.".into(),
                        });
                    }
                }
            }
            reports.extend(traversal::issue_reports(
                track,
                target_track,
                notes,
                traversal::issues_in_span(&timeline.issues, region.start, region.end),
                budget,
            )?);
        }
    }
    Ok(reports)
}

#[cfg(test)]
mod second_review_tests {
    use super::*;
    use crate::engine::target::performance::{self as traversal, Budget};
    #[test]
    fn svp_reporting_preflights_large_borrowed_evidence_and_ids() {
        for large_id in [false, true] {
            let project = traversal::second_review_tests::project(large_id);
            let before = project.clone();
            let error = report_bounded(&project, &mut Budget::with_bytes(256)).unwrap_err();
            assert!(error.contains("MIDI_PERFORMANCE_LIMIT"), "{error}");
            let reports = report(&project).unwrap();
            assert!(!reports.is_empty());
            assert!(reports
                .iter()
                .all(|r| r.status == crate::engine::performance::TransferStatus::Unsupported));
            assert_eq!(project, before);
        }
    }
}

#[cfg(test)]
mod fourth_review_tests {
    use super::*;
    use crate::engine::score_intensity::{Curve, Segment, Time};
    use crate::engine::target::performance::{self as traversal, Budget};
    use std::sync::Arc;

    #[test]
    fn svp_charges_local_visits_and_refuses_actual_repeated_segment_work() {
        let mut project = traversal::second_review_tests::project(false);
        let note = &mut project.tracks[0].notes[0];
        let intensity = Arc::make_mut(
            note.performance
                .as_mut()
                .unwrap()
                .intensity
                .as_mut()
                .unwrap(),
        );
        Arc::make_mut(&mut intensity.timeline).segments = (0..4096)
            .map(|i| Segment {
                start: Time::integer(i),
                end: Time::integer(i + 1),
                curve: Curve::Absent,
                attack_only: false,
                provenance: None,
            })
            .collect();
        // A one-quarter note intersects one segment, irrespective of the total.
        assert!(
            report_bounded(&project, &mut Budget::with_limits(4096, 256))
                .unwrap()
                .is_empty()
        );
        project.tracks[0].notes[0].duration_ticks = 4096 * 480;
        let intensity = Arc::make_mut(
            project.tracks[0].notes[0]
                .performance
                .as_mut()
                .unwrap()
                .intensity
                .as_mut()
                .unwrap(),
        );
        intensity.end = Time::integer(4096);
        let shared = Arc::clone(&intensity.timeline);
        assert!(
            report_bounded(&project, &mut Budget::with_limits(32 * 1024 * 1024, 128))
                .unwrap_err()
                .contains("MIDI_PERFORMANCE_LIMIT")
        );
        project.tracks = vec![project.tracks[0].clone(); 512];
        let error = report(&project).unwrap_err();
        assert!(error.contains("MIDI_PERFORMANCE_LIMIT"), "{error}");
        assert!(
            project.tracks.iter().all(|t| Arc::ptr_eq(
                &shared,
                &t.notes[0]
                    .performance
                    .as_ref()
                    .unwrap()
                    .intensity
                    .as_ref()
                    .unwrap()
                    .timeline
            )),
            "reporting never clones shared timeline payloads"
        );
    }
}
