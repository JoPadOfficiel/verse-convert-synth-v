//! XML field decoding and source-coordinate collection for the intensity resolver.
//! Nominal loaders supply positions; this module never parses a second document.
use super::*;
use roxmltree::Node;

#[derive(Clone, Debug, Default)]
pub struct ScoreInput {
    pub score: ScoreIntensity,
    pub overrides: BTreeMap<String, VelocityEvidence>,
    pub runs: BTreeMap<(String, String), Vec<Occurrence>>,
    route_keys: BTreeMap<String, BTreeMap<String, (String, String)>>,
    route_breaks: BTreeMap<(String, String), BTreeSet<u32>>,
    /// Legacy diagnostic coordinates only. Exact written measure membership
    /// below determines ambiguity; coordinate equality never does.
    pub declaration_points: BTreeMap<(String, String), Vec<DeclarationPoint>>,
    pub retained: BTreeMap<String, Evidence>,
    /// Original declaration coordinates, including unmatched/disabled spanners.
    /// Keys are original source IDs; pairing never overwrites endpoint positions.
    pub declarations: BTreeMap<String, Declaration>,
    /// Original parser facts, independent of interpreted/paired event copies.
    pub original_declarations: BTreeMap<String, OriginalDeclaration>,
    pub written_measures: Vec<WrittenMeasure>,
    pub paired_declarations: BTreeMap<String, BTreeSet<String>>,
    current_measure: Option<usize>,
    pub ties: BTreeMap<(String, u32, u32), String>,
    wedges: Vec<Wedge>,
    spanners: Vec<PendingSpanner>,
    typed_ends: Vec<TypedEndpoint>,
    legacy_ends: BTreeMap<String, Vec<(Time, u32, Evidence)>>,
    pub issues: Vec<Issue>,
    pub issue_passes: BTreeMap<String, Vec<u32>>,
    pub tempos: BTreeMap<Time, Option<Fraction>>,
    pub tempo_candidates: BTreeMap<Time, Vec<TempoCandidate>>,
    captured_bytes: usize,
    provenance_budget: ProvenanceBudget,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Declaration {
    pub scope: Scope,
    pub at: Time,
    pub order: u32,
    pub enabled: bool,
    pub time_only: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum DeclarationKind {
    Dynamic,
    Transition,
    Text,
    Velocity,
    Tempo,
    WedgeStart,
    WedgeContinue,
    WedgeStop,
    SpannerEndpoint,
    Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct OriginalDeclaration {
    pub kinds: BTreeSet<DeclarationKind>,
    pub written_measure: Option<usize>,
    pub note_source_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct WrittenMeasure {
    pub part: String,
    /// MusicXML measures belong to the part; MuseScore measures to a staff.
    pub staff: Option<String>,
    pub ordinal: u32,
    pub start: Time,
    pub end: Time,
    pub visits: Vec<MeasureVisit>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MeasureVisit {
    pub staff: String,
    pub performed_start: Time,
    pub repeat_pass: u32,
    /// Existing maximal forward-run ordinal. Zero width never supplies one.
    pub occurrence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoCandidate {
    pub exact: Option<Fraction>,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationPoint {
    pub written_at: Time,
    pub performed_at: Time,
    pub pass: u32,
}

#[derive(Clone, Debug)]
struct PendingSpanner {
    event: ScoreEvent,
    measure: usize,
    start: Time,
    delta: Option<(i64, Time)>,
    kind: Option<String>,
}

#[derive(Clone, Debug)]
struct TypedEndpoint {
    event: ScoreEvent,
    owner: ScoreVoice,
    measure: usize,
    delta: Option<(i64, Time)>,
    kind: String,
}

pub fn child<'a>(node: Node<'a, '_>, tag: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|n| n.has_tag_name(tag))
}
pub fn text<'a>(node: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(node, tag).and_then(|n| n.text()).map(str::trim)
}
// Intensity decoding must agree on every supported occurrence. Keep the public
// first-child helpers unchanged: nominal loaders own their timing/geometry.
fn agree_value<T: PartialEq>(value: &mut Option<T>, next: T, tag: &str) -> Result<()> {
    if value.as_ref().is_some_and(|value| value != &next) {
        return Err(format!("Conflicting repeated {tag} fields"));
    }
    if value.is_none() {
        *value = Some(next);
    }
    Ok(())
}

fn agreed_children<T: PartialEq>(
    node: Node,
    tag: &str,
    budget: &mut ProvenanceBudget,
    mut decode: impl FnMut(Node, &mut ProvenanceBudget) -> Result<T>,
) -> Result<Option<T>> {
    budget.charge(0, node.range().len().saturating_add(1))?;
    let mut value = None;
    for field in node.children().filter(|n| n.has_tag_name(tag)) {
        agree_value(&mut value, decode(field, budget)?, tag)?;
    }
    Ok(value)
}

fn scalar<T: PartialEq>(
    node: Node,
    tag: &str,
    budget: &mut ProvenanceBudget,
    mut decode: impl FnMut(&str) -> Result<T>,
) -> Result<Option<T>> {
    agreed_children(node, tag, budget, |field, _| {
        decode(field.text().unwrap_or("").trim())
    })
}

fn scalar_text(node: Node, tag: &str, budget: &mut ProvenanceBudget) -> Result<Option<String>> {
    agreed_children(node, tag, budget, |field, budget| {
        budget.charge(field.range().len(), 1)?;
        Ok(field.text().unwrap_or("").trim().to_owned())
    })
}

fn scalar_bool(node: Node, tag: &str, budget: &mut ProvenanceBudget) -> Result<Option<bool>> {
    scalar(node, tag, budget, |value| {
        bool_field(Some(value)).map(|value| value.unwrap())
    })
}

fn decimal(node: Node, tag: &str, budget: &mut ProvenanceBudget) -> Result<Option<Fraction>> {
    scalar(node, tag, budget, Fraction::decimal)
}

fn integer(node: Node, tag: &str, budget: &mut ProvenanceBudget) -> Result<Option<i64>> {
    scalar(node, tag, budget, |value| {
        value
            .parse()
            .map_err(|_| format!("Invalid {tag} displacement"))
    })
}

fn rich_text(node: Node, tag: &str, budget: &mut ProvenanceBudget) -> Result<Option<String>> {
    agreed_children(node, tag, budget, |field, budget| {
        budget.charge(field.range().len(), field.range().len())?;
        Ok(field
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect())
    })
}

fn sound_attribute<T: PartialEq>(
    node: Node,
    attribute: &str,
    budget: &mut ProvenanceBudget,
    mut decode: impl FnMut(&str) -> Result<T>,
) -> Result<Option<T>> {
    budget.charge(0, node.range().len().saturating_add(1))?;
    let mut value = None;
    for sound in node
        .children()
        .chain(std::iter::once(node))
        .filter(|n| n.has_tag_name("sound"))
    {
        if let Some(raw) = sound.attribute(attribute) {
            agree_value(&mut value, decode(raw)?, attribute)?;
        }
    }
    Ok(value)
}

fn spanner_delta(
    node: Node,
    direction: &str,
    stretch: (i64, i64),
    budget: &mut ProvenanceBudget,
) -> Result<Option<(i64, Time)>> {
    agreed_children(node, direction, budget, |next, budget| {
        agreed_children(next, "location", budget, |loc, budget| {
            let measures = integer(loc, "measures", budget)?.unwrap_or(0);
            let staves = integer(loc, "staves", budget)?.unwrap_or(0);
            let voices = integer(loc, "voices", budget)?.unwrap_or(0);
            let fractions = scalar(loc, "fractions", budget, |value| {
                let (n, d) = value.split_once('/').ok_or("Invalid spanner fraction")?;
                Fraction::new(
                    n.parse().map_err(|_| "Invalid spanner numerator")?,
                    d.parse().map_err(|_| "Invalid spanner denominator")?,
                )
            })?
            .unwrap_or(Time::ZERO);
            Ok((measures, fractions, staves, voices))
        })
    })?
    .flatten()
    .map(|(measures, fractions, _, _)| {
        Ok((
            measures,
            fractions
                .checked_mul(Fraction::integer(4))?
                .checked_mul(Fraction::new(stretch.0, stretch.1)?)?,
        ))
    })
    .transpose()
}
pub fn time(tick: i64, ppq: u16) -> Result<Time> {
    Fraction::new(tick, i64::from(ppq))
}
fn bool_field(value: Option<&str>) -> Result<Option<bool>> {
    value
        .map(|s| match s {
            "yes" | "true" | "1" => Ok(true),
            "no" | "false" | "0" => Ok(false),
            _ => Err(format!("Unknown playback boolean {s:?}")),
        })
        .transpose()
}
fn raw(node: Node) -> BTreeMap<String, String> {
    // Keep complete owned XML, including unknown fields and namespace declarations.
    BTreeMap::from([(
        "xml".into(),
        node.document().input_text()[node.range()].into(),
    )])
}
fn interpretation(id: &str, field: &str, explanation: &str, explicit: bool) -> Interpretation {
    Interpretation {
        field: field.into(),
        source_ids: vec![id.into()],
        basis: if explicit {
            Basis::PortableInterpretation
        } else {
            Basis::DeclaredDefault
        },
        exact: None,
        explanation: explanation.into(),
    }
}

fn dynamic_behavior(symbol: &str) -> &str {
    match symbol {
        "fp" => "fp",
        "pf" => "pf",
        "sf" | "sfz" | "rf" | "rfz" | "fz" | "sff" | "sffz" => "attack",
        symbol if standard_level(symbol).is_some() => "persistent",
        // Unknown compounds cannot become persistent through a numeric sibling.
        other => other,
    }
}

fn route_point_pass(runs: &[Occurrence], index: usize, at: Time, terminal: bool) -> Option<u32> {
    let mut cutoff = None;
    let mut reset_pass = None;
    for i in (0..=index).rev() {
        let run = &runs[i];
        if cutoff.is_none_or(|cut| at < cut)
            && at >= run.written_start
            && (at < run.written_end || (i == index && terminal && at == run.written_end))
        {
            return Some(reset_pass.unwrap_or(run.pass));
        }
        cutoff = Some(cutoff.map_or(run.written_start, |cut: Time| cut.min(run.written_start)));
        if i > 0 && run.written_start < runs[i - 1].written_end && reset_pass.is_none() {
            reset_pass = Some(run.pass);
        }
    }
    None
}

/// Reuse the numbered/same-scope pairing rules on each proven forward route.
/// Pass labels belong to individual declarations, not to the whole route.
fn pair_performed_wedges(
    input: &ScoreInput,
    wedges: &[Wedge],
    budget: &mut ProvenanceBudget,
) -> Result<(Vec<ScoreEvent>, Vec<Issue>)> {
    budget.charge(
        wedges.len().saturating_mul(128),
        wedges.len().saturating_mul(16),
    )?;
    let mut scopes: BTreeMap<&Scope, Vec<&Wedge>> = BTreeMap::new();
    let mut synthetic = Vec::new();
    for wedge in wedges {
        if input
            .original_declarations
            .get(&wedge.event.evidence.source_ids[0])
            .is_none_or(|d| d.written_measure.is_none())
        {
            budget.event(&wedge.event)?;
            synthetic.push(wedge.clone());
        } else {
            scopes.entry(&wedge.event.scope).or_default().push(wedge);
        }
    }
    let run_count = input.runs.values().map(Vec::len).sum::<usize>();
    budget.charge(run_count.saturating_mul(4), run_count)?;
    let passes: Vec<_> = input.runs.values().flatten().map(|r| r.pass).collect();
    let (mut events, mut issues) = pair_wedges_bounded(&synthetic, &passes, None, budget)?;
    let mut emitted = BTreeMap::<Vec<String>, Vec<usize>>::new();
    for (scope, candidates) in scopes {
        budget.charge(0, input.runs.len())?;
        for (route_key, runs) in &input.runs {
            let (part, staff) = route_key;
            let breaks = input.route_breaks.get(route_key);
            let voice = match scope {
                Scope::Part(p) if p == part => "1",
                Scope::Staff { part: p, staff: s } if p == part && s == staff => "1",
                Scope::Voice {
                    part: p,
                    staff: s,
                    voice,
                } if p == part && s.as_deref().is_none_or(|s| s == staff) => voice,
                _ => continue,
            };
            budget.charge(
                part.len()
                    .saturating_add(staff.len())
                    .saturating_add(voice.len())
                    .saturating_add(64),
                1,
            )?;
            let owner = ScoreVoice {
                part: part.clone(),
                staff: staff.clone(),
                voice: voice.into(),
                instrument: None,
            };
            let mut first = 0;
            while first < runs.len() {
                let mut last = first;
                while last + 1 < runs.len() {
                    let current = &runs[last];
                    let next = &runs[last + 1];
                    budget.charge(0, 16)?;
                    if current.written_end != next.written_start
                        || current
                            .performed_start
                            .checked_add(current.written_end.checked_sub(current.written_start)?)?
                            != next.performed_start
                        || breaks.is_some_and(|breaks| breaks.contains(&next.ordinal))
                    {
                        break;
                    }
                    last += 1;
                }
                budget.charge(candidates.len().saturating_mul(128), candidates.len())?;
                let mut selected = Vec::new();
                let mut local_passes = BTreeMap::new();
                for wedge in &candidates {
                    let id = &wedge.event.evidence.source_ids[0];
                    let measure = &input.written_measures
                        [input.original_declarations[id].written_measure.unwrap()];
                    // Original measure ownership prevents a skipped ending with
                    // an offset into this route from consuming another start.
                    if measure.start < runs[first].written_start
                        || measure.start >= runs[last].written_end
                        || wedge.event.at < runs[first].written_start
                        || wedge.event.at > runs[last].written_end
                    {
                        continue;
                    }
                    budget.charge(
                        id.len().saturating_add(64),
                        runs.len()
                            .saturating_mul(3)
                            .saturating_add(wedge.event.time_only.len())
                            .saturating_add(16),
                    )?;
                    let Some(pass) = input.declaration_route_pass(
                        id,
                        &owner,
                        runs[last].ordinal,
                        runs[last].pass,
                    ) else {
                        continue;
                    };
                    local_passes.insert(id.as_str(), pass);
                    if !wedge.event.enabled
                        || (!wedge.event.time_only.is_empty()
                            && !wedge.event.time_only.contains(&pass))
                    {
                        budget.event(&wedge.event)?;
                        issues.push(Issue {
                            start: wedge.event.at,
                            end: wedge.event.at,
                            kind: if wedge.event.enabled {
                                IssueKind::PassFiltered
                            } else {
                                IssueKind::Disabled
                            },
                            provenance: Provenance::from_event(&wedge.event, pass),
                            message: if wedge.event.enabled {
                                "time-only excludes this repeat pass."
                            } else {
                                "Playback-disabled wedge endpoint retained inactive."
                            }
                            .into(),
                        });
                        continue;
                    }
                    budget.event(&wedge.event)?;
                    let mut selected_wedge = (*wedge).clone();
                    selected_wedge.event.time_only.clear();
                    selected.push(selected_wedge);
                }
                let (paired, mut unresolved) = pair_wedges_bounded(&selected, &[], None, budget)?;
                for issue in &mut unresolved {
                    if let Some(pass) = issue
                        .provenance
                        .evidence
                        .iter()
                        .flat_map(|e| &e.source_ids)
                        .find_map(|id| local_passes.get(id.as_str()))
                    {
                        issue.provenance.repeat_pass = *pass;
                    }
                }
                issues.extend(unresolved);
                for mut event in paired {
                    let start = selected
                        .iter()
                        .find(|w| {
                            matches!(w.kind, WedgeKind::Start(_))
                                && w.event.at == event.at
                                && w.event.order == event.order
                        })
                        .ok_or("Missing paired wedge start")?;
                    let pass = local_passes[start.event.evidence.source_ids[0].as_str()];
                    let key = &event.evidence.source_ids;
                    budget.charge(
                        strings_size(key).saturating_add(64),
                        selected.len().saturating_add(key.len().saturating_mul(16)),
                    )?;
                    let prior = emitted.get(key).map(Vec::as_slice).unwrap_or(&[]);
                    budget.charge(
                        0,
                        evidence_size(&event.evidence)
                            .saturating_add(scope_size(&event.scope))
                            .saturating_add(256)
                            .saturating_mul(prior.len()),
                    )?;
                    // The same direction node may author distinct numbered
                    // wedges. Equal source-ID sets alone do not prove equal
                    // candidates (in particular, their directions may conflict).
                    let same = prior.iter().copied().find(|&index| {
                        let previous = &events[index];
                        previous.at == event.at
                            && previous.order == event.order
                            && previous.scope == event.scope
                            && previous.scope_interpretation == event.scope_interpretation
                            && previous.enabled == event.enabled
                            && previous.evidence == event.evidence
                            && previous.instruction == event.instruction
                    });
                    if let Some(index) = same {
                        budget.charge(4, events[index].time_only.len().saturating_add(1))?;
                        if !events[index].time_only.contains(&pass) {
                            events[index].time_only.push(pass);
                        }
                    } else {
                        emitted.entry(key.clone()).or_default().push(events.len());
                        event.time_only = vec![pass];
                        events.push(event);
                    }
                }
                first = last + 1;
            }
        }
    }
    Ok((events, issues))
}

impl ScoreInput {
    pub fn begin_written_measure(
        &mut self,
        part: &str,
        staff: Option<&str>,
        ordinal: usize,
        start: Time,
    ) -> Result<usize> {
        self.provenance_budget.charge(
            part.len()
                .saturating_add(staff.map_or(0, str::len))
                .saturating_add(256),
            1,
        )?;
        let index = self.written_measures.len();
        self.written_measures.push(WrittenMeasure {
            part: part.into(),
            staff: staff.map(str::to_owned),
            ordinal: u32::try_from(ordinal).map_err(|_| "Written measure ordinal overflow")?,
            start,
            end: start,
            visits: vec![],
        });
        self.current_measure = Some(index);
        Ok(index)
    }

    pub fn end_written_measure(&mut self, end: Time) -> Result<()> {
        let index = self
            .current_measure
            .take()
            .ok_or("Missing written measure")?;
        let measure = &mut self.written_measures[index];
        if end < measure.start {
            return Err("Reversed written measure".into());
        }
        measure.end = end;
        Ok(())
    }

    pub fn record_measure_run(
        &mut self,
        measure: usize,
        part: &str,
        staff: &str,
        performed: Time,
        pass: u32,
        forward: bool,
    ) -> Result<()> {
        let written = self
            .written_measures
            .get(measure)
            .ok_or("Missing written measure")?;
        let bounds = (written.start, written.end);
        self.provenance_budget.charge(
            part.len().saturating_add(staff.len()).saturating_add(256),
            1,
        )?;
        self.record_run(part, staff, bounds, performed, pass, forward)?;
        let occurrence = if bounds.0 == bounds.1 {
            0
        } else {
            self.runs
                .get(&(part.into(), staff.into()))
                .and_then(|r| r.last())
                .ok_or("Missing positive measure run")?
                .ordinal
        };
        self.written_measures[measure].visits.push(MeasureVisit {
            staff: staff.into(),
            performed_start: performed,
            repeat_pass: pass,
            occurrence,
        });
        Ok(())
    }

    fn record_original(&mut self, id: &str, kind: DeclarationKind) -> Result<()> {
        self.provenance_budget
            .charge(id.len().saturating_add(192), 1)?;
        let original = self
            .original_declarations
            .entry(id.into())
            .or_insert_with(|| OriginalDeclaration {
                written_measure: self.current_measure,
                ..OriginalDeclaration::default()
            });
        original.kinds.insert(kind);
        Ok(())
    }

    fn record_pairs(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            for other in ids {
                self.provenance_budget
                    .charge(0, id.len().saturating_add(other.len()).saturating_add(1))?;
                if other == id {
                    continue;
                }
                self.provenance_budget
                    .charge(id.len().saturating_add(other.len()).saturating_add(128), 1)?;
                self.paired_declarations
                    .entry(id.clone())
                    .or_default()
                    .insert(other.clone());
            }
        }
        Ok(())
    }

    /// Raw membership, never playback-position equality. Unplayed and empty
    /// source measures have no provable occurrence, even with an offset into a
    /// different positive measure. Synthetic callers may omit membership.
    pub fn declaration_is_unresolved(&self, id: &str, owner: &ScoreVoice) -> bool {
        let no_runs = self.owner_runs(owner).is_none_or(|runs| runs.is_empty());
        no_runs
            || self
                .original_declarations
                .get(id)
                .and_then(|original| original.written_measure)
                .and_then(|index| self.written_measures.get(index))
                .is_some_and(|measure| measure.start == measure.end || measure.visits.is_empty())
    }

    pub fn owner_runs(&self, owner: &ScoreVoice) -> Option<&[Occurrence]> {
        let key = self.route_keys.get(&owner.part)?.get(&owner.staff)?;
        self.runs.get(key).map(Vec::as_slice)
    }

    /// A pass-label change can remain forward playback. Explicit navigation
    /// breaks take precedence even when both coordinate boundaries touch.
    pub(super) fn forward_route_boundary(
        &self,
        owner: &ScoreVoice,
        previous: &Occurrence,
        next: &Occurrence,
        budget: &mut ProvenanceBudget,
    ) -> Result<bool> {
        budget.charge(0, 16)?;
        if previous.written_end != next.written_start
            || previous
                .performed_start
                .checked_add(previous.written_end.checked_sub(previous.written_start)?)?
                != next.performed_start
        {
            return Ok(false);
        }
        let depth = |count: usize| usize::BITS as usize - count.leading_zeros() as usize + 1;
        // Borrow stored keys throughout; no cloned owner strings or route
        // inventories are needed. Charge string comparisons before each lookup.
        budget.charge(
            0,
            owner
                .part
                .len()
                .saturating_add(1)
                .saturating_mul(depth(self.route_keys.len())),
        )?;
        let Some(staves) = self.route_keys.get(&owner.part) else {
            return Ok(false);
        };
        budget.charge(
            0,
            owner
                .staff
                .len()
                .saturating_add(1)
                .saturating_mul(depth(staves.len())),
        )?;
        let Some(key) = staves.get(&owner.staff) else {
            return Ok(false);
        };
        budget.charge(
            0,
            key.0
                .len()
                .saturating_add(key.1.len())
                .saturating_add(1)
                .saturating_mul(depth(self.route_breaks.len())),
        )?;
        let Some(breaks) = self.route_breaks.get(key) else {
            return Ok(true);
        };
        budget.charge(0, depth(breaks.len()))?;
        Ok(!breaks.contains(&next.ordinal))
    }

    /// A metadata lane supplements missing declared voices without changing
    /// existing sounding lanes. Its typed voices remain in source topology.
    pub(crate) fn has_missing_declaration_owner<'a>(
        &self,
        part: &str,
        staff: &str,
        voices: impl Iterator<Item = &'a str>,
        occupied: &BTreeSet<(&str, &str, &str)>,
        budget: &mut ProvenanceBudget,
    ) -> Result<bool> {
        let depth = usize::BITS as usize - self.retained.len().leading_zeros() as usize + 1;
        for voice in voices {
            budget.charge(
                0,
                part.len()
                    .saturating_add(staff.len())
                    .saturating_add(voice.len())
                    .saturating_mul(16),
            )?;
            if occupied.contains(&(part, staff, voice)) {
                continue;
            }
            budget.charge(
                0,
                self.declarations
                    .len()
                    .saturating_mul(depth.saturating_add(4)),
            )?;
            for (id, declaration) in &self.declarations {
                if !self.retained.contains_key(id) {
                    continue;
                }
                let applies = match &declaration.scope {
                    Scope::System => true,
                    Scope::Part(p)
                    | Scope::Instrument { part: p, .. }
                    | Scope::Unsupported { part: p, .. } => p == part,
                    Scope::Staff { part: p, staff: s } => p == part && s == staff,
                    Scope::Voice {
                        part: p,
                        staff: s,
                        voice: v,
                    } => p == part && s.as_deref().is_none_or(|s| s == staff) && v == voice,
                    Scope::Midi { .. } => false,
                };
                if applies {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Independent applicability for a contributor on a performed route. This
    /// permits held prefix state and future paired endpoints within the current
    /// written run. It proves eligibility, not that an instruction was selected.
    /// Callers charge O(runs.len() + time_only.len()) before each query. No copy.
    pub fn declaration_on_route(
        &self,
        id: &str,
        owner: &ScoreVoice,
        occurrence: u32,
        repeat_pass: u32,
    ) -> bool {
        self.declaration_route_pass(id, owner, occurrence, repeat_pass)
            .is_some()
    }

    /// Effective pass at the raw declaration after destination-state reset.
    /// Includes disabled/pass-filtered diagnostic evidence; callers apply its
    /// enabled/time_only fields only when validating active mapping.
    pub fn declaration_route_pass(
        &self,
        id: &str,
        owner: &ScoreVoice,
        occurrence: u32,
        repeat_pass: u32,
    ) -> Option<u32> {
        let declaration = self.declarations.get(id)?;
        let applicable = declaration.scope.applies(owner)
            || matches!(&declaration.scope, Scope::Unsupported { part, .. } if part == &owner.part);
        if !applicable {
            return None;
        }
        if occurrence == 0 || repeat_pass == 0 {
            return (occurrence == 0
                && repeat_pass == 0
                && self.declaration_is_unresolved(id, owner))
            .then_some(0);
        }
        if self.declaration_is_unresolved(id, owner) {
            return None;
        }
        let runs = self.owner_runs(owner)?;
        let index = runs
            .iter()
            .position(|run| run.ordinal == occurrence && run.pass == repeat_pass)?;
        let terminal =
            index + 1 == runs.len() || runs[index + 1].written_start == runs[index].written_end;
        if let Some(original) = self.original_declarations.get(id) {
            if let Some(measure) = original
                .written_measure
                .and_then(|m| self.written_measures.get(m))
            {
                let measure_pass = route_point_pass(runs, index, measure.start, false)?;
                // An endpoint authored at its played measure's right edge is
                // part of that measure even before a jump. A declaration in a
                // skipped following measure at the same time has no such proof.
                if declaration.at == measure.end
                    && [
                        DeclarationKind::WedgeStop,
                        DeclarationKind::WedgeContinue,
                        DeclarationKind::SpannerEndpoint,
                    ]
                    .iter()
                    .any(|kind| original.kinds.contains(kind))
                {
                    return Some(measure_pass);
                }
            }
        }
        route_point_pass(runs, index, declaration.at, terminal)
    }

    pub(super) fn declaration_on_path(
        &self,
        id: &str,
        owner: &ScoreVoice,
        path: Option<(&[PlaybackSpan], bool)>,
    ) -> bool {
        let Some(measure) = self
            .original_declarations
            .get(id)
            .and_then(|o| o.written_measure)
            .and_then(|m| self.written_measures.get(m))
        else {
            return true;
        };
        if self.declaration_is_unresolved(id, owner) {
            return false;
        }
        path.is_none_or(|(spans, _)| {
            let index = spans.partition_point(|span| span.start <= measure.start);
            index > 0 && measure.start < spans[index - 1].end
        })
    }

    fn record_velocity_declaration(
        &mut self,
        node: Node,
        id: &str,
        owner: &ScoreVoice,
        at: Time,
    ) -> Result<()> {
        let Some(velocity) = self.overrides.get(id) else {
            return Ok(());
        };
        self.provenance_budget.charge(
            id.len()
                .saturating_mul(3)
                .saturating_add(owner.part.len())
                .saturating_add(owner.staff.len())
                .saturating_add(owner.voice.len())
                .saturating_add(512),
            1,
        )?;
        let raw_id = velocity.evidence.source_ids[0].clone();
        self.record_original(&raw_id, DeclarationKind::Velocity)?;
        self.original_declarations
            .get_mut(&raw_id)
            .unwrap()
            .note_source_id = Some(id.into());
        self.declarations.insert(
            raw_id,
            Declaration {
                scope: Scope::musicxml(&owner.part, Some(&owner.staff), Some(&owner.voice)),
                at,
                order: node.id().get(),
                enabled: true,
                time_only: vec![],
            },
        );
        Ok(())
    }

    pub fn xml_note_owned(
        &mut self,
        node: Node,
        id: &str,
        owner: &ScoreVoice,
        at: Time,
    ) -> Result<()> {
        self.xml_note(node, id)?;
        self.record_velocity_declaration(node, id, owner, at)
    }

    pub fn ms_note_owned(
        &mut self,
        node: Node,
        id: &str,
        modern: bool,
        owner: &ScoreVoice,
        at: Time,
    ) -> Result<()> {
        self.ms_note(node, id, modern)?;
        self.record_velocity_declaration(node, id, owner, at)
    }

    fn evidence(&mut self, node: Node, id: &str, contract: SourceContract) -> Result<Evidence> {
        // Charge the owned source slice before allocating it or cloning it into
        // the retained inventory. The input document itself has loader limits.
        let bytes = node.range().len().saturating_add(id.len());
        self.captured_bytes = self
            .captured_bytes
            .checked_add(bytes)
            .ok_or("SCORE_INTENSITY_LIMIT: evidence size overflow")?;
        if self.captured_bytes > 32 * 1024 * 1024 {
            return Err("SCORE_INTENSITY_LIMIT: source evidence exceeds 32 MiB".into());
        }
        let root = node.document().root_element();
        let metadata = root
            .attribute("version")
            .map_or(0, str::len)
            .saturating_add(text(root, "programVersion").map_or(0, str::len));
        let copies = bytes
            .saturating_add(metadata)
            .saturating_add(512)
            .saturating_mul(4);
        self.provenance_budget.charge(copies, copies / 8 + 1)?;
        Ok(Evidence {
            source_ids: vec![id.into()],
            contract,
            saving_version: text(root, "programVersion").map(str::to_owned),
            layout: root.attribute("version").unwrap_or("unspecified").into(),
            raw_fields: raw(node),
        })
    }

    pub fn retain(&mut self, e: &Evidence) -> Result<()> {
        if self.retained.len() >= 250_000 {
            return Err("SCORE_INTENSITY_LIMIT: too many declarations".into());
        }
        let bytes = evidence_size(e).saturating_mul(e.source_ids.len());
        self.provenance_budget.charge(bytes, bytes / 8 + 1)?;
        for id in &e.source_ids {
            self.retained.insert(id.clone(), e.clone());
        }
        Ok(())
    }
    pub fn push(&mut self, event: ScoreEvent) -> Result<()> {
        self.retain(&event.evidence)?;
        self.record_declaration(&event)?;
        self.score.events.push(event);
        Ok(())
    }
    fn record_declaration(&mut self, event: &ScoreEvent) -> Result<()> {
        for id in &event.evidence.source_ids {
            if !self.original_declarations.contains_key(id) {
                self.record_original(
                    id,
                    match event.instruction {
                        Instruction::Dynamic(_) => DeclarationKind::Dynamic,
                        Instruction::Transition(_) => DeclarationKind::Transition,
                        Instruction::Text { .. } => DeclarationKind::Text,
                        Instruction::Unsupported(_) => DeclarationKind::Unsupported,
                    },
                )?;
            }
            if !self.declarations.contains_key(id) {
                let bytes = id
                    .len()
                    .saturating_add(scope_size(&event.scope))
                    .saturating_add(event.time_only.len().saturating_mul(4))
                    .saturating_add(256);
                self.provenance_budget.charge(bytes, bytes / 8 + 1)?;
                self.declarations.insert(
                    id.clone(),
                    Declaration {
                        scope: event.scope.clone(),
                        at: event.at,
                        order: event.order,
                        enabled: event.enabled,
                        time_only: event.time_only.clone(),
                    },
                );
            }
        }
        Ok(())
    }
    pub fn record_run(
        &mut self,
        part: &str,
        staff: &str,
        written: (Time, Time),
        performed: Time,
        pass: u32,
        forward: bool,
    ) -> Result<()> {
        if !self
            .route_keys
            .get(part)
            .is_some_and(|staves| staves.contains_key(staff))
        {
            self.provenance_budget.charge(
                part.len()
                    .saturating_add(staff.len())
                    .saturating_mul(2)
                    .saturating_add(256),
                1,
            )?;
            self.route_keys
                .entry(part.into())
                .or_default()
                .insert(staff.into(), (part.into(), staff.into()));
        }
        if written.0 == written.1 {
            self.provenance_budget.charge(
                part.len().saturating_add(staff.len()).saturating_add(128),
                1,
            )?;
            self.declaration_points
                .entry((part.into(), staff.into()))
                .or_default()
                .push(DeclarationPoint {
                    written_at: written.0,
                    performed_at: performed,
                    pass,
                });
            return Ok(());
        }
        let runs = self.runs.entry((part.into(), staff.into())).or_default();
        if let Some(last) = runs.last_mut() {
            if forward
                && last.pass == pass
                && last.written_end == written.0
                && last
                    .performed_start
                    .checked_add(last.written_end.checked_sub(last.written_start)?)?
                    == performed
            {
                last.written_end = written.1;
                return Ok(());
            }
        }
        if written.1 > written.0 {
            if !forward {
                self.provenance_budget
                    .charge(part.len().saturating_add(staff.len()).saturating_add(64), 1)?;
                self.route_breaks
                    .entry((part.into(), staff.into()))
                    .or_default()
                    .insert(runs.len() as u32 + 1);
            }
            runs.push(Occurrence {
                written_start: written.0,
                written_end: written.1,
                performed_start: performed,
                pass,
                ordinal: runs.len() as u32 + 1,
            });
        }
        Ok(())
    }
    pub fn xml_note(&mut self, node: Node, id: &str) -> Result<()> {
        if let Some(value) = node.attribute("dynamics") {
            let e = self.evidence(
                node,
                &format!("expression:{id}:velocity"),
                SourceContract::MusicXml,
            )?;
            self.retain(&e)?;
            match Fraction::decimal(value) {
                Ok(value) => {
                    self.overrides.insert(
                        id.into(),
                        VelocityEvidence {
                            value: AttackVelocity::MusicXml(value),
                            evidence: e,
                        },
                    );
                }
                Err(message) => {
                    self.overrides.insert(
                        id.into(),
                        VelocityEvidence {
                            value: AttackVelocity::Invalid(message),
                            evidence: e,
                        },
                    );
                }
            }
        }
        Ok(())
    }
    pub fn ms_note(&mut self, node: Node, id: &str, modern: bool) -> Result<()> {
        if child(node, "velocity").is_some() {
            let e = self.evidence(
                node,
                &format!("expression:{id}:velocity"),
                if modern {
                    SourceContract::MuseScoreModern
                } else {
                    SourceContract::MuseScoreLegacy
                },
            )?;
            self.retain(&e)?;
            let decoded = (|| -> Result<AttackVelocity> {
                let value = decimal(node, "velocity", &mut self.provenance_budget)?
                    .ok_or("Missing note velocity")?;
                let user = scalar(
                    node,
                    "veloType",
                    &mut self.provenance_budget,
                    |value| match value {
                        "user" | "1" => Ok(true),
                        "offset" | "0" => Ok(false),
                        other => Err(format!("Unknown note veloType {other:?}")),
                    },
                )?
                .unwrap_or(modern);
                Ok(if user {
                    AttackVelocity::MuseScoreUser {
                        value,
                        legacy: !modern,
                    }
                } else {
                    AttackVelocity::MuseScoreOffset {
                        percent: value,
                        legacy: !modern,
                    }
                })
            })();
            let velocity = match decoded {
                Ok(value) => value,
                Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => {
                    return Err(message)
                }
                Err(message) => AttackVelocity::Invalid(message),
            };
            self.overrides.insert(
                id.into(),
                VelocityEvidence {
                    value: velocity,
                    evidence: e,
                },
            );
        }
        Ok(())
    }
    pub fn xml_direction(
        &mut self,
        node: Node,
        id: &str,
        part: &str,
        at: Time,
        divisions: u32,
    ) -> Result<()> {
        let e = self.evidence(node, id, SourceContract::MusicXml)?;
        let decoded_scope = (|| -> Result<Scope> {
            let staff = scalar_text(node, "staff", &mut self.provenance_budget)?;
            let voice = scalar_text(node, "voice", &mut self.provenance_budget)?;
            Ok(Scope::musicxml(part, staff.as_deref(), voice.as_deref()))
        })();
        self.provenance_budget.charge(0, node.range().len())?;
        for piece in node.descendants().filter(|piece| piece.is_element()) {
            let kind = match piece.tag_name().name() {
                "dynamics" => Some(DeclarationKind::Dynamic),
                "sound" if piece.attribute("dynamics").is_some() => Some(DeclarationKind::Dynamic),
                "words" => Some(DeclarationKind::Text),
                "wedge" => Some(match piece.attribute("type") {
                    Some("crescendo" | "diminuendo") => DeclarationKind::WedgeStart,
                    Some("continue") => DeclarationKind::WedgeContinue,
                    Some("stop") => DeclarationKind::WedgeStop,
                    _ => DeclarationKind::Unsupported,
                }),
                _ => None,
            };
            if let Some(kind) = kind {
                self.record_original(id, kind)?;
            }
        }
        // Decode independent facts before interpreting the instruction. An
        // invalid level/wedge must not discard a valid playback offset or pass.
        let position = (|| -> Result<Time> {
            let mut offset = None;
            self.provenance_budget.charge(0, node.range().len())?;
            for sound in node
                .children()
                .chain(std::iter::once(node))
                .filter(|n| n.has_tag_name("sound"))
            {
                if let Some(value) = decimal(sound, "offset", &mut self.provenance_budget)? {
                    agree_value(&mut offset, value, "offset")?;
                }
            }
            if offset.is_none() && !node.has_tag_name("sound") {
                self.provenance_budget.charge(0, node.range().len())?;
                for field in node
                    .children()
                    .filter(|n| n.has_tag_name("offset") && n.attribute("sound") == Some("yes"))
                {
                    agree_value(
                        &mut offset,
                        Fraction::decimal(field.text().unwrap_or("").trim())?,
                        "offset",
                    )?;
                }
            }
            let position = match offset {
                Some(value) => {
                    at.checked_add(value.checked_mul(Fraction::new(1, i64::from(divisions))?)?)?
                }
                None => at,
            };
            if position < Time::ZERO {
                return Err("Expression offset precedes the score".into());
            }
            Ok(position)
        })();
        let passes = sound_attribute(node, "time-only", &mut self.provenance_budget, |raw| {
            let mut passes = raw
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse::<u32>()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or_else(|| format!("Invalid sound time-only {raw:?}"))
                })
                .collect::<Result<Vec<_>>>()?;
            passes.sort_unstable();
            passes.dedup();
            Ok(passes)
        })
        .map(Option::unwrap_or_default);
        let mut problems = Vec::new();
        let scope = match decoded_scope {
            Ok(scope) => scope,
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(message) => {
                problems.push(message.clone());
                Scope::Unsupported {
                    part: part.into(),
                    raw: message,
                }
            }
        };
        let resolved_at = match position {
            Ok(position) => position,
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(message) => {
                problems.push(format!("Unresolved playback offset; diagnostic attached to the written cursor: {message}"));
                // This is only a diagnostic attachment to the source cursor;
                // an invalid offset never supplies an invented active time.
                at
            }
        };
        let time_only = match passes {
            Ok(passes) => passes,
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(message) => {
                problems.push(message);
                vec![]
            }
        };
        if !time_only.is_empty() {
            self.issue_passes.insert(id.into(), time_only.clone());
        }
        let mut base = ScoreEvent { at: resolved_at, order: node.id().get(), scope,
                scope_interpretation: interpretation(id, "scope", "MusicXML part; explicit staff/voice narrow scope; absent qualifiers use portable part scope.", text(node,"staff").is_some() || text(node,"voice").is_some()),
                enabled: true, time_only, evidence: e, instruction: Instruction::Unsupported(String::new()) };
        if !problems.is_empty() {
            base.instruction = Instruction::Unsupported(problems.join("; "));
            return self.push(base);
        }
        // A numeric override and its printed symbol form one instruction. A bad
        // numeric field must not fall back to that symbol, or suppress siblings.
        let dynamics: Vec<_> = node
            .descendants()
            .filter(|n| n.has_tag_name("dynamics"))
            .collect();
        let numeric = sound_attribute(
            node,
            "dynamics",
            &mut self.provenance_budget,
            Fraction::decimal,
        );
        if !numeric.as_ref().is_ok_and(|value| value.is_none()) || !dynamics.is_empty() {
            let instruction = (|| -> Result<Instruction> {
                let numeric = numeric?;
                let symbols: Vec<_> = dynamics
                    .iter()
                    .flat_map(|d| d.children().filter(|n| n.is_element()))
                    .map(|n| {
                        if n.has_tag_name("other-dynamics") {
                            n.text().unwrap_or("").trim().to_owned()
                        } else {
                            n.tag_name().name().into()
                        }
                    })
                    .collect();
                if symbols.len() > 1
                    && (numeric.is_none()
                        || symbols.iter().skip(1).any(|symbol| {
                            dynamic_behavior(symbol) != dynamic_behavior(&symbols[0])
                        }))
                {
                    return Err("Multiple unrelated dynamics in one direction".into());
                }
                Ok(Instruction::Dynamic(Dynamic {
                    symbol: symbols.first().cloned(),
                    numeric: numeric.map(NumericLevel::MusicXml),
                    ..Dynamic::default()
                }))
            })();
            if let Err(message) = &instruction {
                if message.starts_with("SCORE_INTENSITY_LIMIT:") {
                    return Err(message.clone());
                }
            }
            self.provenance_budget.event(&base)?;
            self.push(ScoreEvent {
                instruction: instruction.unwrap_or_else(Instruction::Unsupported),
                ..base.clone()
            })?;
        }
        let label_intents: Vec<_> = node
            .descendants()
            .filter(|n| n.has_tag_name("words"))
            .filter_map(|n| {
                recognized_text(
                    &n.descendants()
                        .filter(|n| n.is_text())
                        .filter_map(|n| n.text())
                        .collect::<String>(),
                )
            })
            .collect();
        for intent in &label_intents {
            let key = match intent {
                TextIntent::Transition(Direction::Crescendo) | TextIntent::FadeIn => {
                    "intensity_label_crescendo"
                }
                TextIntent::Transition(Direction::Diminuendo) | TextIntent::FadeOut => {
                    "intensity_label_diminuendo"
                }
            };
            base.evidence
                .raw_fields
                .insert(key.into(), "Recognized owning-wedge words".into());
        }
        let mut has_valid_wedge = false;
        for wedge in node.descendants().filter(|n| n.has_tag_name("wedge")) {
            let decoded = (|| -> Result<_> {
                let kind = match wedge.attribute("type") {
                    Some("crescendo") => WedgeKind::Start(Direction::Crescendo),
                    Some("diminuendo") => WedgeKind::Start(Direction::Diminuendo),
                    Some("continue") => WedgeKind::Continue,
                    Some("stop") => WedgeKind::Stop,
                    _ => return Err("Unknown MusicXML wedge type".into()),
                };
                let niente = bool_field(wedge.attribute("niente"))?.unwrap_or(false)
                    || label_intents
                        .iter()
                        .any(|intent| matches!(intent, TextIntent::FadeIn | TextIntent::FadeOut));
                Ok((kind, niente))
            })();
            self.provenance_budget.event(&base)?;
            match decoded {
                Ok((kind, niente)) => {
                    self.retain(&base.evidence)?;
                    self.record_declaration(&base)?;
                    self.wedges.push(Wedge {
                        event: base.clone(),
                        number: wedge.attribute("number").map(str::to_owned),
                        kind,
                        niente,
                    });
                    has_valid_wedge = true;
                }
                Err(message) => self.push(ScoreEvent {
                    instruction: Instruction::Unsupported(message),
                    ..base.clone()
                })?,
            }
        }
        for words in node.descendants().filter(|n| n.has_tag_name("words")) {
            let words = words
                .descendants()
                .filter(|n| n.is_text())
                .filter_map(|n| n.text())
                .collect::<String>();
            let owning_wedge = has_valid_wedge && recognized_text(&words).is_some();
            if !(words.trim().is_empty() || owning_wedge) {
                self.provenance_budget.event(&base)?;
                self.push(ScoreEvent {
                    instruction: Instruction::Text {
                        text: words,
                        end: None,
                        owning_spanner: None,
                    },
                    ..base.clone()
                })?;
            }
        }
        Ok(())
    }

    // The nominal loader supplies its cursor and source contract together; keep
    // these explicit to avoid reparsing or maintaining a second timing model.
    #[allow(clippy::too_many_arguments)]
    pub fn ms_element(
        &mut self,
        node: Node,
        id: &str,
        owner: &ScoreVoice,
        at: Time,
        measure: usize,
        modern: bool,
        tempo: Fraction,
        stretch: (i64, i64),
    ) -> Result<()> {
        let spanner = node.has_tag_name("Spanner")
            && matches!(node.attribute("type"), Some("HairPin" | "TextLine"));
        if !spanner
            && !matches!(
                node.tag_name().name(),
                "Dynamic" | "HairPin" | "StaffText" | "SystemText" | "TextLine" | "endSpanner"
            )
        {
            return Ok(());
        }
        if node.has_tag_name("endSpanner") {
            if let Some(legacy_id) = node.attribute("id") {
                self.record_original(id, DeclarationKind::SpannerEndpoint)?;
                let evidence = self.evidence(
                    node,
                    id,
                    if modern {
                        SourceContract::MuseScoreModern
                    } else {
                        SourceContract::MuseScoreLegacy
                    },
                )?;
                self.legacy_ends
                    .entry(format!("{}:{}:{legacy_id}", owner.part, owner.staff))
                    .or_default()
                    .push((at, node.id().get(), evidence));
            }
            return Ok(());
        }
        let payload = if spanner {
            node.children()
                .find(|n| n.has_tag_name("HairPin") || n.has_tag_name("TextLine"))
        } else {
            Some(node)
        };
        // A typed reciprocal stop carries no second instruction, but is still
        // original intensity evidence even when no start can be proved.
        let Some(payload) = payload else {
            let evidence = self.evidence(
                node,
                id,
                if modern {
                    SourceContract::MuseScoreModern
                } else {
                    SourceContract::MuseScoreLegacy
                },
            )?;
            self.record_original(id, DeclarationKind::SpannerEndpoint)?;
            let event = ScoreEvent {
                at,
                order: node.id().get(),
                scope: Scope::musicxml(&owner.part, Some(&owner.staff), Some(&owner.voice)),
                scope_interpretation: interpretation(
                    id,
                    "scope",
                    "Typed endpoint retains its original source staff and voice.",
                    true,
                ),
                enabled: true,
                time_only: vec![],
                evidence,
                instruction: Instruction::Unsupported(
                    "Typed intensity endpoint without a payload".into(),
                ),
            };
            self.retain(&event.evidence)?;
            self.record_declaration(&event)?;
            match spanner_delta(node, "prev", stretch, &mut self.provenance_budget) {
                Ok(delta) => {
                    self.provenance_budget
                        .charge(scope_size(&event.scope).saturating_add(256), 1)?;
                    self.typed_ends.push(TypedEndpoint {
                        event,
                        owner: owner.clone(),
                        measure,
                        delta,
                        kind: node.attribute("type").unwrap().into(),
                    });
                }
                Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => {
                    return Err(message)
                }
                Err(message) => {
                    self.provenance_budget.event(&event)?;
                    self.issues.push(Issue {
                        start: at,
                        end: at,
                        kind: IssueKind::UnresolvedSpan,
                        provenance: Provenance::from_event(&event, 0),
                        message,
                    });
                }
            }
            return Ok(());
        };
        let e = self.evidence(
            node,
            id,
            if modern {
                SourceContract::MuseScoreModern
            } else {
                SourceContract::MuseScoreLegacy
            },
        )?;
        self.record_original(
            id,
            match payload.tag_name().name() {
                "Dynamic" => DeclarationKind::Dynamic,
                "HairPin" | "TextLine" => DeclarationKind::Transition,
                _ => DeclarationKind::Text,
            },
        )?;
        let decoded_scope = (|| -> Result<_> {
            let assignment = scalar(
                payload,
                "voiceAssignment",
                &mut self.provenance_budget,
                |value| match value {
                    "currentVoiceOnly" => Ok(VoiceAssignment::CurrentVoice),
                    "allInStaff" => Ok(VoiceAssignment::StaffVoices),
                    "allInInstrument" => Ok(VoiceAssignment::InstrumentVoices),
                    value => Err(format!("Unknown voiceAssignment {value:?}")),
                },
            )?;
            let legacy = scalar(
                payload,
                "dynType",
                &mut self.provenance_budget,
                |value| match value {
                    "0" | "staff" => Ok(LegacyRange::Staff),
                    "1" | "part" => Ok(LegacyRange::Part),
                    "2" | "system" => Ok(LegacyRange::System),
                    value => Err(format!("Unknown dynType {value:?}")),
                },
            )?;
            let (mut scope, mut reason) = musescore_scope(owner, modern, assignment, legacy);
            if assignment.is_none() && legacy.is_none() {
                if payload.has_tag_name("StaffText") {
                    scope = Scope::Staff {
                        part: owner.part.clone(),
                        staff: owner.staff.clone(),
                    };
                    reason = "StaffText belongs to its source staff unless explicitly assigned otherwise.";
                } else if payload.has_tag_name("SystemText") {
                    scope = Scope::System;
                    reason = "SystemText belongs to the score system unless explicitly assigned otherwise.";
                }
            }
            Ok((scope, reason, assignment.is_some() || legacy.is_some()))
        })();
        let mut problems = Vec::new();
        let (scope, reason, explicit) = match decoded_scope {
            Ok(decoded) => decoded,
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(message) => {
                problems.push(message.clone());
                (
                    Scope::Unsupported {
                        part: owner.part.clone(),
                        raw: message,
                    },
                    "Unresolved source scope",
                    true,
                )
            }
        };
        let enabled = match scalar_bool(payload, "play", &mut self.provenance_budget) {
            Ok(enabled) => enabled.unwrap_or(true),
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(message) => {
                problems.push(message);
                false
            }
        };
        let mut event = ScoreEvent {
            at,
            order: node.id().get(),
            scope,
            scope_interpretation: interpretation(id, "scope", reason, explicit),
            enabled,
            time_only: vec![],
            evidence: e,
            instruction: Instruction::Unsupported(String::new()),
        };
        let mut span_delta = None;
        let mut is_span = false;
        let result = (|| -> Result<()> {
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            if payload.has_tag_name("Dynamic") {
                let speed = scalar(
                    payload,
                    "veloChangeSpeed",
                    &mut self.provenance_budget,
                    |value| match value {
                        "1" | "normal" => Ok(Speed::Normal),
                        "0" | "slow" => Ok(Speed::Slow),
                        "2" | "fast" => Ok(Speed::Fast),
                        value => Err(format!("Unknown velocity speed {value:?}")),
                    },
                )?
                .unwrap_or_default();
                event.instruction = Instruction::Dynamic(Dynamic {
                    symbol: scalar_text(payload, "subtype", &mut self.provenance_budget)?,
                    numeric: decimal(payload, "velocity", &mut self.provenance_budget)?
                        .map(NumericLevel::MuseScoreDynamic),
                    velo_change: decimal(payload, "veloChange", &mut self.provenance_budget)?,
                    speed,
                    tempo_at_start: Some(tempo),
                });
            } else if payload.has_tag_name("HairPin") || payload.has_tag_name("TextLine") {
                let labels: Vec<_> = ["beginText", "text", "endText"]
                    .iter()
                    .map(|tag| rich_text(payload, tag, &mut self.provenance_budget))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                let intents: Vec<_> = labels
                    .iter()
                    .filter_map(|label| recognized_text(label))
                    .collect();
                let text_direction = |intent| match intent {
                    TextIntent::FadeIn | TextIntent::Transition(Direction::Crescendo) => {
                        Direction::Crescendo
                    }
                    TextIntent::FadeOut | TextIntent::Transition(Direction::Diminuendo) => {
                        Direction::Diminuendo
                    }
                };
                let direction =
                    if payload.has_tag_name("TextLine") {
                        text_direction(*intents.first().ok_or_else(|| {
                            format!("Unrecognized text-line intensity {labels:?}")
                        })?)
                    } else {
                        scalar(payload, "subtype", &mut self.provenance_budget, |subtype| {
                            match subtype {
                                "0" | "2" | "crescendo" => Ok(Direction::Crescendo),
                                "1" | "3" | "decrescendo" => Ok(Direction::Diminuendo),
                                _ => Err(format!("Unknown hairpin subtype {subtype:?}")),
                            }
                        })?
                        .unwrap_or(Direction::Crescendo)
                    };
                if intents
                    .iter()
                    .any(|intent| text_direction(*intent) != direction)
                {
                    event.evidence.raw_fields.insert(
                        "intensity_direction_conflict".into(),
                        "Recognized owning-spanner label disagrees with the authored direction"
                            .into(),
                    );
                }
                let mut transition = Transition::new(at, direction);
                transition.velo_change =
                    decimal(payload, "veloChange", &mut self.provenance_budget)?;
                transition.method =
                    scalar_text(payload, "veloChangeMethod", &mut self.provenance_budget)?;
                transition.single_note_dynamics =
                    scalar_bool(payload, "singleNoteDynamics", &mut self.provenance_budget)?;
                let niente = scalar_bool(payload, "nienteCircled", &mut self.provenance_budget)?
                    .unwrap_or(false);
                transition.niente_start = (niente && direction == Direction::Crescendo)
                    || intents.contains(&TextIntent::FadeIn);
                transition.niente_end = (niente && direction == Direction::Diminuendo)
                    || intents.contains(&TextIntent::FadeOut);
                transition.end_level = rich_text(payload, "endText", &mut self.provenance_budget)?
                    .as_deref()
                    .and_then(standard_level);
                event.instruction = Instruction::Transition(transition);
                // Anchor displacements never replace the start's playback scope.
                let delta = spanner_delta(node, "next", stretch, &mut self.provenance_budget)?;
                if let Some(legacy_id) = payload.attribute("id") {
                    event.evidence.raw_fields.insert(
                        "legacy_spanner".into(),
                        format!("{}:{}:{legacy_id}", owner.part, owner.staff),
                    );
                }
                if decimal(payload, "ticks", &mut self.provenance_budget)?.is_some() {
                    let ticks = text(payload, "ticks").ok_or("Missing explicit hairpin ticks")?;
                    event
                        .evidence
                        .raw_fields
                        .insert("span_ticks".into(), ticks.into());
                }
                span_delta = delta;
                is_span = true;
            } else {
                let words =
                    rich_text(payload, "text", &mut self.provenance_budget)?.unwrap_or_default();
                event.instruction = Instruction::Text {
                    text: words,
                    end: None,
                    owning_spanner: None,
                };
            }
            Ok(())
        })();
        if let Err(message) = result {
            if message.starts_with("SCORE_INTENSITY_LIMIT:") {
                return Err(message);
            }
            event.instruction = Instruction::Unsupported(message);
            self.push(event)?;
        } else if is_span {
            self.retain(&event.evidence)?;
            self.record_declaration(&event)?;
            self.spanners.push(PendingSpanner {
                event,
                measure,
                start: at,
                delta: span_delta,
                kind: spanner.then(|| payload.tag_name().name().into()),
            });
        } else {
            self.push(event)?;
        }
        Ok(())
    }
    pub fn ms_tempo(
        &mut self,
        node: Node,
        id: &str,
        at: Time,
        raw: &str,
        modern: bool,
    ) -> Result<Option<Fraction>> {
        let parse_exact = |raw: &str| -> Result<Fraction> {
            let (mantissa, exponent) = raw.split_once(['e', 'E']).unwrap_or((raw, "0"));
            let mantissa = if mantissa.contains('.') {
                mantissa.trim_end_matches('0').trim_end_matches('.')
            } else {
                mantissa
            };
            let value = Fraction::decimal(mantissa)?;
            let exponent = exponent
                .parse::<i32>()
                .map_err(|_| "Unsupported exact tempo exponent")?;
            let scale = Some(exponent.unsigned_abs())
                .filter(|e| *e <= 38)
                .and_then(|e| 10i128.checked_pow(e))
                .ok_or("Unsupported exact tempo scale")?;
            value
                .checked_mul(if exponent >= 0 {
                    Fraction::wide(scale, 1)?
                } else {
                    Fraction::wide(1, scale)?
                })?
                .checked_mul(Fraction::integer(60))
        };
        let exact = match scalar(node, "tempo", &mut self.provenance_budget, parse_exact) {
            Ok(Some(value)) => Some(value),
            Ok(None) => parse_exact(raw).ok(),
            Err(message) if message.starts_with("SCORE_INTENSITY_LIMIT:") => return Err(message),
            Err(_) => None,
        };
        self.tempos
            .entry(at)
            .and_modify(|previous| {
                if *previous != exact {
                    *previous = None;
                }
            })
            .or_insert(exact);
        let evidence = self.evidence(
            node,
            id,
            if modern {
                SourceContract::MuseScoreModern
            } else {
                SourceContract::MuseScoreLegacy
            },
        )?;
        self.record_original(id, DeclarationKind::Tempo)?;
        self.provenance_budget
            .charge(std::mem::size_of::<TempoCandidate>().saturating_add(64), 1)?;
        self.tempo_candidates
            .entry(at)
            .or_default()
            .push(TempoCandidate { exact, evidence });
        self.provenance_budget
            .charge(id.len().saturating_add(256), 1)?;
        self.declarations.insert(
            id.into(),
            Declaration {
                scope: Scope::System,
                at,
                order: node.id().get(),
                enabled: true,
                time_only: vec![],
            },
        );
        Ok(exact)
    }

    pub fn finish_staff(&mut self, bounds: &[(Time, Time)], source_ppq: u16) -> Result<()> {
        let mut resolved = Vec::new();
        for PendingSpanner {
            mut event,
            measure,
            start,
            delta,
            kind,
        } in std::mem::take(&mut self.spanners)
        {
            let mut candidates = Vec::new();
            let mut problems = Vec::new();
            let mut legacy_conflict = false;
            if let Some((dm, fraction)) = delta {
                let located = (|| -> Result<Time> {
                    let target = i64::try_from(measure)
                        .map_err(|_| "Measure overflow")?
                        .checked_add(dm)
                        .ok_or("Spanner measure overflow")?;
                    let from = bounds.get(measure).ok_or("Missing spanner start measure")?;
                    let to = usize::try_from(target)
                        .ok()
                        .and_then(|i| bounds.get(i))
                        .ok_or("Spanner endpoint measure is outside the written score")?;
                    start
                        .checked_add(to.0.checked_sub(from.0)?)?
                        .checked_add(fraction)
                })();
                match located {
                    Ok(end) => candidates.push(end),
                    Err(reason) => problems.push(reason),
                }
            }
            if let Some(id) = event.evidence.raw_fields.get("legacy_spanner") {
                if let Some(ends) = self.legacy_ends.get(id) {
                    let bytes = ends.iter().fold(0usize, |bytes, (_, _, evidence)| {
                        bytes.saturating_add(evidence_size(evidence).saturating_mul(2))
                    });
                    self.provenance_budget.charge(bytes, bytes / 8 + 1)?;
                    let ends = ends.clone();
                    let first = ends.first().map(|(end, _, _)| *end);
                    legacy_conflict = ends.iter().any(|(end, _, _)| Some(*end) != first);
                    for (index, (end, order, evidence)) in ends.into_iter().enumerate() {
                        candidates.push(end);
                        self.retain(&evidence)?;
                        // Endpoint identity belongs to this spanner's scope but
                        // retains its own authored coordinate and source order.
                        for id in &evidence.source_ids {
                            let bytes = id
                                .len()
                                .saturating_add(scope_size(&event.scope))
                                .saturating_add(event.time_only.len().saturating_mul(4))
                                .saturating_add(256);
                            self.provenance_budget.charge(bytes, bytes / 8 + 1)?;
                            self.declarations
                                .entry(id.clone())
                                .or_insert_with(|| Declaration {
                                    scope: event.scope.clone(),
                                    at: end,
                                    order,
                                    enabled: event.enabled,
                                    time_only: event.time_only.clone(),
                                });
                        }
                        event.evidence.source_ids.extend(evidence.source_ids);
                        for (field, raw) in evidence.raw_fields {
                            event
                                .evidence
                                .raw_fields
                                .insert(format!("legacy-end:{index}/{field}"), raw);
                        }
                    }
                }
            }
            if let Some(ticks) = event.evidence.raw_fields.get("span_ticks") {
                match Fraction::decimal(ticks)
                    .and_then(|ticks| ticks.checked_mul(Fraction::new(1, i64::from(source_ppq))?))
                    .and_then(|duration| start.checked_add(duration))
                {
                    Ok(end) => candidates.push(end),
                    Err(reason) => {
                        problems.push(format!("Invalid explicit hairpin ticks: {reason}"))
                    }
                }
            }
            if candidates.iter().any(|end| *end <= start) {
                problems.push("Hairpin endpoint does not define a positive written span".into());
            }
            candidates.retain(|end| *end > start);
            candidates.sort();
            candidates.dedup();
            if candidates.len() > 1 || legacy_conflict {
                problems.push(
                    "Conflicting explicit hairpin endpoints are retained without selecting one"
                        .into(),
                );
                event.instruction = Instruction::Unsupported(problems.join("; "));
            } else if let Some(end) = candidates.first() {
                if let Instruction::Transition(transition) = &mut event.instruction {
                    transition.end = *end;
                }
                if let Some(kind) = kind {
                    self.provenance_budget.charge(
                        scope_size(&event.scope)
                            .saturating_add(strings_size(&event.evidence.source_ids))
                            .saturating_add(256),
                        1,
                    )?;
                    resolved.push((
                        kind,
                        event.scope.clone(),
                        start,
                        *end,
                        event.evidence.source_ids.clone(),
                    ));
                }
                if !problems.is_empty() {
                    self.provenance_budget.event(&event)?;
                    self.issues.push(Issue {
                        start,
                        end: *end,
                        kind: IssueKind::UnresolvedSpan,
                        provenance: Provenance::from_event(&event, 0),
                        message: problems.join("; "),
                    });
                }
            } else {
                problems.push("Hairpin has no positive resolved written span".into());
                event.instruction = Instruction::Unsupported(problems.join("; "));
            }
            self.record_pairs(&event.evidence.source_ids)?;
            self.push(event)?;
        }
        for endpoint in std::mem::take(&mut self.typed_ends) {
            let event = endpoint.event;
            let previous = endpoint.delta.and_then(|(dm, fraction)| {
                let target = i64::try_from(endpoint.measure).ok()?.checked_add(dm)?;
                let from = bounds.get(endpoint.measure)?;
                let to = bounds.get(usize::try_from(target).ok()?)?;
                event
                    .at
                    .checked_add(to.0.checked_sub(from.0).ok()?)
                    .ok()?
                    .checked_add(fraction)
                    .ok()
            });
            self.provenance_budget.charge(
                resolved.len().saturating_mul(8),
                resolved.len().saturating_mul(16),
            )?;
            let matches: Vec<_> = resolved
                .iter()
                .filter(|(kind, scope, start, end, _)| {
                    kind == &endpoint.kind
                        && scope.applies(&endpoint.owner)
                        && previous == Some(*start)
                        && *end == event.at
                })
                .collect();
            if let [(_, _, _, _, ids)] = matches.as_slice() {
                self.provenance_budget.charge(
                    strings_size(ids).saturating_add(strings_size(&event.evidence.source_ids)),
                    ids.len().saturating_add(1),
                )?;
                let mut pair = ids.clone();
                pair.extend(event.evidence.source_ids.iter().cloned());
                self.record_pairs(&pair)?;
            } else {
                self.provenance_budget.event(&event)?;
                self.issues.push(Issue {
                    start: event.at, end: event.at, kind: IssueKind::UnresolvedSpan,
                    provenance: Provenance::from_event(&event, 0),
                    message: "Typed intensity endpoint has no unique reciprocal start in its source staff.".into(),
                });
            }
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        let count = self.wedges.len();
        self.provenance_budget
            .charge(count.saturating_mul(16), count.saturating_mul(16))?;
        // Filter by original written membership before starts can be consumed.
        // Raw evidence and source-only diagnostics remain in the inventories.
        let mut wedges = std::mem::take(&mut self.wedges);
        wedges.retain(|wedge| {
            wedge.event.evidence.source_ids.iter().all(|id| {
                self.original_declarations
                    .get(id)
                    .and_then(|o| o.written_measure)
                    .and_then(|m| self.written_measures.get(m))
                    .is_none_or(|m| m.start < m.end && !m.visits.is_empty())
            })
        });
        let mut budget = std::mem::take(&mut self.provenance_budget);
        let paired = pair_performed_wedges(self, &wedges, &mut budget);
        self.provenance_budget = budget;
        let (events, issues) = paired?;
        for event in &events {
            self.record_pairs(&event.evidence.source_ids)?;
        }
        self.score.events.extend(events);
        self.issues.extend(issues);
        // Tempo-dependent instructions remain intact until the performed route
        // is known. A skipped ending cannot invalidate or retime another pass.
        let needs_timing = self.score.events.iter().any(|event| {
            event.evidence.contract != SourceContract::MusicXml
                && matches!(&event.instruction, Instruction::Dynamic(dynamic)
                    if matches!(dynamic.symbol.as_deref(), Some("fp" | "pf"))
                        || dynamic.velo_change.is_some_and(|v| v != Fraction::ZERO))
        });
        if needs_timing {
            // Retain exact tempo evidence once. Only the tempo actually reached
            // by a compound is copied into that occurrence's interpretations.
            for evidence in self
                .tempo_candidates
                .values()
                .flatten()
                .map(|candidate| &candidate.evidence)
            {
                if self
                    .retained
                    .len()
                    .saturating_add(evidence.source_ids.len())
                    > 250_000
                {
                    return Err("SCORE_INTENSITY_LIMIT: too many declarations".into());
                }
                let bytes = evidence_size(evidence).saturating_mul(evidence.source_ids.len());
                self.provenance_budget.charge(bytes, bytes / 8 + 1)?;
                for id in &evidence.source_ids {
                    self.retained.insert(id.clone(), evidence.clone());
                }
            }
        }
        let bytes: usize = self
            .retained
            .values()
            .flat_map(|e| e.raw_fields.values())
            .map(String::len)
            .sum();
        if bytes > 32 * 1024 * 1024 {
            return Err("SCORE_INTENSITY_LIMIT: source evidence exceeds 32 MiB".into());
        }
        Ok(())
    }
    /// Source-aware timeline seam. The score-only APIs retain their synthetic
    /// tempo_at_start contract; real loaders resolve against the visited route.
    pub fn occurrences(
        &self,
        owner: &ScoreVoice,
        runs: &[Occurrence],
        sounding_ends: &[Time],
    ) -> Result<Arc<Timeline>> {
        self.occurrences_bounded(owner, runs, sounding_ends, &mut ProvenanceBudget::default())
    }
    pub(crate) fn occurrences_bounded(
        &self,
        owner: &ScoreVoice,
        runs: &[Occurrence],
        sounding_ends: &[Time],
        budget: &mut ProvenanceBudget,
    ) -> Result<Arc<Timeline>> {
        self.score
            .occurrences_with_input_bounded(owner, runs, sounding_ends, Some(self), budget)
    }
    pub fn is_empty(&self) -> bool {
        self.retained.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_route_boundary_uses_owner_breaks_and_bounded_borrowed_lookups() {
        let mut input = ScoreInput::default();
        for (staff, forward) in [("1", true), ("2", false)] {
            input
                .record_run("P1", staff, (Time::ZERO, Time::ONE), Time::ZERO, 2, false)
                .unwrap();
            input
                .record_run(
                    "P1",
                    staff,
                    (Time::ONE, Time::integer(2)),
                    Time::ONE,
                    1,
                    forward,
                )
                .unwrap();
        }
        let owner = owner();
        let runs = input.owner_runs(&owner).unwrap();
        assert_eq!((runs[0].pass, runs[1].pass), (2, 1));
        let mut measured = ProvenanceBudget::with_limits(0, MAX_RESOLUTION_WORK);
        assert!(input
            .forward_route_boundary(&owner, &runs[0], &runs[1], &mut measured)
            .unwrap());
        assert_eq!(measured.bytes, 0, "the helper borrows every route key");
        assert!(
            measured.work > 16,
            "lookup work is charged as well as coordinates"
        );
        let mut denied = ProvenanceBudget::with_limits(0, measured.work - 1);
        assert!(input
            .forward_route_boundary(&owner, &runs[0], &runs[1], &mut denied)
            .is_err());

        let other = ScoreVoice {
            staff: "2".into(),
            ..owner.clone()
        };
        let other_runs = input.owner_runs(&other).unwrap();
        assert!(
            !input
                .forward_route_boundary(
                    &other,
                    &other_runs[0],
                    &other_runs[1],
                    &mut ProvenanceBudget::default()
                )
                .unwrap(),
            "a recorded navigation break wins over coordinate adjacency"
        );
        for field in ["written", "performed"] {
            let mut next = runs[1].clone();
            if field == "written" {
                next.written_start = Time::integer(2);
            } else {
                next.performed_start = Time::integer(2);
            }
            assert!(
                !input
                    .forward_route_boundary(
                        &owner,
                        &runs[0],
                        &next,
                        &mut ProvenanceBudget::default()
                    )
                    .unwrap(),
                "{field} gap"
            );
        }
        for other in [
            ScoreVoice {
                part: "unknown".into(),
                ..owner.clone()
            },
            ScoreVoice {
                staff: "unknown".into(),
                ..owner.clone()
            },
        ] {
            assert!(!input
                .forward_route_boundary(
                    &other,
                    &runs[0],
                    &runs[1],
                    &mut ProvenanceBudget::default()
                )
                .unwrap());
        }
    }

    fn owner() -> ScoreVoice {
        ScoreVoice {
            part: "P1".into(),
            staff: "1".into(),
            voice: "1".into(),
            instrument: None,
        }
    }

    fn review5_wedge_route(
        forward: bool,
        stop_pass: u32,
        disabled: bool,
        skipped: bool,
    ) -> ScoreInput {
        let document = roxmltree::Document::parse(r#"<root>
            <direction><direction-type><wedge type="crescendo" niente="yes"/></direction-type><staff>1</staff><sound time-only="2"/></direction>
            <direction><direction-type><wedge type="stop"/><dynamics><f/></dynamics></direction-type><staff>1</staff><sound time-only="1"/></direction>
        </root>"#).unwrap();
        let mut input = ScoreInput::default();
        for (index, node) in document
            .root_element()
            .children()
            .filter(|n| n.is_element())
            .enumerate()
        {
            input
                .begin_written_measure("P1", None, index, Time::integer(index as i64))
                .unwrap();
            input
                .xml_direction(
                    node,
                    if index == 0 { "start" } else { "stop" },
                    "P1",
                    if index == 0 {
                        Time::ZERO
                    } else {
                        Fraction::new(3, 2).unwrap()
                    },
                    480,
                )
                .unwrap();
            input
                .end_written_measure(Time::integer(index as i64 + 1))
                .unwrap();
        }
        input
            .begin_written_measure("P1", None, 2, Time::integer(2))
            .unwrap();
        input.end_written_measure(Time::integer(3)).unwrap();
        if disabled {
            input.wedges[1].event.enabled = false;
            input.declarations.get_mut("stop").unwrap().enabled = false;
        }
        input
            .record_measure_run(0, "P1", "1", Time::ZERO, 1, false)
            .unwrap();
        input
            .record_measure_run(0, "P1", "1", Time::ONE, 2, false)
            .unwrap();
        input
            .record_measure_run(
                if skipped { 2 } else { 1 },
                "P1",
                "1",
                Time::integer(2),
                stop_pass,
                forward,
            )
            .unwrap();
        input.finish().unwrap();
        input
    }

    #[test]
    fn source_review5_wedges_cross_contiguous_pass_labels_but_not_jumps_or_exclusions() {
        let input = review5_wedge_route(true, 1, false, false);
        let transitions: Vec<_> = input
            .score
            .events
            .iter()
            .filter(|event| matches!(event.instruction, Instruction::Transition(_)))
            .collect();
        assert_eq!(transitions.len(), 1);
        let event = transitions[0];
        assert_eq!(event.time_only, [2]);
        assert_eq!(event.evidence.source_ids, ["start", "stop"]);
        assert!(
            matches!(&event.instruction, Instruction::Transition(t) if t.end == Fraction::new(3, 2).unwrap())
        );
        assert_eq!(input.declarations["start"].time_only, [2]);
        assert_eq!(input.declarations["stop"].time_only, [1]);
        let runs = input.owner_runs(&owner()).unwrap();
        let timeline = input
            .occurrences(
                &owner(),
                runs,
                &runs.iter().map(|r| r.written_end).collect::<Vec<_>>(),
            )
            .unwrap();
        assert!(timeline
            .segments
            .iter()
            .any(|segment| segment.start >= Time::integer(2)
                && segment.provenance.as_ref().is_some_and(|p| p
                    .evidence
                    .iter()
                    .any(|e| e.source_ids == ["start", "stop"]))));
        for (forward, pass, disabled, skipped) in [
            (false, 1, false, false),
            (true, 2, false, false),
            (true, 1, true, false),
            (true, 1, false, true),
        ] {
            let negative = review5_wedge_route(forward, pass, disabled, skipped);
            assert!(
                !negative
                    .score
                    .events
                    .iter()
                    .any(|event| matches!(event.instruction, Instruction::Transition(_))),
                "forward={forward} pass={pass} disabled={disabled} skipped={skipped}"
            );
            assert!(negative.retained.contains_key("stop"));
            if disabled {
                assert!(negative
                    .issues
                    .iter()
                    .any(|i| i.kind == IssueKind::Disabled));
            }
            if pass == 2 {
                assert!(negative
                    .issues
                    .iter()
                    .any(|i| i.kind == IssueKind::PassFiltered));
            }
        }
    }

    #[test]
    fn source_review5_typed_orphan_endpoints_keep_original_ownership_and_generic_ends_stay_generic()
    {
        for kind in ["HairPin", "TextLine"] {
            let xml = format!(
                r#"<Spanner type="{kind}"><prev><location><fractions>-1/4</fractions></location></prev></Spanner>"#
            );
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            let mut endpoint_owner = owner();
            endpoint_owner.voice = "7".into();
            input
                .begin_written_measure("P1", Some("1"), 0, Time::ZERO)
                .unwrap();
            input
                .ms_element(
                    document.root_element(),
                    "endpoint",
                    &endpoint_owner,
                    Time::ONE,
                    0,
                    true,
                    Fraction::integer(120),
                    (1, 1),
                )
                .unwrap();
            input.end_written_measure(Time::integer(2)).unwrap();
            input
                .finish_staff(&[(Time::ZERO, Time::integer(2))], 480)
                .unwrap();
            input.finish().unwrap();
            assert_eq!(input.retained["endpoint"].raw_fields["xml"], xml);
            assert_eq!(
                input.declarations["endpoint"].scope,
                Scope::musicxml("P1", Some("1"), Some("7"))
            );
            assert_eq!(input.declarations["endpoint"].at, Time::ONE);
            assert_eq!(
                input.original_declarations["endpoint"].written_measure,
                Some(0)
            );
            assert!(input.original_declarations["endpoint"]
                .kinds
                .contains(&DeclarationKind::SpannerEndpoint));
            assert_eq!(input.issues.len(), 1);
            assert_eq!(
                (
                    input.issues[0].start,
                    input.issues[0].end,
                    input.issues[0].kind
                ),
                (Time::ONE, Time::ONE, IssueKind::UnresolvedSpan)
            );
            assert!(input.score.events.is_empty());
        }
        for xml in [
            r#"<endSpanner id="unmatched"/>"#,
            r#"<Spanner type="Slur"><prev/></Spanner>"#,
        ] {
            let document = roxmltree::Document::parse(xml).unwrap();
            let mut input = ScoreInput::default();
            input
                .ms_element(
                    document.root_element(),
                    "generic",
                    &owner(),
                    Time::ONE,
                    0,
                    false,
                    Fraction::integer(120),
                    (1, 1),
                )
                .unwrap();
            input
                .finish_staff(&[(Time::ZERO, Time::integer(2))], 480)
                .unwrap();
            input.finish().unwrap();
            assert!(
                input.retained.is_empty()
                    && input.declarations.is_empty()
                    && input.issues.is_empty()
            );
        }
    }

    #[test]
    fn source_review5_reciprocal_typed_endpoint_is_retained_without_an_orphan_warning() {
        for kind in ["HairPin", "TextLine"] {
            let xml = format!(
                r#"<root><Spanner type="{kind}"><{kind}><beginText>cresc.</beginText></{kind}><next><location><fractions>1/4</fractions></location></next></Spanner><Spanner type="{kind}"><prev><location><fractions>-1/4</fractions></location></prev></Spanner></root>"#
            );
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            for (index, node) in document
                .root_element()
                .children()
                .filter(|n| n.is_element())
                .enumerate()
            {
                input
                    .ms_element(
                        node,
                        if index == 0 { "start" } else { "stop" },
                        &owner(),
                        Time::integer(index as i64),
                        0,
                        true,
                        Fraction::integer(120),
                        (1, 1),
                    )
                    .unwrap();
            }
            input
                .finish_staff(&[(Time::ZERO, Time::integer(2))], 480)
                .unwrap();
            assert!(input.issues.is_empty());
            assert!(input.retained.contains_key("stop"));
            assert!(input.paired_declarations["start"].contains("stop"));
            assert!(
                matches!(&input.score.events[0].instruction, Instruction::Transition(t) if t.end == Time::ONE)
            );
        }
    }

    #[test]
    fn source_review5_note_scalars_coalesce_semantically_equal_values_and_retain_conflicts() {
        for modern in [false, true] {
            for (velocity, mode, valid) in [
                ("64.00", "1", true),
                ("65", "1", false),
                ("64", "offset", false),
            ] {
                let xml = format!("<Note><velocity>64</velocity><velocity>{velocity}</velocity><veloType>user</veloType><veloType>{mode}</veloType></Note>");
                let document = roxmltree::Document::parse(&xml).unwrap();
                let mut input = ScoreInput::default();
                input
                    .ms_note_owned(document.root_element(), "note", modern, &owner(), Time::ONE)
                    .unwrap();
                assert_eq!(
                    text(document.root_element(), "velocity"),
                    Some("64"),
                    "nominal helper remains first-child"
                );
                assert_eq!(
                    matches!(input.overrides["note"].value, AttackVelocity::MuseScoreUser { value, .. } if value == Fraction::integer(64)),
                    valid
                );
                assert_eq!(
                    matches!(input.overrides["note"].value, AttackVelocity::Invalid(_)),
                    !valid
                );
                assert_eq!(
                    input.retained["expression:note:velocity"].raw_fields["xml"],
                    xml
                );
                assert_eq!(input.declarations["expression:note:velocity"].at, Time::ONE);
            }
        }
    }

    #[test]
    fn source_review5_dynamic_scalar_conflicts_cannot_select_active_arithmetic() {
        for fields in [
            "<velocity>40</velocity><velocity>90</velocity>",
            "<veloChange>10</veloChange><veloChange>20</veloChange>",
            "<voiceAssignment>currentVoiceOnly</voiceAssignment><voiceAssignment>allInStaff</voiceAssignment>",
            "<dynType>0</dynType><dynType>2</dynType>",
            "<play>1</play><play>0</play>",
            "<veloChangeSpeed>fast</veloChangeSpeed><veloChangeSpeed>slow</veloChangeSpeed>",
        ] {
            let xml = format!("<Dynamic><subtype>f</subtype>{fields}</Dynamic>");
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            input.ms_element(document.root_element(), "conflict", &owner(), Time::ONE, 0, true, Fraction::integer(120), (1, 1)).unwrap();
            assert!(matches!(&input.score.events[0].instruction, Instruction::Unsupported(message) if message.contains("Conflicting repeated")), "{fields}");
            assert_eq!(input.retained["conflict"].raw_fields["xml"], xml);
        }
        let document = roxmltree::Document::parse("<Dynamic><subtype>f</subtype><velocity>64</velocity><velocity>64.0</velocity><dynType>staff</dynType><dynType>0</dynType><play>true</play><play>1</play><veloChangeSpeed>fast</veloChangeSpeed><veloChangeSpeed>2</veloChangeSpeed></Dynamic>").unwrap();
        let mut input = ScoreInput::default();
        input
            .ms_element(
                document.root_element(),
                "equal",
                &owner(),
                Time::ZERO,
                0,
                true,
                Fraction::integer(120),
                (1, 1),
            )
            .unwrap();
        assert!(
            matches!(&input.score.events[0].instruction, Instruction::Dynamic(d) if d.numeric == Some(NumericLevel::MuseScoreDynamic(Fraction::integer(64))) && d.speed == Speed::Fast)
        );
    }

    #[test]
    fn source_review5_spanner_timing_and_text_conflicts_remain_inactive() {
        for (fields, location) in [
            ("<ticks>480</ticks><ticks>960</ticks>", ""),
            ("<beginText>cresc.</beginText><beginText>dim.</beginText>", ""),
            ("<singleNoteDynamics>1</singleNoteDynamics><singleNoteDynamics>0</singleNoteDynamics>", ""),
            ("", "<measures>0</measures><measures>1</measures>"),
            ("", "<fractions>1/4</fractions><fractions>1/2</fractions>"),
            ("", "<voices>0</voices><voices>1</voices>"),
        ] {
            let xml = format!(r#"<Spanner type="HairPin"><HairPin><subtype>0</subtype>{fields}</HairPin><next><location>{location}</location></next></Spanner>"#);
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            input.ms_element(document.root_element(), "span", &owner(), Time::ZERO, 0, true, Fraction::integer(120), (1, 1)).unwrap();
            input.finish_staff(&[(Time::ZERO, Time::integer(4))], 480).unwrap();
            assert!(matches!(&input.score.events[0].instruction, Instruction::Unsupported(message) if message.contains("Conflicting repeated")), "{xml}");
        }
        let document = roxmltree::Document::parse(r#"<Spanner type="HairPin"><HairPin><ticks>480</ticks><ticks>480.0</ticks><subtype>0</subtype><subtype>crescendo</subtype></HairPin><next><location><fractions>1/4</fractions><fractions>2/8</fractions></location></next></Spanner>"#).unwrap();
        let mut input = ScoreInput::default();
        input
            .ms_element(
                document.root_element(),
                "equal-span",
                &owner(),
                Time::ZERO,
                0,
                true,
                Fraction::integer(120),
                (1, 1),
            )
            .unwrap();
        input
            .finish_staff(&[(Time::ZERO, Time::integer(4))], 480)
            .unwrap();
        assert!(
            matches!(&input.score.events[0].instruction, Instruction::Transition(t) if t.end == Time::ONE)
        );
        assert!(input.issues.is_empty());
    }

    #[test]
    fn source_review5_musicxml_scalar_conflicts_preserve_source_and_valid_timing() {
        for fields in [
            "<staff>1</staff><staff>2</staff>",
            "<voice>1</voice><voice>2</voice>",
            "<sound><offset>0</offset><offset>480</offset></sound>",
            "<offset sound=\"yes\">0</offset><offset sound=\"yes\">480</offset>",
            "<sound dynamics=\"40\"/><sound dynamics=\"90\"/>",
            "<sound time-only=\"1\"/><sound time-only=\"2\"/>",
        ] {
            let xml = format!("<direction><direction-type><dynamics><f/></dynamics></direction-type>{fields}</direction>");
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            input
                .xml_direction(
                    document.root_element(),
                    "xml-conflict",
                    "P1",
                    Time::ONE,
                    480,
                )
                .unwrap();
            assert!(
                matches!(&input.score.events[0].instruction, Instruction::Unsupported(message) if message.contains("Conflicting repeated")),
                "{xml}"
            );
            assert_eq!(input.retained["xml-conflict"].raw_fields["xml"], xml);
        }
        let document = roxmltree::Document::parse(r#"<direction><staff>1</staff><staff>1</staff><sound dynamics="64" time-only="2,1"><offset>480</offset><offset>480.0</offset></sound><sound dynamics="64.0" time-only="1,2"/></direction>"#).unwrap();
        let mut input = ScoreInput::default();
        input
            .xml_direction(document.root_element(), "equal-xml", "P1", Time::ONE, 480)
            .unwrap();
        assert_eq!(input.score.events[0].at, Time::integer(2));
        assert_eq!(input.score.events[0].time_only, [1, 2]);
        assert!(matches!(
            &input.score.events[0].instruction,
            Instruction::Dynamic(_)
        ));
    }

    #[test]
    fn source_review5_repeated_tempo_and_scalar_scans_are_bounded() {
        for (other, expected) in [("2.0e0", Some(Fraction::integer(120))), ("3", None)] {
            let xml = format!("<Tempo><tempo>2</tempo><tempo>{other}</tempo></Tempo>");
            let document = roxmltree::Document::parse(&xml).unwrap();
            let mut input = ScoreInput::default();
            assert_eq!(
                input
                    .ms_tempo(document.root_element(), "tempo", Time::ZERO, "2", true)
                    .unwrap(),
                expected
            );
            assert_eq!(
                input.tempo_candidates[&Time::ZERO][0].evidence.raw_fields["xml"],
                xml
            );
            let mut denied = ProvenanceBudget::with_limits(MAX_PROVENANCE_BYTES, 0);
            assert!(decimal(document.root_element(), "tempo", &mut denied)
                .unwrap_err()
                .starts_with("SCORE_INTENSITY_LIMIT:"));
        }
    }

    #[test]
    fn source_review4_declarations_preserve_unmatched_wedges_and_zero_width_routes() {
        let document = roxmltree::Document::parse(r#"<direction><direction-type><wedge type="crescendo"/></direction-type><staff>1</staff><voice>2</voice><sound time-only="2"/></direction>"#).unwrap();
        let mut input = ScoreInput::default();
        input
            .xml_direction(
                document.root_element(),
                "unmatched",
                "P1",
                Time::integer(3),
                480,
            )
            .unwrap();
        input
            .record_run(
                "P1",
                "1",
                (Time::integer(3), Time::integer(3)),
                Time::integer(9),
                2,
                false,
            )
            .unwrap();
        input.finish().unwrap();
        assert!(input.runs.is_empty());
        assert_eq!(
            input.declaration_points[&("P1".into(), "1".into())],
            [DeclarationPoint {
                written_at: Time::integer(3),
                performed_at: Time::integer(9),
                pass: 2,
            }]
        );
        assert_eq!(
            input.declarations["unmatched"],
            Declaration {
                scope: Scope::musicxml("P1", Some("1"), Some("2")),
                at: Time::integer(3),
                order: document.root_element().id().get(),
                enabled: true,
                time_only: vec![2],
            }
        );
        assert!(input.retained.contains_key("unmatched"));
        assert_eq!(input.issues.len(), 1);
        assert_eq!(input.issues[0].kind, IssueKind::UnresolvedSpan);
        assert!(!input
            .issues
            .iter()
            .any(|issue| issue.kind == IssueKind::PassFiltered));
        // Recording a later real span keeps its normal ordinal and duration.
        input
            .record_run(
                "P1",
                "1",
                (Time::integer(3), Time::integer(4)),
                Time::integer(9),
                2,
                true,
            )
            .unwrap();
        assert_eq!(
            input.runs[&("P1".into(), "1".into())][0],
            Occurrence {
                written_start: Time::integer(3),
                written_end: Time::integer(4),
                performed_start: Time::integer(9),
                pass: 2,
                ordinal: 1,
            }
        );
    }

    #[test]
    fn source_review4_malformed_tempo_and_disabled_instruction_keep_typed_locations() {
        let document = roxmltree::Document::parse(r#"<museScore version="4.70"><Tempo><tempo>1.0000000000000000001</tempo></Tempo><Dynamic><subtype>fp</subtype><play>0</play><voiceAssignment>currentVoiceOnly</voiceAssignment><velocity>bad</velocity></Dynamic><Dynamic><subtype>fp</subtype></Dynamic></museScore>"#).unwrap();
        let root = document.root_element();
        let mut input = ScoreInput::default();
        let tempo = child(root, "Tempo").unwrap();
        assert_eq!(
            input
                .ms_tempo(
                    tempo,
                    "tempo",
                    Time::integer(1),
                    text(tempo, "tempo").unwrap(),
                    true
                )
                .unwrap(),
            None
        );
        let dynamics: Vec<_> = root
            .children()
            .filter(|n| n.has_tag_name("Dynamic"))
            .collect();
        input
            .ms_element(
                dynamics[0],
                "disabled",
                &owner(),
                Time::integer(2),
                0,
                true,
                Fraction::integer(120),
                (1, 1),
            )
            .unwrap();
        input
            .ms_element(
                dynamics[1],
                "compound",
                &owner(),
                Time::integer(3),
                0,
                true,
                Fraction::integer(120),
                (1, 1),
            )
            .unwrap();
        input.finish().unwrap();
        assert!(input
            .retained
            .keys()
            .all(|id| input.declarations.contains_key(id)));
        assert_eq!(input.declarations["tempo"].scope, Scope::System);
        assert_eq!(input.declarations["tempo"].at, Time::integer(1));
        let disabled = &input.declarations["disabled"];
        assert_eq!(disabled.scope, Scope::musicxml("P1", Some("1"), Some("1")));
        assert_eq!(
            (disabled.at, disabled.enabled, disabled.order),
            (Time::integer(2), false, dynamics[0].id().get())
        );
        assert!(
            matches!(&input.score.events[0].instruction, Instruction::Unsupported(message) if message.contains("bad"))
        );
        assert!(
            matches!(input.score.events[1].instruction, Instruction::Dynamic(_)),
            "finish must not rewrite compounds using written tempo"
        );
    }

    #[test]
    fn source_review4_sibling_decoding_propagates_provenance_limits() {
        let document = roxmltree::Document::parse(r#"<direction><direction-type><wedge type="unknown"/><wedge type="crescendo"/><words>dolce</words></direction-type><sound dynamics="bad"/></direction>"#).unwrap();
        // Evidence itself fits, but decoding/copying multiple siblings does not.
        let mut input = ScoreInput {
            provenance_budget: ProvenanceBudget::with_limits(8000, MAX_RESOLUTION_WORK),
            ..ScoreInput::default()
        };
        let error = input
            .xml_direction(document.root_element(), "siblings", "P1", Time::ZERO, 480)
            .unwrap_err();
        assert!(error.starts_with("SCORE_INTENSITY_LIMIT:"));
        assert!(input.score.events.len() < 4);
    }

    #[test]
    fn source_review4_tempo_route_work_uses_the_shared_budget() {
        let document = roxmltree::Document::parse("<Tempo><tempo>2</tempo></Tempo>").unwrap();
        let mut input = ScoreInput::default();
        input
            .ms_tempo(document.root_element(), "tempo", Time::ZERO, "2", false)
            .unwrap();
        let runs = [Occurrence {
            written_start: Time::ZERO,
            written_end: Time::ONE,
            performed_start: Time::ZERO,
            pass: 1,
            ordinal: 1,
        }];
        let mut budget = ProvenanceBudget::with_limits(0, MAX_RESOLUTION_WORK);
        assert!(input
            .occurrences_bounded(&owner(), &runs, &[Time::ONE], &mut budget)
            .is_err_and(|message| message.contains("SCORE_INTENSITY_LIMIT")));
    }

    #[test]
    fn source_review4_pass_filtered_compound_does_not_consume_invalid_tempo() {
        let document = roxmltree::Document::parse("<root><Tempo><tempo>1.0000000000000000001</tempo></Tempo><Dynamic><subtype>fp</subtype></Dynamic></root>").unwrap();
        let root = document.root_element();
        let mut input = ScoreInput::default();
        input
            .ms_tempo(
                child(root, "Tempo").unwrap(),
                "tempo",
                Time::ZERO,
                "1.0000000000000000001",
                false,
            )
            .unwrap();
        input
            .ms_element(
                child(root, "Dynamic").unwrap(),
                "compound",
                &owner(),
                Time::ZERO,
                0,
                false,
                Fraction::integer(120),
                (1, 1),
            )
            .unwrap();
        input.score.events[0].time_only = vec![2];
        input.finish().unwrap();
        let runs = [
            Occurrence {
                written_start: Time::ZERO,
                written_end: Time::ONE,
                performed_start: Time::ZERO,
                pass: 1,
                ordinal: 1,
            },
            Occurrence {
                written_start: Time::ZERO,
                written_end: Time::ONE,
                performed_start: Time::ONE,
                pass: 2,
                ordinal: 2,
            },
        ];
        let timeline = input
            .occurrences(&owner(), &runs, &[Time::ONE, Time::ONE])
            .unwrap();
        assert!(timeline
            .issues
            .iter()
            .any(|i| i.start == Time::ZERO && i.kind == IssueKind::PassFiltered));
        let precision: Vec<_> = timeline
            .issues
            .iter()
            .filter(|i| i.message.contains("precision"))
            .collect();
        assert_eq!(precision.len(), 1);
        assert_eq!(
            (precision[0].start, precision[0].provenance.repeat_pass),
            (Time::ONE, 2)
        );
        assert_eq!(timeline.evaluate(Time::ZERO), Evaluation::Absent);
    }

    #[test]
    fn source_review4_ambiguous_points_are_filtered_before_state_and_endpoint_resolution() {
        let document = roxmltree::Document::parse(r#"<root>
            <direction><direction-type><dynamics><p/></dynamics></direction-type></direction>
            <direction><direction-type><wedge type="crescendo"/></direction-type></direction>
            <direction><direction-type><dynamics><f/></dynamics><wedge type="stop"/></direction-type></direction>
            <direction><direction-type><dynamics><mp/></dynamics></direction-type></direction>
        </root>"#).unwrap();
        let mut input = ScoreInput::default();
        input
            .begin_written_measure("P1", None, 0, Time::ZERO)
            .unwrap();
        for (index, node) in document
            .root_element()
            .children()
            .filter(|n| n.is_element())
            .enumerate()
        {
            if index == 2 || index == 3 {
                input.end_written_measure(Time::integer(2)).unwrap();
                input
                    .begin_written_measure("P1", None, index - 1, Time::integer(2))
                    .unwrap();
            }
            input
                .xml_direction(
                    node,
                    &format!("direction-{index}"),
                    "P1",
                    Time::integer(index as i64),
                    480,
                )
                .unwrap();
        }
        input.end_written_measure(Time::integer(4)).unwrap();
        for (index, start) in [Time::ZERO, Time::integer(2), Time::integer(2)]
            .into_iter()
            .enumerate()
        {
            input
                .record_measure_run(index, "P1", "1", start, 1, true)
                .unwrap();
        }
        input.finish().unwrap();
        let runs = &input.runs[&("P1".into(), "1".into())];
        assert_eq!(runs.len(), 1);
        let timeline = input
            .occurrences(&owner(), runs, &[Time::integer(4)])
            .unwrap();
        for at in [Fraction::new(3, 2).unwrap(), Fraction::new(5, 2).unwrap()] {
            assert_eq!(
                timeline.evaluate(at),
                Evaluation::Known(Intensity::Decibels(-7.75))
            );
        }
        assert_eq!(
            timeline.evaluate(Time::integer(3)),
            Evaluation::Known(Intensity::Decibels(-4.0))
        );
        assert!(timeline
            .segments
            .iter()
            .filter_map(|s| s.provenance.as_ref())
            .flat_map(|p| &p.evidence)
            .all(|e| !e
                .source_ids
                .iter()
                .any(|id| id == "direction-1" || id == "direction-2")));
        assert!(input.retained.contains_key("direction-2"));
    }

    #[test]
    fn repeated_retention_is_charged_before_replacing_or_cloning_evidence() {
        let document = roxmltree::Document::parse("<note dynamics=\"100\"/>").unwrap();
        let mut input = ScoreInput::default();
        let evidence = input
            .evidence(document.root_element(), "note", SourceContract::MusicXml)
            .unwrap();
        input.retain(&evidence).unwrap();
        let retained = input.retained.clone();
        input.provenance_budget.bytes = MAX_PROVENANCE_BYTES;
        assert!(input
            .retain(&evidence)
            .unwrap_err()
            .contains("SCORE_INTENSITY_LIMIT"));
        assert_eq!(input.retained, retained);
    }

    #[test]
    fn source_evidence_limit_is_charged_before_owning_another_xml_slice() {
        let document = roxmltree::Document::parse("<note dynamics=\"100\"/>").unwrap();
        let mut input = ScoreInput {
            captured_bytes: 32 * 1024 * 1024,
            ..ScoreInput::default()
        };
        let error = input
            .xml_note(document.root_element(), "note-bound")
            .unwrap_err();
        assert!(error.contains("SCORE_INTENSITY_LIMIT"));
        assert!(input.retained.is_empty() && input.overrides.is_empty());
    }
}
