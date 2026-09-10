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
}

pub fn child<'a>(node: Node<'a, '_>, tag: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|n| n.has_tag_name(tag))
}
pub fn text<'a>(node: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(node, tag).and_then(|n| n.text()).map(str::trim)
}
fn decimal(node: Node, tag: &str) -> Result<Option<Fraction>> {
    text(node, tag).map(Fraction::decimal).transpose()
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
        if let Some(raw_value) = text(node, "velocity") {
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
            let value = match Fraction::decimal(raw_value) {
                Ok(value) => value,
                Err(message) => {
                    self.overrides.insert(
                        id.into(),
                        VelocityEvidence {
                            value: AttackVelocity::Invalid(message),
                            evidence: e,
                        },
                    );
                    return Ok(());
                }
            };
            let velocity = match text(node, "veloType") {
                Some("user" | "1") => AttackVelocity::MuseScoreUser {
                    value,
                    legacy: !modern,
                },
                None if modern => AttackVelocity::MuseScoreUser {
                    value,
                    legacy: false,
                },
                None | Some("offset" | "0") => AttackVelocity::MuseScoreOffset {
                    percent: value,
                    legacy: !modern,
                },
                Some(other) => AttackVelocity::Invalid(format!("Unknown note veloType {other:?}")),
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
        let sound = if node.has_tag_name("sound") {
            Some(node)
        } else {
            child(node, "sound")
        };
        let scope = Scope::musicxml(part, text(node, "staff"), text(node, "voice"));
        let e = self.evidence(node, id, SourceContract::MusicXml)?;
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
            let offset = sound
                .and_then(|n| child(n, "offset"))
                .or_else(|| child(node, "offset").filter(|o| o.attribute("sound") == Some("yes")));
            let position = match offset {
                Some(offset) => {
                    let value = Fraction::decimal(offset.text().unwrap_or(""))?;
                    at.checked_add(value.checked_mul(Fraction::new(1, i64::from(divisions))?)?)?
                }
                None => at,
            };
            if position < Time::ZERO {
                return Err("Expression offset precedes the score".into());
            }
            Ok(position)
        })();
        let passes = match sound.and_then(|s| s.attribute("time-only")) {
            None => Ok(vec![]),
            Some(raw) => raw
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse::<u32>()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or_else(|| format!("Invalid sound time-only {raw:?}"))
                })
                .collect::<Result<Vec<_>>>(),
        };
        let mut problems = Vec::new();
        let resolved_at = match position {
            Ok(position) => position,
            Err(message) => {
                problems.push(format!("Unresolved playback offset; diagnostic attached to the written cursor: {message}"));
                // This is only a diagnostic attachment to the source cursor;
                // an invalid offset never supplies an invented active time.
                at
            }
        };
        let time_only = match passes {
            Ok(passes) => passes,
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
        let raw_numeric = sound.and_then(|n| n.attribute("dynamics"));
        if raw_numeric.is_some() || !dynamics.is_empty() {
            let instruction = (|| -> Result<Instruction> {
                let numeric = raw_numeric.map(Fraction::decimal).transpose()?;
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
        // A reciprocal stop carries no second instruction.
        let Some(payload) = payload else {
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
            let assignment = match text(payload, "voiceAssignment") {
                None => None,
                Some("currentVoiceOnly") => Some(VoiceAssignment::CurrentVoice),
                Some("allInStaff") => Some(VoiceAssignment::StaffVoices),
                Some("allInInstrument") => Some(VoiceAssignment::InstrumentVoices),
                Some(value) => return Err(format!("Unknown voiceAssignment {value:?}")),
            };
            let legacy = match text(payload, "dynType") {
                None => None,
                Some("0" | "staff") => Some(LegacyRange::Staff),
                Some("1" | "part") => Some(LegacyRange::Part),
                Some("2" | "system") => Some(LegacyRange::System),
                Some(value) => return Err(format!("Unknown dynType {value:?}")),
            };
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
        let enabled = match bool_field(text(payload, "play")) {
            Ok(enabled) => enabled.unwrap_or(true),
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
                let speed = match text(payload, "veloChangeSpeed") {
                    None | Some("1" | "normal") => Speed::Normal,
                    Some("0" | "slow") => Speed::Slow,
                    Some("2" | "fast") => Speed::Fast,
                    Some(value) => return Err(format!("Unknown velocity speed {value:?}")),
                };
                event.instruction = Instruction::Dynamic(Dynamic {
                    symbol: text(payload, "subtype").map(str::to_owned),
                    numeric: decimal(payload, "velocity")?.map(NumericLevel::MuseScoreDynamic),
                    velo_change: decimal(payload, "veloChange")?,
                    speed,
                    tempo_at_start: Some(tempo),
                });
            } else if payload.has_tag_name("HairPin") || payload.has_tag_name("TextLine") {
                let subtype = text(payload, "subtype").unwrap_or("0");
                let labels: Vec<_> = ["beginText", "text", "endText"]
                    .iter()
                    .filter_map(|tag| child(payload, tag))
                    .map(|n| {
                        n.descendants()
                            .filter(|n| n.is_text())
                            .filter_map(|n| n.text())
                            .collect::<String>()
                    })
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
                        match subtype {
                            "0" | "2" | "crescendo" => Direction::Crescendo,
                            "1" | "3" | "decrescendo" => Direction::Diminuendo,
                            _ => return Err(format!("Unknown hairpin subtype {subtype:?}")),
                        }
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
                transition.velo_change = decimal(payload, "veloChange")?;
                transition.method = text(payload, "veloChangeMethod").map(str::to_owned);
                transition.single_note_dynamics = bool_field(text(payload, "singleNoteDynamics"))?;
                let niente = bool_field(text(payload, "nienteCircled"))?.unwrap_or(false);
                transition.niente_start = (niente && direction == Direction::Crescendo)
                    || intents.contains(&TextIntent::FadeIn);
                transition.niente_end = (niente && direction == Direction::Diminuendo)
                    || intents.contains(&TextIntent::FadeOut);
                transition.end_level = text(payload, "endText").and_then(standard_level);
                event.instruction = Instruction::Transition(transition);
                let location = child(node, "next").and_then(|n| child(n, "location"));
                let delta = location
                    .map(|loc| -> Result<_> {
                        let measures = text(loc, "measures")
                            .unwrap_or("0")
                            .parse::<i64>()
                            .map_err(|_| "Invalid spanner measure displacement")?;
                        for tag in ["staves", "voices"] {
                            if let Some(value) = text(loc, tag) {
                                value
                                    .parse::<i64>()
                                    .map_err(|_| format!("Invalid spanner {tag} displacement"))?;
                            }
                        }
                        // These displacements locate the endpoint's engraving
                        // anchor. Timing is given by measures/fractions; the
                        // playback scope remains the start instance's declared
                        // dynType/voiceAssignment, including across staves.
                        let fractions = text(loc, "fractions").unwrap_or("0/1");
                        let (n, d) = fractions
                            .split_once('/')
                            .ok_or("Invalid spanner fraction")?;
                        Ok((
                            measures,
                            Fraction::new(
                                n.parse().map_err(|_| "Invalid spanner numerator")?,
                                d.parse().map_err(|_| "Invalid spanner denominator")?,
                            )?
                            .checked_mul(Fraction::integer(4))?
                            .checked_mul(Fraction::new(stretch.0, stretch.1)?)?,
                        ))
                    })
                    .transpose()?;
                if let Some(legacy_id) = payload.attribute("id") {
                    event.evidence.raw_fields.insert(
                        "legacy_spanner".into(),
                        format!("{}:{}:{legacy_id}", owner.part, owner.staff),
                    );
                }
                if let Some(ticks) = text(payload, "ticks") {
                    event
                        .evidence
                        .raw_fields
                        .insert("span_ticks".into(), ticks.into());
                }
                span_delta = delta;
                is_span = true;
            } else {
                let words = child(payload, "text")
                    .map(|n| {
                        n.descendants()
                            .filter(|n| n.is_text())
                            .filter_map(|n| n.text())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
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
        let exact = (|| -> Result<Fraction> {
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
        })()
        .ok();
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
        for PendingSpanner {
            mut event,
            measure,
            start,
            delta,
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
        let run_count = self.runs.values().map(Vec::len).sum::<usize>();
        self.provenance_budget
            .charge(run_count.saturating_mul(4), run_count)?;
        let passes: Vec<_> = self.runs.values().flatten().map(|run| run.pass).collect();
        self.provenance_budget
            .charge(wedges.len().saturating_mul(64), wedges.len())?;
        let mut route_passes = Vec::new();
        for wedge in &wedges {
            let id = &wedge.event.evidence.source_ids[0];
            let mut eligible = BTreeSet::new();
            if self
                .original_declarations
                .get(id)
                .is_none_or(|d| d.written_measure.is_none())
            {
                // The score-only synthetic seam has no parser route contract.
                let count = passes
                    .len()
                    .saturating_add(wedge.event.time_only.len())
                    .saturating_add(1);
                self.provenance_budget
                    .charge(count.saturating_mul(64), count.saturating_mul(16))?;
                eligible.extend(
                    std::iter::once(1)
                        .chain(passes.iter().copied())
                        .chain(wedge.event.time_only.iter().copied()),
                );
            } else {
                self.provenance_budget.charge(0, self.runs.len())?;
                for ((part, staff), runs) in &self.runs {
                    let (scope_part, scope_staff, voice) = match &wedge.event.scope {
                        Scope::Part(part) => (part, None, "1"),
                        Scope::Staff { part, staff } => (part, Some(staff.as_str()), "1"),
                        Scope::Voice { part, staff, voice } => {
                            (part, staff.as_deref(), voice.as_str())
                        }
                        _ => continue,
                    };
                    if part != scope_part || scope_staff.is_some_and(|s| s != staff) {
                        continue;
                    }
                    self.provenance_budget.charge(
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
                    for run in runs {
                        self.provenance_budget
                            .charge(64, runs.len().saturating_mul(3).saturating_add(64))?;
                        if let Some(pass) =
                            self.declaration_route_pass(id, &owner, run.ordinal, run.pass)
                        {
                            eligible.insert(pass);
                        }
                    }
                }
            }
            route_passes.push(eligible);
        }
        let (events, issues) = pair_wedges_bounded(
            &wedges,
            &passes,
            Some(&route_passes),
            &mut self.provenance_budget,
        )?;
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

    fn owner() -> ScoreVoice {
        ScoreVoice {
            part: "P1".into(),
            staff: "1".into(),
            voice: "1".into(),
            instrument: None,
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
