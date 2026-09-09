//! OpenUtau performance adaptation. Consumer revision:
//! 3f213e8993ca792c3e6f8958c92ab27eae78eac5, verified against the installed
//! Core assembly's informational version and SHA-256 (see EXP-002 evidence).
//! Marketing/release labels are not the code pin.
//! Format/USTx.cs:42-44: PITD cents [-1200,1200], DYN 0.1 dB [-240,120].
//! Ustx/UCurve.cs:57-68: linear, integer-rounded samples; default outside xs.
//! Render/RenderPhrase.cs:283-285,448-478,522-529: 5-tick samples from
//! pitchStart = phrase position - part position - first phone leading.
//! The origin depends on the phonemizer; it is NOT necessarily tick 0 mod 5.
//! RenderPhrase.cs:337 floors the flat note base's start sample index, so a
//! nominal note boundary can step up to four ticks early on that same grid.
use super::{exact_ustx_ticks, UstxCurve};
use crate::engine::performance::{Dimension, PerformanceTransfer, TransferStatus};
use crate::engine::projection::{ProjectedProject, ProjectedTrack};
use crate::engine::target::performance::{self as traversal, Budget};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Adapted {
    pub curves: Vec<UstxCurve>,
    pub flat_notes: BTreeSet<usize>,
    pub transfers: Vec<PerformanceTransfer>,
}

fn target_value(value: f64, dimension: Dimension) -> Result<i32, String> {
    if !value.is_finite() {
        return Err("Nonfinite performance value.".into());
    }
    match dimension {
        Dimension::PitchCents => {
            if !(-1200.0..=1200.0).contains(&value) {
                return Err(format!(
                    "Pitch deviation {value} cents exceeds PITD [-1200,1200]; it was not clipped."
                ));
            }
            Ok(value.round() as i32)
        }
        Dimension::LinearGain => {
            if value == 0.0 {
                return Ok(-240);
            }
            if value < 0.0 {
                return Err("Negative source gain is unsupported.".into());
            }
            let dyn_value = (200.0 * value.log10()).round();
            if dyn_value <= -240.0 || dyn_value > 120.0 {
                return Err(format!("Positive gain {value} is outside DYN's usable range; it was neither clipped nor converted to the exact-mute sentinel."));
            }
            Ok(dyn_value as i32)
        }
        Dimension::Other => unreachable!("only pitch and gain have target curves"),
    }
}

/// Source-tempo integral, indexed once per project. Template offsets are ms.
pub(super) struct Clock {
    segments: Vec<(u32, f64, f64)>,
    ppq: u16,
}
impl Clock {
    pub fn new(project: &ProjectedProject, budget: &mut Budget) -> Result<Self, String> {
        if !project
            .tracks
            .iter()
            .flat_map(|t| &t.notes)
            .any(|n| n.performance.is_some())
        {
            return Ok(Self {
                segments: vec![(0, 0.0, 120.0)],
                ppq: project.ticks_per_beat,
            });
        }
        budget.work(project.tempos.len())?;
        let mut sorted = BTreeMap::new();
        for tempo in project.tempos_in_discovery_order() {
            sorted.insert(tempo.tick, tempo.bpm);
        }
        let mut segments = vec![(0, 0.0, 120.0)];
        for (tick, bpm) in sorted {
            if !bpm.is_finite() || bpm <= 0.0 {
                return Err("Performance tempo must be finite and positive".into());
            }
            let (previous, ms, rate) = *segments.last().unwrap();
            let at = ms
                + f64::from(tick - previous) * 60000.0 / (rate * f64::from(project.ticks_per_beat));
            segments.push((tick, at, bpm));
        }
        Ok(Self {
            segments,
            ppq: project.ticks_per_beat,
        })
    }
    fn ms(&self, tick: u32) -> f64 {
        let i = self.segments.partition_point(|(t, _, _)| *t <= tick) - 1;
        let (start, ms, bpm) = self.segments[i];
        ms + f64::from(tick - start) * 60000.0 / (bpm * f64::from(self.ppq))
    }
}

pub(super) fn adapt(
    track: &ProjectedTrack,
    ppq: u16,
    clock: &Clock,
    target_track: usize,
    budget: &mut Budget,
) -> Result<Adapted, String> {
    let mut result = Adapted::default();
    if !track.notes.iter().any(|note| note.performance.is_some()) {
        return Ok(result);
    }
    if ppq == 0 {
        return Err("MIDI PPQ division must be non-zero".into());
    }
    let regions = traversal::regions(track, budget)?;
    let mut pitch = BTreeMap::new();
    let mut gain = BTreeMap::new();
    let mut has_pitch = false;
    let mut has_gain = false;
    for (region_index, region) in regions.spans.iter().enumerate() {
        budget.work(128)?; // binary searches and region bookkeeping
        let notes = &regions.notes[region.notes.clone()];
        let owner = notes[0].1.performance.as_ref();
        let mut unsafe_base = false;
        if region.notes.start > 0 {
            let previous = regions.notes[region.notes.start - 1].1;
            let end = previous
                .onset_ticks
                .checked_add(previous.duration_ticks)
                .ok_or("Performance note endpoint overflows source ticks")?;
            unsafe_base |=
                clock.ms(end).max(clock.ms(previous.onset_ticks) + 40.0) >= clock.ms(region.start);
        }
        if region.notes.end < regions.notes.len() {
            let next = regions.notes[region.notes.end].1;
            unsafe_base |= clock.ms(next.onset_ticks) - 40.0 < clock.ms(region.end);
        }
        for dimension in [Dimension::PitchCents, Dimension::LinearGain] {
            let points = owner.map_or(&[][..], |p| match dimension {
                Dimension::PitchCents => &p.timeline.pitch_cents,
                _ => &p.timeline.linear_gain,
            });
            let used = traversal::points_in_span(points, region.start, region.end);
            budget.work(used.len())?;
            // Recoverable unknown state divides an owner run into local spans.
            // An earlier conflict cannot suppress later explicitly known state.
            let mut bounds = vec![region.start];
            let mut known = used
                .first()
                .filter(|p| p.tick <= region.start)
                .is_none_or(|p| p.value.is_some());
            for point in used {
                if point.tick > region.start && point.value.is_some() != known {
                    bounds.push(point.tick);
                    known = point.value.is_some();
                }
            }
            bounds.push(region.end);
            for span in bounds.windows(2) {
                budget.work(64)?;
                let (start, end) = (span[0], span[1]);
                let target_start = exact_ustx_ticks(start, ppq, "performance span start")?;
                let target_end = exact_ustx_ticks(end, ppq, "performance span end")?;
                let used = traversal::points_in_span(points, start, end);
                budget.work(used.len())?;
                let authored = !used.is_empty();
                let neutral = if dimension == Dimension::PitchCents {
                    0.0
                } else {
                    1.0
                };
                let mut steps = vec![(target_start, target_value(neutral, dimension).unwrap())];
                let mut reason = None;
                for point in used {
                    budget.ids(&point.source_ids)?;
                    // Predecessor state is already active at onset; a historical
                    // timestamp outside this span isn't a new target event.
                    let tick = exact_ustx_ticks(
                        point.tick.max(start),
                        ppq,
                        &format!("performance event {}", point.source_ids.join(", ")),
                    )?;
                    let value = point
                        .value
                        .ok_or_else(|| {
                            "Unknown channel state prevents editable transfer of this span."
                                .to_string()
                        })
                        .and_then(|v| target_value(v, dimension));
                    match value {
                        Err(message) => {
                            reason.get_or_insert(message);
                        }
                        Ok(value) => {
                            if steps.last().is_some_and(|(last, _)| *last == tick) {
                                steps.pop();
                            }
                            if steps.last().is_none_or(|(_, old)| *old != value) {
                                steps.push((tick, value));
                            }
                        }
                    }
                }
                if steps.windows(2).any(|pair| pair[1].0 - pair[0].0 < 5)
                    || steps.last().is_some_and(|(tick, _)| target_end - tick < 5)
                {
                    reason.get_or_insert("A positive held span is shorter than OpenUtau's 5-tick sampling interval and may disappear for the actual phrase origin.".into());
                }
                if authored && dimension == Dimension::PitchCents && unsafe_base {
                    reason.get_or_insert("Different source owners have overlapping default pitch-template extents (-40/+40 ms, integrated through the tempo map); the boundary has no verified pitch mapping. Nominal notes and other owners' defaults are retained.".into());
                }
                let status = if reason.is_none() {
                    TransferStatus::Mapped
                } else if used.iter().any(|p| p.value.is_none()) {
                    TransferStatus::Unsupported
                } else {
                    TransferStatus::RepresentationLimit
                };
                let map = if dimension == Dimension::PitchCents {
                    &mut pitch
                } else {
                    &mut gain
                };
                if !authored || reason.is_some() {
                    steps = vec![(target_start, 0)];
                } else if dimension == Dimension::PitchCents {
                    has_pitch = true;
                    let first = notes.partition_point(|(_, n)| {
                        u64::from(n.onset_ticks) + u64::from(n.duration_ticks) <= u64::from(start)
                    });
                    let last = notes.partition_point(|(_, n)| n.onset_ticks < end);
                    budget.work(last - first)?;
                    result
                        .flat_notes
                        .extend(notes[first..last].iter().map(|(i, _)| *i));
                } else {
                    has_gain = true;
                }
                budget.references(steps.len().saturating_mul(2).saturating_add(3))?;
                if region_index == 0 && start == region.start {
                    // Preserve applicable initial state during the first note's
                    // pre-roll, without assigning earlier superseded events.
                    map.insert(0, steps[0].1);
                }
                for (tick, value) in steps {
                    let previous = map.range(..tick).next_back().map_or(0, |(_, value)| *value);
                    if tick > 0 && previous != value {
                        map.insert(tick - 1, previous);
                    }
                    map.insert(tick, value);
                }
                let value = *map.last_key_value().unwrap().1;
                map.insert(target_end - 1, value);
                map.insert(
                    target_end,
                    if region_index + 1 == regions.spans.len() && end == region.end {
                        value
                    } else {
                        0
                    },
                );
                if authored {
                    let policy = match dimension {
                        Dimension::PitchCents => "Explicit MIDI RPN sensitivity and pitch bend mapped to PITD; integer cents rounded with at most 0.5-cent value error.",
                        _ => "CC7/CC11 linear normalized gains multiplied once, absent contributors neutral, mapped to DYN=round(200*log10(gain)); zero is exact mute. Value rounding is at most 0.05 dB. This conversion policy is not a universal GM response or identical soundfont playback.",
                    };
                    let message = reason.unwrap_or_else(|| format!("{policy} Held guard points are exact at integer ticks; render transitions are bounded to one 5-tick sample interval for any phrase origin, with no long ramp. The native flat note base can step up to four ticks early."));
                    for (point_index, point) in used.iter().enumerate() {
                        let affected_start = point.tick.max(start);
                        let affected_end =
                            used.get(point_index + 1).map_or(end, |p| p.tick.min(end));
                        if affected_start >= affected_end {
                            continue;
                        }
                        budget.references(1)?;
                        budget.ids(&point.source_ids)?;
                        budget.text(message.len())?;
                        result.transfers.push(PerformanceTransfer {
                            track_id: track.source_track_id.clone(),
                            target_track: Some(target_track),
                            dimension,
                            start_tick: affected_start,
                            end_tick: affected_end,
                            source_ids: point.source_ids.clone(),
                            note_ids: traversal::note_ids(
                                notes,
                                affected_start,
                                affected_end,
                                budget,
                            )?,
                            status,
                            message: message.clone(),
                        });
                    }
                }
            }
        }
        if let Some(owner) = owner {
            result.transfers.extend(traversal::issue_reports(
                track,
                target_track,
                notes,
                traversal::issues_in_span(&owner.timeline.issues, region.start, region.end),
                budget,
            )?);
        }
    }
    for (abbr, points, active) in [("pitd", pitch, has_pitch), ("dyn", gain, has_gain)] {
        if active {
            let (xs, ys) = points.into_iter().unzip();
            result.curves.push(UstxCurve {
                abbr: abbr.into(),
                xs,
                ys,
            });
        }
    }
    Ok(result)
}
