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
use crate::engine::performance::{
    exact_source_tick_bounds, exact_target_tick, Dimension, PerformanceTransfer, TransferStatus,
};
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
    let continuous_gain = track.notes.iter().any(|n| {
        n.performance
            .as_ref()
            .is_some_and(|p| p.intensity.is_some())
    });
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
            if continuous_gain && dimension == Dimension::LinearGain {
                continue;
            }
            let points = owner
                .and_then(|p| p.channel())
                .map_or(&[][..], |p| match dimension {
                    Dimension::PitchCents => &p.pitch_cents,
                    _ => &p.linear_gain,
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
                        for attribution in traversal::attributions(
                            track,
                            notes,
                            affected_start,
                            affected_end,
                            budget,
                        )? {
                            budget.references(1)?;
                            budget.ids(&point.source_ids)?;
                            budget.text(message.len())?;
                            result.transfers.push(PerformanceTransfer {
                                intensity: None,
                                track_id: attribution.track_id,
                                target_track: Some(target_track),
                                dimension,
                                start_tick: attribution.start,
                                end_tick: attribution.end,
                                source_ids: point.source_ids.clone(),
                                note_ids: attribution.note_ids,
                                status,
                                message: message.clone(),
                            });
                        }
                    }
                }
            }
        }
        if let Some(owner) = owner.and_then(|p| p.channel()) {
            result.transfers.extend(traversal::issue_reports(
                track,
                target_track,
                notes,
                traversal::issues_in_span(&owner.issues, region.start, region.end),
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
    if continuous_gain {
        let (curve, reports) = adapt_intensity(track, ppq, target_track, budget)?;
        if let Some(curve) = curve {
            result.curves.push(curve);
        }
        result.transfers.extend(reports);
    }
    Ok(result)
}

/// Continuous neutral score gain and the existing held CC product share one DYN.
/// Dense knots are needed only inside actual transitions. Every integer position
/// there is validated, covering every possible five-tick phrase sampling phase.
fn adapt_intensity(
    track: &ProjectedTrack,
    ppq: u16,
    target_track: usize,
    budget: &mut Budget,
) -> Result<(Option<UstxCurve>, Vec<PerformanceTransfer>), String> {
    use crate::engine::score_intensity::{self as si, source::time};
    let regions = traversal::regions(track, budget)?;
    let mut points = BTreeMap::<i32, i32>::new();
    let mut reports = Vec::new();
    let mut active = false;
    for (position, (_, note)) in regions.notes.iter().enumerate() {
        let Some(binding) = &note.performance else {
            let tick = exact_ustx_ticks(note.onset_ticks, ppq, "unowned gain boundary")?;
            if tick > 0 {
                let previous = points.range(..tick).next_back().map_or(0, |(_, v)| *v);
                points.insert(tick - 1, previous);
            }
            points.insert(tick, 0);
            continue;
        };
        let start = time(i64::from(note.onset_ticks), ppq)?;
        let end = time(
            i64::from(note.onset_ticks) + i64::from(note.duration_ticks),
            ppq,
        )?;
        let next_start = regions
            .notes
            .get(position + 1)
            .map(|(_, n)| time(i64::from(n.onset_ticks), ppq))
            .transpose()?
            .unwrap_or(end);
        let region_end = next_start.max(end);
        let intensity = binding.intensity.as_deref();
        let channel = binding.channel();
        let source_end = note
            .onset_ticks
            .checked_add(note.duration_ticks)
            .ok_or("Performance note endpoint overflows source ticks")?;
        let source_region_end = regions
            .notes
            .get(position + 1)
            .map_or(source_end, |(_, n)| n.onset_ticks)
            .max(source_end);
        let all_segments = intensity.map_or(&[][..], |n| n.timeline.segments.as_slice());
        let all_controls = channel.map_or(&[][..], |c| c.linear_gain.as_slice());
        let depth = |n: usize| usize::BITS as usize - n.leading_zeros() as usize + 1;
        budget.work(
            2usize.saturating_mul(
                depth(all_segments.len()).saturating_add(depth(all_controls.len())),
            ),
        )?;
        let first = all_segments.partition_point(|s| s.end < start);
        let last = all_segments
            .partition_point(|s| s.start <= region_end)
            .max(first);
        let segments = &all_segments[first..last];
        let controls = traversal::points_in_span(all_controls, note.onset_ticks, source_region_end);
        // Count borrowed, intersecting candidates before Vec allocation. Charge
        // both passes, then reserve storage and sort/compaction work cumulatively.
        budget.work(
            segments
                .len()
                .saturating_mul(4)
                .saturating_add(controls.len().saturating_mul(2)),
        )?;
        let score_bounds = segments
            .iter()
            .flat_map(|s| [s.start, s.end])
            .filter(|at| *at > start && *at < region_end);
        let cc_bounds = controls
            .iter()
            .filter(|p| p.tick > note.onset_ticks && p.tick < source_region_end);
        let count = 3usize
            .saturating_add(score_bounds.clone().count())
            .saturating_add(cc_bounds.clone().count());
        let mut bounds = budget.breakpoints(count)?;
        bounds.extend([start, end, region_end]);
        bounds.extend(score_bounds);
        for point in cc_bounds {
            bounds.push(time(i64::from(point.tick), ppq)?);
        }
        debug_assert_eq!(bounds.len(), count);
        bounds.sort_unstable();
        bounds.dedup();
        for span in bounds.windows(2) {
            let (a, b) = (span[0], span[1]);
            let inside_note = a < end;
            budget.work(depth(all_segments.len()).saturating_add(depth(all_controls.len())))?;
            let segment = intensity.and_then(|n| n.timeline.segment_at(a));
            budget.references(
                usize::from(intensity.is_some_and(|n| n.provenance.is_some()))
                    + usize::from(segment.is_some_and(|s| s.provenance.is_some())),
            )?;
            let provenance: Vec<_> = intensity
                .and_then(|n| n.provenance.as_ref())
                .into_iter()
                .chain(segment.and_then(|s| s.provenance.as_ref()))
                .collect();
            let source_tick = exact_source_tick_bounds(a, a, ppq)?.0;
            let cc = channel.and_then(|c| {
                let i = c.linear_gain.partition_point(|p| p.tick <= source_tick);
                i.checked_sub(1).map(|i| &c.linear_gain[i])
            });
            let source_ids = budget.collect_ids(
                provenance
                    .iter()
                    .flat_map(|p| p.evidence.iter().flat_map(|e| &e.source_ids))
                    .chain(cc.into_iter().flat_map(|p| &p.source_ids)),
            )?;
            let authored = !source_ids.is_empty();
            let score_at = |at: si::Time, current: Option<&si::Segment>| match intensity {
                Some(n) if inside_note => n.evaluate_segment(at, current),
                Some(_) => current.map_or(si::Evaluation::Absent, |s| s.curve.evaluate(at)),
                None => si::Evaluation::Absent,
            };
            let authored =
                authored && (score_at(a, segment) != si::Evaluation::Absent || cc.is_some());
            let evaluate = |at: si::Time| -> Result<(i32, bool), String> {
                let current = if at < b {
                    segment
                } else {
                    intensity.and_then(|n| n.timeline.segment_at(at))
                };
                composed_intensity_value(score_at(at, current), cc.map(|p| p.value), current)
            };
            let ta = exact_target_tick(a)?;
            let tb = exact_target_tick(b)?;
            let smooth = matches!(
                segment.map(|s| &s.curve),
                Some(si::Curve::Transition { .. })
            ) && !segment.is_some_and(|s| s.attack_only && inside_note);
            // Classify individual samples so out-of-range state recovers locally,
            // and the permitted positive niente floor describes only its tail.
            // (start, exclusive end, error, floor, unknown)
            let mut groups: Vec<(i32, i32, Option<String>, bool, bool)> = Vec::new();
            let mut short = tb <= ta || (tb - ta < 5 && authored);
            if short && tb > ta {
                if let Ok((value, _)) = evaluate(a) {
                    // A note boundary may subdivide one continuous source ramp.
                    // Extend only through the same transition and composed gain;
                    // authored short ramps, CC pulses and attack steps still stop
                    // this check. Dense sampling below keeps every phrase phase.
                    let transition = segment.filter(|_| smooth).map(|s| &s.curve);
                    let expected = |tick| {
                        if smooth {
                            let at = si::Fraction::new(i64::from(tick), 480).ok()?;
                            // Evaluate the candidate source phase with this
                            // note's existing anchor. NoteIntensity deliberately
                            // rejects times outside its note, so keep its query
                            // at a and supply the candidate's source value.
                            let curve = match transition?.evaluate(at) {
                                si::Evaluation::Absent => si::Curve::Absent,
                                si::Evaluation::Unknown => si::Curve::Held(None),
                                si::Evaluation::Known(value) => si::Curve::Held(Some(value)),
                            };
                            let sampled = si::Segment {
                                start: a,
                                end: b,
                                curve,
                                attack_only: false,
                                provenance: None,
                            };
                            composed_intensity_value(
                                score_at(a, Some(&sampled)),
                                cc.map(|p| p.value),
                                segment,
                            )
                            .ok()
                            .map(|(value, _)| value)
                        } else {
                            Some(value)
                        }
                    };
                    let (mut left, mut right) = (ta, tb);
                    while left > 0 && right - left < 5 {
                        let candidate =
                            gain_at_track_tick(&regions.notes, ppq, left - 1, transition, budget)?;
                        if candidate.is_none() || candidate != expected(left - 1) {
                            break;
                        }
                        left -= 1;
                    }
                    while right - left < 5 {
                        let candidate =
                            gain_at_track_tick(&regions.notes, ppq, right, transition, budget)?;
                        if candidate.is_none() || candidate != expected(right) {
                            break;
                        }
                        right += 1;
                    }
                    short = right - left < 5;
                }
            }
            let count = if smooth && !short {
                (tb - ta) as usize
            } else {
                1
            };
            // Fixed-segment evaluation is constant per sample; account for the
            // ordered point map's logarithmic insertions as well as this lookup.
            let lookup = usize::BITS as usize
                - intensity
                    .map_or(0, |n| n.timeline.segments.len())
                    .leading_zeros() as usize;
            let point_work =
                usize::BITS as usize - points.len().saturating_add(count).leading_zeros() as usize;
            budget
                .work(lookup.saturating_add(count.saturating_mul(point_work.saturating_add(2))))?;
            budget.references(count)?;
            for i in 0..count {
                let tick = ta + i as i32;
                let at = if smooth {
                    si::Fraction::new(i64::from(tick), 480)?.max(a).min(b)
                } else {
                    a
                };
                let sampled = if short {
                    Err("Positive expression span is shorter than the consumer's 5-tick sampling interval".into())
                } else {
                    evaluate(at)
                };
                let (value, error, floor, unknown) = match sampled {
                    Ok((value, floor)) => (value, None, floor, false),
                    Err(message) => {
                        let unknown = message.starts_with("Unknown");
                        (0, Some(message), false, unknown)
                    }
                };
                let stop = if smooth && !short { tick + 1 } else { tb };
                if let Some(last) = groups
                    .last_mut()
                    .filter(|g| g.2.is_some() == error.is_some() && g.3 == floor && g.4 == unknown)
                {
                    last.1 = stop;
                } else {
                    groups.push((tick, stop, error.clone(), floor, unknown));
                }
                if error.is_none() && authored {
                    active = true;
                }
                let previous = points.range(..tick).next_back().map_or(0, |(_, v)| *v);
                if tick > 0 && previous != value && (!smooth || tick == ta) {
                    points.insert(tick - 1, previous);
                }
                points.insert(tick, value);
            }
            if tb > ta {
                let value = points.range(..tb).next_back().map_or(0, |(_, v)| *v);
                points.insert(tb - 1, value);
            }
            if position == 0 && a == start {
                points.insert(0, points.get(&ta).copied().unwrap_or(0));
            }
            if position + 1 == regions.notes.len() && b == end && !short {
                if let Ok((value, _)) = evaluate(b) {
                    points.insert(tb, value);
                }
            }
            if authored && inside_note {
                for (gs, ge, reason, limited_tail, unknown) in groups {
                    let exact_start = if gs == ta {
                        a
                    } else {
                        si::Fraction::new(i64::from(gs), 480)?
                    };
                    let exact_end = if ge == tb {
                        b
                    } else {
                        si::Fraction::new(i64::from(ge), 480)?
                    };
                    #[derive(serde::Serialize)]
                    #[serde(rename_all = "camelCase")]
                    struct GainEvidence<'a> {
                        policy: &'static str,
                        start: si::Time,
                        end: si::Time,
                        provenance: &'a [&'a si::Provenance],
                        curve: Option<&'a si::Curve>,
                        rounding: &'static str,
                        limited_niente_tail: bool,
                        target_start: i32,
                        target_end: i32,
                    }
                    budget.ids(&source_ids)?;
                    let message = reason.as_deref().unwrap_or(if limited_tail {
                        "Active niente fade: positive tail held at DYN -239 until exact zero; this tail has a representation limit"
                    } else {
                        "verse-score-intensity-v1: absolute attack plus relative score motion, existing linear CC7/CC11 gain product once (not a universal GM response); DYN sampled at integer target positions with <=0.1 dB smooth-span value error; boundary sampling <=5 ticks"
                    });
                    budget.report_text(
                        traversal::original_track_id(track, note),
                        Some(traversal::original_note_id(note, binding)),
                        message,
                    )?;
                    let exact = budget.json(&GainEvidence {
                        policy: si::POLICY,
                        start: exact_start,
                        end: exact_end,
                        provenance: &provenance,
                        curve: segment.map(|s| &s.curve),
                        rounding: "nearest, ties away from zero",
                        limited_niente_tail: limited_tail,
                        target_start: gs,
                        target_end: ge,
                    })?;
                    let (start_tick, end_tick) =
                        exact_source_tick_bounds(exact_start, exact_end, ppq)?;
                    reports.push(PerformanceTransfer {
                        intensity: Some(exact),
                        track_id: traversal::original_track_id(track, note).to_string(),
                        target_track: Some(target_track),
                        dimension: Dimension::LinearGain,
                        start_tick,
                        end_tick,
                        source_ids: source_ids.clone(),
                        note_ids: vec![traversal::original_note_id(note, binding).to_string()],
                        status: if unknown {
                            TransferStatus::Unsupported
                        } else if reason.is_some() || limited_tail {
                            TransferStatus::RepresentationLimit
                        } else {
                            TransferStatus::Mapped
                        },
                        message: message.to_string(),
                    });
                }
            }
        }
        reports.extend(traversal::intensity_issue_reports(
            track,
            note,
            target_track,
            ppq,
            position + 1 == regions.notes.len(),
            budget,
        )?);
    }
    let terminal_tick = track
        .notes
        .last()
        .map(|n| {
            exact_target_tick(time(
                i64::from(n.onset_ticks) + i64::from(n.duration_ticks),
                ppq,
            )?)
        })
        .transpose()?;
    let terminal_mapped = active && terminal_tick.is_some_and(|tick| points.contains_key(&tick));
    if let Some(terminal) =
        traversal::intensity_terminal_report(track, target_track, ppq, terminal_mapped, budget)?
    {
        reports.push(terminal);
    }
    // Equal collinear integer knots can be omitted without changing UCurve values.
    let mut compact: Vec<(i32, i32)> = Vec::new();
    for point in points {
        while compact.len() >= 2 {
            let a = compact[compact.len() - 2];
            let b = compact[compact.len() - 1];
            if i64::from(b.1 - a.1) * i64::from(point.0 - b.0)
                == i64::from(point.1 - b.1) * i64::from(b.0 - a.0)
            {
                compact.pop();
            } else {
                break;
            }
        }
        compact.push(point);
    }
    let curve = active.then(|| {
        let (xs, ys) = compact.into_iter().unzip();
        UstxCurve {
            abbr: "dyn".into(),
            xs,
            ys,
        }
    });
    Ok((curve, reports))
}

fn composed_intensity_value(
    score: crate::engine::score_intensity::Evaluation,
    controller: Option<Option<f64>>,
    segment: Option<&crate::engine::score_intensity::Segment>,
) -> Result<(i32, bool), String> {
    use crate::engine::score_intensity as si;
    let gain = match score {
        si::Evaluation::Absent => 1.0,
        si::Evaluation::Unknown => {
            return Err(
                "Unknown score intensity; neutral intent retained without guessed gain".into(),
            )
        }
        si::Evaluation::Known(value) => value.gain(),
    };
    let controller = controller
        .unwrap_or(Some(1.0))
        .ok_or("Unknown channel gain")?;
    let combined = gain * controller;
    if !combined.is_finite() || (combined == 0.0 && gain > 0.0 && controller > 0.0) {
        return Err("Combined gain overflow or positive underflow".into());
    }
    let tail = matches!(
        segment.map(|s| &s.curve),
        Some(si::Curve::Transition {
            domain: si::Domain::LinearGain,
            ..
        })
    );
    if combined > 0.0 && tail && 200.0 * combined.log10() < -239.0 {
        return Ok((-239, true));
    }
    target_value(combined, Dimension::LinearGain).map(|value| (value, false))
}

/// A note boundary does not end a held expression or continuous source ramp.
/// Check at most four adjacent target positions using indexed lookup. A ramp
/// must retain its source phase; equal values from another segment do not count.
fn gain_at_track_tick(
    notes: &[(usize, &crate::engine::projection::ProjectedNote)],
    ppq: u16,
    tick: i32,
    transition: Option<&crate::engine::score_intensity::Curve>,
    budget: &mut Budget,
) -> Result<Option<i32>, String> {
    use crate::engine::score_intensity::{self as si, source::time};
    if tick < 0 {
        return Ok(None);
    }
    let at = si::Fraction::new(i64::from(tick), 480)?;
    budget.work(usize::BITS as usize - notes.len().leading_zeros() as usize + 1)?;
    let source_tick = exact_source_tick_bounds(at, at, ppq)?.0;
    let Some(index) = notes
        .partition_point(|(_, n)| n.onset_ticks <= source_tick)
        .checked_sub(1)
    else {
        return Ok(None);
    };
    let note = notes[index].1;
    let end = time(
        i64::from(note.onset_ticks) + i64::from(note.duration_ticks),
        ppq,
    )?;
    if index + 1 == notes.len() && at >= end {
        return Ok(None);
    }
    let Some(binding) = &note.performance else {
        // Neutral equality alone cannot prove the phase of a smooth source
        // transition across a boundary with no source performance binding.
        return Ok(transition.is_none().then_some(0));
    };
    let segment = binding
        .intensity
        .as_ref()
        .and_then(|n| n.timeline.segment_at(at));
    if transition.is_some_and(|curve| {
        segment.is_none_or(|s| &s.curve != curve || (s.attack_only && at < end))
    }) {
        return Ok(None);
    }
    let score = binding
        .intensity
        .as_ref()
        .map_or(si::Evaluation::Absent, |n| {
            if at < end {
                n.evaluate_segment(at, segment)
            } else {
                n.timeline.evaluate(at)
            }
        });
    let channel = binding.channel();
    budget.work(
        2 + usize::BITS as usize
            - binding
                .intensity
                .as_ref()
                .map_or(0, |n| n.timeline.segments.len())
                .leading_zeros() as usize
            + usize::BITS as usize
            - channel.map_or(0, |c| c.linear_gain.len()).leading_zeros() as usize,
    )?;
    let cc = channel.and_then(|c| {
        c.linear_gain
            .partition_point(|p| p.tick <= source_tick)
            .checked_sub(1)
            .map(|i| c.linear_gain[i].value)
    });
    Ok(composed_intensity_value(score, cc, segment)
        .ok()
        .map(|(value, _)| value))
}

#[cfg(test)]
mod second_review_tests {
    use super::*;
    #[test]
    fn ustx_adaptation_preflights_large_borrowed_evidence_and_ids() {
        for large_id in [false, true] {
            let project = traversal::second_review_tests::project(large_id);
            let before = project.clone();
            let track = &project.tracks[0];
            let error = adapt_intensity(
                track,
                project.ticks_per_beat,
                0,
                &mut Budget::with_bytes(256),
            )
            .unwrap_err();
            assert!(error.contains("MIDI_PERFORMANCE_LIMIT"), "{error}");
            let (curve, reports) =
                adapt_intensity(track, project.ticks_per_beat, 0, &mut Budget::default()).unwrap();
            assert!(curve.is_some());
            assert!(reports.iter().any(|r| r.status == TransferStatus::Mapped));
            assert_eq!(project, before);
        }
    }
}

#[cfg(test)]
mod fourth_review_tests {
    use super::*;
    use crate::engine::projection::{ProjectedLyric, ProjectedNote};
    use crate::engine::{midi, performance};

    /// Actual SMF with one sustained note and independently authored CC7 events.
    /// Normalize through the production parser; the test calls the real adapter
    /// with a smaller budget before any target report can allocate breakpoints.
    fn sustained_controls(count: usize, spacing: u8) -> ProjectedTrack {
        let mut events = vec![0, 0x90, 60, 80];
        for i in 0..count {
            events.extend([spacing, 0xb0, 7, if i % 2 == 0 { 80 } else { 100 }]);
        }
        events.extend([spacing, 0x80, 60, 0, 0, 0xff, 0x2f, 0]);
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        bytes.extend_from_slice(&(events.len() as u32).to_be_bytes());
        bytes.extend(events);
        let source = midi::parse(&bytes).unwrap();
        let normalized = performance::normalize(&source).unwrap();
        let ((track_id, _), binding) = normalized.bindings.into_iter().next().unwrap();
        assert_eq!(binding.channel().unwrap().linear_gain.len(), count);
        ProjectedTrack {
            name: "Sustain".into(),
            source_track_id: track_id,
            muted: false,
            notes: vec![ProjectedNote {
                onset_ticks: 0,
                duration_ticks: (count as u32 + 1) * u32::from(spacing),
                pitch: 60,
                lyric: ProjectedLyric::Absent,
                source_evidence: None,
                performance: Some(binding),
            }],
        }
    }

    #[test]
    fn dense_cc_breakpoints_refuse_before_allocation_for_bytes_and_sort_work() {
        let track = sustained_controls(4096, 10);
        let before = track.clone();
        for mut budget in [
            Budget::with_bytes(128),
            Budget::with_limits(32 * 1024 * 1024, 12_000),
        ] {
            let error = adapt_intensity(&track, 480, 0, &mut budget).unwrap_err();
            assert!(error.contains("MIDI_PERFORMANCE_LIMIT"), "{error}");
            assert_eq!(
                budget.breakpoint_capacity(),
                0,
                "refusal must precede bounds allocation"
            );
        }
        assert_eq!(track, before);
    }

    #[test]
    fn dense_score_breakpoints_refuse_before_allocation() {
        use crate::engine::score_intensity::{Curve, Segment, Time};
        use std::sync::Arc;
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
        let timeline = Arc::make_mut(&mut intensity.timeline);
        let original = timeline
            .segments
            .iter()
            .find(|s| s.provenance.is_some())
            .unwrap()
            .clone();
        timeline.segments = (0..4096)
            .map(|i| Segment {
                start: Time::new(i, 4096).unwrap(),
                end: Time::new(i + 1, 4096).unwrap(),
                curve: Curve::Held(Some(crate::engine::score_intensity::Intensity::Decibels(
                    0.0,
                ))),
                attack_only: false,
                provenance: original.provenance.clone(),
            })
            .collect();
        let mut budget = Budget::with_bytes(1024);
        assert!(adapt_intensity(&project.tracks[0], 480, 0, &mut budget)
            .unwrap_err()
            .contains("MIDI_PERFORMANCE_LIMIT"));
        assert_eq!(budget.breakpoint_capacity(), 0);
    }

    #[test]
    fn sparse_notes_share_cc_timeline_and_cumulative_breakpoint_budget() {
        let mut track = sustained_controls(48, 10);
        let head = track.notes[0].clone();
        // Forty-nine touching fragments of the same sustained source attack.
        // Only local CC breakpoints count; the complete timeline stays shared.
        track.notes = (0..49)
            .map(|i| ProjectedNote {
                onset_ticks: i * 10,
                duration_ticks: 10,
                ..head.clone()
            })
            .collect();
        let mut budget = Budget::default();
        let (curve, reports) = adapt_intensity(&track, 480, 0, &mut budget).unwrap();
        assert!(curve.is_some());
        assert!(reports.iter().all(|r| r.status == TransferStatus::Mapped));
        assert_eq!(budget.breakpoint_capacity(), 49 * 3);
        // Exhaust the remaining allowance with another real pass; the same
        // Budget cannot reset between notes, lanes, or calls.
        let mut small = Budget::with_bytes(128 * 1024);
        let mut refused = false;
        for _ in 0..32 {
            if let Err(error) = adapt_intensity(&track, 480, 0, &mut small) {
                assert!(error.contains("MIDI_PERFORMANCE_LIMIT"));
                refused = true;
                break;
            }
        }
        assert!(refused);
    }
}
