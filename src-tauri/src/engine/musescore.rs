//! Native MuseScore parser (.mscz = ZIP containing a .mscx, or raw .mscx).
//! Produces the same intermediate `Midi` structure as the other parsers.
//! Covers MuseScore 3.x / 4.x: Division, Part/Instrument/longName,
//! Staff/Measure/voice, TimeSig, Tempo, Chord (dots, tuplets, graces),
//! Rest (including full measures), location, and all source lyric lanes.
use crate::engine::midi::{
    merge_measure_marks, unroll_with_passes, ChordReading, Event, InstrumentInfo, Jump, Kind,
    Lyric, LyricFragment, LyricState, MeasureMarks, Midi, MidiTextProfile, NoteOff, NoteOn,
    NoteSource, SourceContinuity, SourceContinuityIssue, SourceEvidenceRef, SourceExtension,
    SourceFormat, SourceNoteRef, SourcePart, SourceStaff, SourceTie, SourceTopology, SourceVoice,
    StaffLink, Syllabic, TimeBase, Track, TrackRoleHint, TrackSource,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::Arc;

pub fn is_musescore_xml(data: &[u8]) -> bool {
    crate::engine::musicxml::xml_bytes_contain_ascii(data, b"<museScore")
}

pub fn zip_has_mscx(data: &[u8]) -> bool {
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
        for i in 0..zip.len() {
            if let Ok(f) = zip.by_index(i) {
                if f.name().ends_with(".mscx") {
                    return true;
                }
            }
        }
    }
    false
}

pub fn parse(data: &[u8]) -> Result<Midi, String> {
    let xml = if data.len() >= 2 && &data[0..2] == b"PK" {
        extract_mscz(data)?
    } else {
        crate::engine::musicxml::decode_xml_bytes(data)?
    };
    parse_mscx(&xml)
}

/// The Parts a MuseScore container holds, in score order, each named by the
/// `id` MuseScore writes on it.
///
/// `--score-parts` answers with every part the score can be cut into *and*
/// every excerpt already saved in the file, so a score whose author saved a
/// two-instrument part comes back with more containers than the source has
/// Parts. Reading what a container actually holds is what tells a
/// one-Part excerpt apart from a saved multi-instrument one.
///
/// `None` for bytes that are not a readable score; a `None` entry for a Part
/// written without an id, as MuseScore 3 writes them.
pub fn container_part_ids(data: &[u8]) -> Option<Vec<Option<String>>> {
    let xml = if data.len() >= 2 && &data[0..2] == b"PK" {
        extract_mscz(data).ok()?
    } else {
        crate::engine::musicxml::decode_xml_bytes(data).ok()?
    };
    crate::engine::musicxml::check_nesting(&xml).ok()?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 5_000_000,
    };
    let document = roxmltree::Document::parse_with_options(&xml, options).ok()?;
    let score = document
        .descendants()
        .find(|node| node.has_tag_name("Score"))?;
    Some(
        score
            .children()
            .filter(|node| node.has_tag_name("Part"))
            .map(|part| part.attribute("id").map(str::to_string))
            .collect(),
    )
}

/// Removes only MuseScore-generated empty measures that can appear in an
/// extracted `--score-parts` excerpt but not in the source Part. The source
/// measure sequence is the authority; if the sequences cannot be aligned by
/// semantic measure content, the excerpt is left untouched and the bundle
/// timeline validator fails closed.
#[cfg(test)]
pub(crate) fn repair_extracted_part_timeline(
    source: &[u8],
    extracted: &[u8],
    source_part_index: usize,
) -> Result<Vec<u8>, String> {
    let source_xml = if source.starts_with(b"PK") {
        extract_mscz(source)?
    } else {
        crate::engine::musicxml::decode_xml_bytes(source)?
    };
    let extracted_xml = if extracted.starts_with(b"PK") {
        extract_mscz(extracted)?
    } else {
        crate::engine::musicxml::decode_xml_bytes(extracted)?
    };
    let source_document = roxmltree::Document::parse_with_options(
        &source_xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 5_000_000,
        },
    )
    .map_err(|error| format!("invalid source MuseScore XML: {error}"))?;
    let extracted_document = roxmltree::Document::parse_with_options(
        &extracted_xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 5_000_000,
        },
    )
    .map_err(|error| format!("invalid extracted MuseScore XML: {error}"))?;
    let source_score = source_document
        .descendants()
        .find(|node| node.has_tag_name("Score"))
        .ok_or_else(|| "source MuseScore XML has no Score".to_string())?;
    let extracted_score = extracted_document
        .descendants()
        .find(|node| node.has_tag_name("Score"))
        .ok_or_else(|| "extracted MuseScore XML has no Score".to_string())?;

    let source_parts = source_score
        .children()
        .filter(|node| {
            node.has_tag_name("Part")
                && node.children().any(|staff| {
                    staff.has_tag_name("Staff")
                        && !staff.children().any(|child| child.has_tag_name("linkedTo"))
                })
        })
        .collect::<Vec<_>>();
    let source_part = source_parts
        .get(source_part_index)
        .copied()
        .ok_or_else(|| format!("source Part index {source_part_index} is unavailable"))?;
    let source_staff_ids = source_part
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .filter_map(|node| node.attribute("id"))
        .collect::<Vec<_>>();
    if source_staff_ids.is_empty() {
        return Ok(extracted.to_vec());
    }

    let source_staffs = source_staff_ids
        .iter()
        .map(|staff_id| {
            source_score
                .children()
                .find(|node| node.has_tag_name("Staff") && node.attribute("id") == Some(*staff_id))
                .ok_or_else(|| format!("source Part staff {staff_id} has no score body"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let extracted_staffs = extracted_score
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .collect::<Vec<_>>();
    if extracted_staffs.len() != source_staffs.len() {
        return Ok(extracted.to_vec());
    }

    let mut common_removals: Option<Vec<usize>> = None;
    let mut length_replacements = Vec::new();
    for (source_staff, extracted_staff) in source_staffs.iter().zip(extracted_staffs) {
        let source_measures = source_staff
            .children()
            .filter(|node| node.has_tag_name("Measure"))
            .collect::<Vec<_>>();
        let extracted_measures = extracted_staff
            .children()
            .filter(|node| node.has_tag_name("Measure"))
            .collect::<Vec<_>>();
        let (extra, length_fixes) = align_measure_sequences(&source_measures, &extracted_measures)?;
        if let Some(existing) = &common_removals {
            if existing != &extra {
                return Err("extracted MuseScore staves disagree on extra measures".into());
            }
        } else {
            common_removals = Some(extra);
        }
        for (index, length) in length_fixes {
            length_replacements.push((extracted_measures[index].range(), length));
        }
    }
    let removals = common_removals.unwrap_or_default();
    if removals.is_empty() && length_replacements.is_empty() {
        return Ok(extracted.to_vec());
    }

    let ranges = extracted_document
        .descendants()
        .filter(|node| node.has_tag_name("Staff"))
        .flat_map(|staff| {
            staff
                .children()
                .filter(|node| node.has_tag_name("Measure"))
                .enumerate()
                .filter(|(index, _)| removals.contains(index))
                .map(|(_, node)| node.range())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut edits = ranges
        .into_iter()
        .map(|range| (range, String::new()))
        .collect::<Vec<_>>();
    let opening_replacements = length_replacements
        .into_iter()
        .filter_map(|(range, length)| measure_opening_replacement(&extracted_xml, range, length))
        .collect::<Vec<_>>();
    edits.extend(opening_replacements);
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut repaired_xml = extracted_xml;
    for (range, replacement) in edits {
        repaired_xml.replace_range(range, &replacement);
    }
    rewrite_master_mscx(extracted, &repaired_xml)
}

#[cfg(test)]
fn measure_signature(measure: roxmltree::Node<'_, '_>) -> Vec<String> {
    measure
        .descendants()
        .filter(|node| {
            if !node.is_element() {
                return false;
            }
            let tag = node.tag_name().name();
            !matches!(
                tag,
                "eid"
                    | "linkedTo"
                    | "linkedMain"
                    | "font"
                    | "family"
                    | "b"
                    | "i"
                    | "u"
                    | "size"
                    | "ticks"
                    | "ticks_f"
            ) && !node.ancestors().any(|ancestor| {
                matches!(ancestor.tag_name().name(), "Tempo" | "KeySig" | "BarLine")
            })
        })
        .map(|node| {
            let tag = match node.tag_name().name() {
                "concertKey" => "accidental",
                "text"
                    if !node
                        .ancestors()
                        .any(|ancestor| ancestor.has_tag_name("Lyrics")) =>
                {
                    "non-lyric-text"
                }
                other => other,
            };
            let text = node.text().unwrap_or("").trim();
            format!("{tag}:{text}")
        })
        .collect()
}

#[cfg(test)]
type MeasureAlignment = (Vec<usize>, Vec<(usize, Option<String>)>);

#[cfg(test)]
fn align_measure_sequences(
    source: &[roxmltree::Node<'_, '_>],
    extracted: &[roxmltree::Node<'_, '_>],
) -> Result<MeasureAlignment, String> {
    let mut source_index = 0;
    let mut extracted_index = 0;
    let mut extra = Vec::new();
    let mut length_fixes = Vec::new();
    while source_index < source.len() && extracted_index < extracted.len() {
        if measure_signature(source[source_index]) == measure_signature(extracted[extracted_index])
        {
            let source_length = source[source_index].attribute("len").map(str::to_string);
            if source_length
                != extracted[extracted_index]
                    .attribute("len")
                    .map(str::to_string)
            {
                length_fixes.push((extracted_index, source_length));
            }
            source_index += 1;
            extracted_index += 1;
        } else if extracted_index + 1 < extracted.len()
            && measure_signature(source[source_index])
                == measure_signature(extracted[extracted_index + 1])
        {
            extra.push(extracted_index);
            extracted_index += 1;
        } else {
            return Err(format!(
                "extracted MuseScore measures diverge at source {} / excerpt {}",
                source_index + 1,
                extracted_index + 1
            ));
        }
    }
    if source_index != source.len() || extracted_index != extracted.len() {
        return Err("extracted MuseScore measure count cannot be aligned".into());
    }
    Ok((extra, length_fixes))
}

#[cfg(test)]
fn measure_opening_replacement(
    xml: &str,
    range: std::ops::Range<usize>,
    length: Option<String>,
) -> Option<(std::ops::Range<usize>, String)> {
    let opening_end = xml[range.start..range.end].find('>')? + range.start + 1;
    let opening_range = range.start..opening_end;
    let opening = &xml[opening_range.clone()];
    let mut replacement = opening.to_string();
    if let Some(attribute_start) = replacement.find(" len=\"") {
        let value_start = attribute_start + 6;
        let value_end = replacement[value_start..].find('"')? + value_start + 1;
        replacement.replace_range(attribute_start..value_end, "");
    }
    if let Some(length) = length {
        let insert_at = replacement.rfind('>')?;
        replacement.insert_str(insert_at, &format!(" len=\"{length}\""));
    }
    (replacement != opening).then_some((opening_range, replacement))
}

#[cfg(test)]
fn rewrite_master_mscx(data: &[u8], replacement: &str) -> Result<Vec<u8>, String> {
    let master = master_mscx_path(data)?;
    let mut input = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut output = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut entry = input.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            output
                .add_directory(name, zip::write::SimpleFileOptions::default())
                .map_err(|e| e.to_string())?;
        } else {
            output
                .start_file(name.clone(), zip::write::SimpleFileOptions::default())
                .map_err(|e| e.to_string())?;
            if name == master {
                std::io::Write::write_all(&mut output, replacement.as_bytes())
                    .map_err(|e| e.to_string())?;
            } else {
                std::io::copy(&mut entry, &mut output).map_err(|e| e.to_string())?;
            }
        }
    }
    output
        .finish()
        .map_err(|e| e.to_string())
        .map(|cursor| cursor.into_inner())
}

fn is_mscx_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mscx"))
}

fn validate_rootfile_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("MuseScore rootfile has an empty full-path".to_string());
    }
    // ZIP member names always use `/`. Rejecting `\` also prevents a
    // Windows-style absolute or traversal path from appearing relative when
    // this code runs on Unix.
    if path.contains('\\') {
        return Err(format!(
            "MuseScore rootfile has an unsafe full-path: {path:?}"
        ));
    }
    let bytes = path.as_bytes();
    let has_windows_drive_prefix =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';
    let has_unsafe_segment = path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..");
    let path_object = Path::new(path);
    if has_windows_drive_prefix
        || has_unsafe_segment
        || path_object.is_absolute()
        || path_object
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "MuseScore rootfile has an unsafe full-path: {path:?}"
        ));
    }
    if !is_mscx_path(path) {
        return Err(format!(
            "MuseScore rootfile is not an .mscx score: {path:?}"
        ));
    }
    Ok(())
}

fn is_top_level_path(path: &str) -> bool {
    Path::new(path).components().count() == 1
}

fn is_excerpt_path(path: &str) -> bool {
    Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::Normal(segment)
                if segment
                    .to_str()
                    .is_some_and(|segment| segment.eq_ignore_ascii_case("Excerpts"))
        )
    })
}

fn select_declared_master(container: &str) -> Result<String, String> {
    let document = roxmltree::Document::parse_with_options(
        container,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|error| format!("invalid MuseScore container: {error}"))?;

    let mut roots = Vec::new();
    for rootfile in document
        .descendants()
        .filter(|node| node.has_tag_name("rootfile"))
    {
        let path = rootfile
            .attribute("full-path")
            .ok_or_else(|| "MuseScore rootfile has no full-path".to_string())?;
        if is_mscx_path(path) {
            validate_rootfile_path(path)?;
            roots.push(path.to_string());
        }
    }
    if roots.is_empty() {
        return Err("MuseScore container has no .mscx rootfile".to_string());
    }

    let top_level: Vec<_> = roots
        .iter()
        .filter(|path| is_top_level_path(path) && !is_excerpt_path(path))
        .collect();
    if top_level.len() == 1 {
        return Ok(top_level[0].clone());
    }
    if top_level.len() > 1 {
        return Err("MuseScore container has ambiguous master .mscx rootfiles".to_string());
    }

    let non_excerpt: Vec<_> = roots.iter().filter(|path| !is_excerpt_path(path)).collect();
    if non_excerpt.len() == 1 {
        return Ok(non_excerpt[0].clone());
    }
    if non_excerpt.is_empty() {
        return Err(
            "MuseScore container declares only Excerpts and no master .mscx rootfile".to_string(),
        );
    }
    Err("MuseScore container has ambiguous master .mscx rootfiles".to_string())
}

fn master_mscx_path(data: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;

    let mut container_indices = Vec::new();
    let mut mscx_entries = Vec::new();
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name().to_string();
        if name == "META-INF/container.xml" {
            container_indices.push(index);
        }
        if !file.is_dir() && is_mscx_path(&name) {
            if file.enclosed_name().is_none() {
                return Err(format!(
                    "MuseScore archive has an unsafe .mscx member path: {name:?}"
                ));
            }
            validate_rootfile_path(&name)?;
            mscx_entries.push((index, name));
        }
    }

    let selected = match container_indices.as_slice() {
        [container_index] => {
            let mut container_file = zip
                .by_index(*container_index)
                .map_err(|error| error.to_string())?;
            let container = crate::engine::musicxml::read_zip_entry_capped(&mut container_file)?;
            drop(container_file);
            select_declared_master(&container)?
        }
        [] => {
            let top_level: Vec<_> = mscx_entries
                .iter()
                .filter(|(_, path)| is_top_level_path(path))
                .collect();
            if top_level.len() == 1 {
                top_level[0].1.clone()
            } else if top_level.is_empty() && mscx_entries.len() == 1 {
                mscx_entries[0].1.clone()
            } else if mscx_entries.is_empty() {
                return Err("no .mscx in MuseScore archive".to_string());
            } else {
                return Err(
                    "MuseScore archive has ambiguous .mscx roots and no container".to_string(),
                );
            }
        }
        _ => {
            return Err(
                "MuseScore archive has ambiguous META-INF/container.xml entries".to_string(),
            )
        }
    };

    let matching_entries: Vec<_> = mscx_entries
        .iter()
        .filter(|(_, path)| path == &selected)
        .collect();
    match matching_entries.as_slice() {
        [(_, path)] => Ok(path.clone()),
        [] => Err(format!(
            "MuseScore container-declared rootfile is missing: {selected:?}"
        )),
        _ => Err(format!(
            "MuseScore container-declared rootfile is ambiguous: {selected:?}"
        )),
    }
}

fn extract_mscz(data: &[u8]) -> Result<String, String> {
    let selected = master_mscx_path(data)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let entry_index = (0..zip.len())
        .find(|index| {
            zip.by_index(*index)
                .ok()
                .is_some_and(|file| file.name() == selected)
        })
        .ok_or_else(|| format!("MuseScore container-declared rootfile is missing: {selected:?}"))?;

    let mut score_file = zip
        .by_index(entry_index)
        .map_err(|error| error.to_string())?;
    crate::engine::musicxml::read_zip_entry_capped(&mut score_file)
}

fn frac(s: &str) -> Option<(i64, i64)> {
    let mut it = s.trim().split('/');
    let a = it.next()?.trim().parse::<i64>().ok()?;
    let b = it.next()?.trim().parse::<i64>().ok()?;
    if it.next().is_some() || b <= 0 || a.unsigned_abs() > 1_000_000 || b > 1_000_000 {
        None
    } else {
        Some((a, b))
    }
}

fn child<'a, 'b>(n: roxmltree::Node<'a, 'b>, tag: &str) -> Option<roxmltree::Node<'a, 'b>> {
    n.children().find(|c| c.has_tag_name(tag))
}

/// One side (`<next>` or `<prev>`) of a MuseScore tie spanner. MuseScore stores
/// the distance to the other end of the tie, and that distance is the evidence
/// that proves a candidate pairing is the real one. Pairing on pitch and voice
/// alone is a guess: measured over the pinned corpus it mis-pairs 49 ties, one
/// of them fusing 58 measures that the score never sustains.
#[derive(Clone, Copy, Default)]
struct TieSide {
    measure_delta: i64,
    voice_delta: i64,
    staff_delta: i64,
    grace_target: bool,
    fraction: Option<(i64, i64)>,
    unsupported: bool,
}

/// Tie evidence carried by one `<Note>`. MuseScore 3.x nests a
/// `<Spanner type="Tie">` per side; MuseScore 2.x pairs `<Tie id>` with
/// `<endSpanner id>`. A note in the middle of a chain carries both sides.
#[derive(Default)]
struct NoteTies {
    start: Option<TieSide>,
    stop: Option<TieSide>,
    legacy_start: Option<String>,
    legacy_stop: Option<String>,
}

/// A tie chain whose head note-off is already queued and can still be extended.
/// `measure_index` tracks the latest link, because the next link's back
/// reference points at that link and not at the head of the chain.
struct PendingTie {
    bucket: (usize, Option<usize>),
    off_index: usize,
    end_tick: u32,
    measure_index: usize,
    /// Independent from the historical bare-tail merge state. Ambiguous source
    /// starts retain that behavior but cannot authorize new sung recovery.
    continuity: Option<PendingContinuityTie>,
}

struct PendingContinuityTie {
    source: SourceNoteRef,
    evidence: SourceEvidenceRef,
    pitch: u8,
    voice: usize,
    measure: usize,
    measure_tick: i64,
    end_tick: u32,
    next: Option<TieSide>,
}

/// Unresolved declarations are tracked independently of bare-tail merging:
/// a text-bearing tail resolves its source link without consuming the old merge
/// state. Store event addresses, not another copy of the source XML.
struct OutgoingTie {
    bucket: (usize, Option<usize>),
    on_index: usize,
    source: SourceNoteRef,
    start_tick: u32,
    end_tick: u32,
}

const MAX_UNRESOLVED_TIE_STARTS: usize = 4096;

fn abandon_tie_start(
    outgoing: OutgoingTie,
    voice_events: &mut BTreeMap<(usize, Option<usize>), Vec<Event>>,
    reason: &str,
    diagnostic_count: &mut usize,
) -> Result<(), String> {
    if *diagnostic_count >= MAX_UNRESOLVED_TIE_STARTS {
        return Err(format!(
            "SOURCE_CONTINUITY_LIMIT: MuseScore unresolved tie diagnostics exceed \
             {MAX_UNRESOLVED_TIE_STARTS}"
        ));
    }
    let event = voice_events
        .get_mut(&outgoing.bucket)
        .and_then(|events| events.get_mut(outgoing.on_index))
        .ok_or_else(|| "MuseScore outgoing tie source event is missing".to_string())?;
    let Kind::NoteOn(note) = &mut event.kind else {
        return Err("MuseScore outgoing tie source is not a note-on".into());
    };
    let continuity = note
        .source
        .continuity
        .as_mut()
        .ok_or_else(|| "MuseScore outgoing tie source evidence is missing".to_string())?;
    let continuity = Arc::make_mut(continuity);
    continuity.issues.push(SourceContinuityIssue {
        code: "SOURCE_CONTINUITY_LINK_INVALID",
        message: format!(
            "Outgoing tie from {}, occurrence {}, segment {}, interval {}..{}: {reason}; \
             no source endpoint was validated",
            outgoing.source.source_id,
            outgoing.source.occurrence,
            outgoing.source.playback_segment,
            outgoing.start_tick,
            outgoing.end_tick
        ),
        evidence: continuity.evidence.clone(),
    });
    *diagnostic_count += 1;
    Ok(())
}

fn abandon_tie_starts(
    outgoing: &mut BTreeMap<TieKey, OutgoingTie>,
    voice_events: &mut BTreeMap<(usize, Option<usize>), Vec<Event>>,
    reason: &str,
    diagnostic_count: &mut usize,
) -> Result<(), String> {
    for start in std::mem::take(outgoing).into_values() {
        abandon_tie_start(start, voice_events, reason, diagnostic_count)?;
    }
    Ok(())
}

/// One immutable payload per original XML element. Playback occurrences and
/// later source/projection clones share these bytes instead of multiplying them.
/// The cumulative cap also covers newly retained continuity metadata; it is
/// checked before allocating a new payload or cloning per-note vectors.
struct ContinuityEvidencePool {
    xml: BTreeMap<usize, Arc<str>>,
    source_version: Option<Arc<str>>,
    program_version: Option<Arc<str>>,
    bytes: usize,
    limit: usize,
}
const MAX_CONTINUITY_EVIDENCE_BYTES: usize = 128 * 1024 * 1024;
impl ContinuityEvidencePool {
    fn new(root: roxmltree::Node) -> Result<Self, String> {
        let mut pool = Self {
            xml: BTreeMap::new(),
            source_version: None,
            program_version: None,
            bytes: 0,
            limit: MAX_CONTINUITY_EVIDENCE_BYTES,
        };
        let version = root.attribute("version");
        let program = child_text(root, "programVersion");
        pool.charge(
            version
                .map_or(0, str::len)
                .saturating_add(program.map_or(0, str::len)),
        )?;
        pool.source_version = version.map(Arc::from);
        pool.program_version = program.map(Arc::from);
        Ok(pool)
    }
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("SOURCE_CONTINUITY_LIMIT: source evidence exceeds cumulative memory bound")?;
        Ok(())
    }
    fn evidence(
        &mut self,
        node: roxmltree::Node,
        source_id: &str,
    ) -> Result<SourceEvidenceRef, String> {
        self.charge(std::mem::size_of::<SourceEvidenceRef>().saturating_add(source_id.len()))?;
        let key = node.range().start;
        if !self.xml.contains_key(&key) {
            let raw = &node.document().input_text()[node.range()];
            self.charge(raw.len().saturating_add(64))?;
            self.xml.insert(key, Arc::from(raw));
        }
        Ok(SourceEvidenceRef {
            source_format: SourceFormat::MuseScore,
            source_version: self.source_version.clone(),
            program_version: self.program_version.clone(),
            source_id: source_id.to_owned(),
            raw_xml: Arc::clone(&self.xml[&key]),
        })
    }
    fn extension_copies(
        &mut self,
        extensions: &[SourceExtension],
        issues: &[SourceContinuityIssue],
    ) -> Result<(), String> {
        for extension in extensions {
            self.charge(
                std::mem::size_of::<SourceExtension>()
                    .saturating_add(extension.lyric_id.len())
                    .saturating_add(extension.lane.len())
                    .saturating_add(extension.chord_id.len())
                    .saturating_add(extension.evidence.source_id.len()),
            )?;
        }
        for issue in issues {
            self.charge(
                std::mem::size_of::<SourceContinuityIssue>()
                    .saturating_add(issue.message.len())
                    .saturating_add(issue.evidence.source_id.len()),
            )?;
        }
        Ok(())
    }
}

/// Both positional declarations must agree with the same exact contact.
/// Fractions are offsets between positions within their notated measures.
fn tie_side_matches(
    side: TieSide,
    from: (usize, usize, i64),
    to: (usize, usize, i64),
    division: i64,
) -> bool {
    if side.staff_delta != 0 || side.grace_target || side.unsupported {
        return false;
    }
    let Ok(from_measure) = i64::try_from(from.0) else {
        return false;
    };
    let Ok(to_measure) = i64::try_from(to.0) else {
        return false;
    };
    let Ok(from_voice) = i64::try_from(from.1) else {
        return false;
    };
    let Ok(to_voice) = i64::try_from(to.1) else {
        return false;
    };
    let fraction = side.fraction.unwrap_or((0, 1));
    let delta = fraction.0.checked_mul(4).and_then(|numerator| {
        exact_ticks(division, (numerator, fraction.1), "MuseScore tie location").ok()
    });
    from_measure.checked_add(side.measure_delta) == Some(to_measure)
        && from_voice.checked_add(side.voice_delta) == Some(to_voice)
        && delta.and_then(|delta| from.2.checked_add(delta)) == Some(to.2)
}

fn validated_incoming_tie(
    pending: &PendingTie,
    ties: &NoteTies,
    tail: &PendingContinuityTie,
    on: u32,
    division: i64,
) -> Option<SourceTie> {
    let head = pending.continuity.as_ref()?;
    if head.end_tick != on
        || head.pitch != tail.pitch
        || head.source.playback_segment != tail.source.playback_segment
    {
        return None;
    }
    let head_position = (head.measure, head.voice, head.measure_tick);
    let tail_position = (tail.measure, tail.voice, tail.measure_tick);
    // Legacy IDs are already matched by TieKey, but do not waive pitch/contact.
    if ties.legacy_stop.is_none()
        && (!ties
            .stop
            .is_some_and(|side| tie_side_matches(side, tail_position, head_position, division))
            || !head
                .next
                .is_some_and(|side| tie_side_matches(side, head_position, tail_position, division)))
    {
        return None;
    }
    Some(SourceTie {
        head: head.source.clone(),
        tail: tail.source.clone(),
        contact_tick: on,
        pitch: tail.pitch,
        evidence: vec![head.evidence.clone(), tail.evidence.clone()],
    })
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TieKey {
    Positional(usize, u8),
    Legacy(String),
}

fn tie_location(side: roxmltree::Node) -> Result<TieSide, String> {
    let Some(location) = child(side, "location") else {
        return Ok(TieSide::default());
    };
    let signed = |tag: &str| -> Result<i64, String> {
        match child_text(location, tag) {
            None => Ok(0),
            Some(text) => text
                .parse::<i64>()
                .ok()
                .filter(|value| value.unsigned_abs() <= 1024)
                .ok_or_else(|| format!("MuseScore tie location {tag} is invalid: {text:?}")),
        }
    };
    Ok(TieSide {
        measure_delta: signed("measures")?,
        voice_delta: signed("voices")?,
        staff_delta: signed("staves")?,
        grace_target: child(location, "grace").is_some(),
        fraction: child_text(location, "fractions")
            .map(|text| {
                frac(text).ok_or_else(|| format!("MuseScore tie fraction is invalid: {text:?}"))
            })
            .transpose()?,
        unsupported: side
            .children()
            .filter(|node| node.has_tag_name("location"))
            .count()
            != 1
            || ["measures", "voices", "staves", "fractions", "grace"]
                .iter()
                .any(|tag| {
                    location
                        .children()
                        .filter(|node| node.has_tag_name(*tag))
                        .count()
                        > 1
                })
            || location
                .children()
                .filter(|node| node.is_element())
                .any(|node| {
                    !matches!(
                        node.tag_name().name(),
                        "measures" | "voices" | "staves" | "fractions" | "grace"
                    )
                }),
    })
}

/// True when at least one lyric actually carries a word. An `ExplicitEmpty`
/// lyric does not: MuseScore writes one on a tied note to state that nothing is
/// sung there, which is evidence for merging rather than against it.
fn sings_a_word(lyrics: &[Lyric]) -> bool {
    lyrics
        .iter()
        .any(|lyric| matches!(&lyric.state, LyricState::Text(text) if !text.trim().is_empty()))
}

fn tie_stop_key(ties: &NoteTies, voice_index: usize, pitch: u8) -> Option<TieKey> {
    if let Some(id) = &ties.legacy_stop {
        return Some(TieKey::Legacy(id.clone()));
    }
    let stop = ties.stop?;
    // A cross-staff tie, or one ending on a grace note, cannot be resolved from
    // this staff's event stream. Leaving it unmerged keeps both notes audible,
    // which is the safe direction: nothing is invented and nothing goes silent.
    if stop.staff_delta != 0 || stop.grace_target {
        return None;
    }
    let head_voice = i64::try_from(voice_index).ok()? + stop.voice_delta;
    usize::try_from(head_voice)
        .ok()
        .map(|voice| TieKey::Positional(voice, pitch))
}

fn tie_start_key(ties: &NoteTies, voice_index: usize, pitch: u8) -> Option<TieKey> {
    if let Some(id) = &ties.legacy_start {
        return Some(TieKey::Legacy(id.clone()));
    }
    ties.start.map(|_| TieKey::Positional(voice_index, pitch))
}

fn note_ties(note: roxmltree::Node) -> Result<NoteTies, String> {
    let mut ties = NoteTies::default();
    for element in note.children().filter(|c| c.is_element()) {
        match element.tag_name().name() {
            // `<Note>` also carries TextLine and Glissando spanners in real
            // scores, so the type filter is required rather than cosmetic.
            "Spanner" if element.attribute("type") == Some("Tie") => {
                if child(element, "next").is_none() && child(element, "prev").is_none() {
                    return Err("MuseScore tie spanner has neither source endpoint".into());
                }
                if ["next", "prev"].iter().any(|tag| {
                    element
                        .children()
                        .filter(|node| node.has_tag_name(*tag))
                        .count()
                        > 1
                }) {
                    return Err("MuseScore tie spanner declares duplicate endpoints".into());
                }
                if let Some(next) = child(element, "next") {
                    if ties.start.is_some() {
                        return Err("MuseScore Note declares two tie starts".into());
                    }
                    ties.start = Some(tie_location(next)?);
                }
                if let Some(previous) = child(element, "prev") {
                    if ties.stop.is_some() {
                        return Err("MuseScore Note declares two tie stops".into());
                    }
                    ties.stop = Some(tie_location(previous)?);
                }
            }
            // MuseScore 2.x encoding; `<Tie>` is never a direct child of
            // `<Note>` in 3.x, where it sits inside the `<Spanner>`.
            "Tie" | "endSpanner" => {
                let destination = if element.has_tag_name("Tie") {
                    &mut ties.legacy_start
                } else {
                    &mut ties.legacy_stop
                };
                if destination.is_some() {
                    return Err("MuseScore Note declares duplicate legacy tie endpoints".into());
                }
                *destination = Some(
                    element
                        .attribute("id")
                        .filter(|id| !id.trim().is_empty())
                        .ok_or_else(|| "MuseScore legacy tie endpoint has no id".to_string())?
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    if (ties.start.is_some() && ties.legacy_start.is_some())
        || (ties.stop.is_some() && ties.legacy_stop.is_some())
    {
        return Err("MuseScore Note mixes positional and legacy tie endpoints".into());
    }
    Ok(ties)
}

fn child_text<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(n, tag).and_then(|c| c.text()).map(|t| t.trim())
}

/// Raw concatenation of every descendant text node, skipping `<sym>` elements
/// (their content is a SMuFL glyph name like "space", not lyric text) and
/// turning `<br/>` line breaks into spaces so adjacent words never fuse.
/// Rich text (`<text>`, names) may embed formatting elements (`<font size=..>`,
/// `<b>`, `<i>`, `<u>`, `<sup>`, `<sub>`, ...) around or between the words, so
/// a plain first-child `.text()` misses the content.
pub(crate) fn deep_text_raw(n: roxmltree::Node, out: &mut String) {
    for c in n.children() {
        if c.is_text() {
            out.push_str(c.text().unwrap_or(""));
        } else if c.has_tag_name("br") {
            out.push(' ');
        } else if !c.has_tag_name("sym") {
            deep_text_raw(c, out);
        }
    }
}

/// `deep_text_raw`, end-trimmed. Control characters and stray punctuation
/// inside the text are cleaned downstream (clean_syllable for lyrics,
/// collapse_ws for names).
pub(crate) fn deep_text(n: roxmltree::Node) -> String {
    let mut raw = String::new();
    deep_text_raw(n, &mut raw);
    raw.trim().to_string()
}

/// Collapses every whitespace run (spaces, tabs, newlines) into one space.
/// Used for display names, where a two-line MuseScore label must become a
/// single readable line.
pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Duration as a rational number of quarter notes.
fn duration_ratio(kind: &str) -> Option<(i64, i64)> {
    Some(match kind {
        "long" => (16, 1),
        "breve" => (8, 1),
        "whole" => (4, 1),
        "half" => (2, 1),
        "quarter" => (1, 1),
        "eighth" => (1, 2),
        "16th" => (1, 4),
        "32nd" => (1, 8),
        "64th" => (1, 16),
        "128th" => (1, 32),
        "256th" => (1, 64),
        _ => return None, // "measure" handled separately
    })
}

fn checked_ratio_mul(
    ratio: (i64, i64),
    numerator: i64,
    denominator: i64,
    context: &str,
) -> Result<(i64, i64), String> {
    if denominator <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let numerator = i128::from(ratio.0)
        .checked_mul(i128::from(numerator))
        .ok_or_else(|| format!("{context} numerator overflow"))?;
    let denominator = i128::from(ratio.1)
        .checked_mul(i128::from(denominator))
        .ok_or_else(|| format!("{context} denominator overflow"))?;
    let numerator =
        i64::try_from(numerator).map_err(|_| format!("{context} numerator overflow"))?;
    let denominator =
        i64::try_from(denominator).map_err(|_| format!("{context} denominator overflow"))?;
    let divisor = gcd_i64(numerator.unsigned_abs(), denominator as u64);
    Ok((
        numerator / i64::try_from(divisor).unwrap_or(1),
        denominator / i64::try_from(divisor).unwrap_or(1),
    ))
}

fn dotted_ratio(ratio: (i64, i64), dots: u32) -> Result<(i64, i64), String> {
    let denominator = 1i64
        .checked_shl(dots)
        .ok_or_else(|| "MuseScore dot denominator overflow".to_string())?;
    let numerator = denominator
        .checked_mul(2)
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| "MuseScore dot numerator overflow".to_string())?;
    checked_ratio_mul(ratio, numerator, denominator, "MuseScore dotted duration")
}

fn gcd_i64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn include_tick_ratio(
    scale: &mut i64,
    source_division: i64,
    ratio: (i64, i64),
    context: &str,
) -> Result<(), String> {
    if ratio.1 <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let base_numerator = i128::from(source_division)
        .checked_mul(i128::from(ratio.0))
        .ok_or_else(|| format!("{context} timing overflow"))?;
    let base_abs =
        u64::try_from(base_numerator.abs()).map_err(|_| format!("{context} timing overflow"))?;
    let denominator =
        u64::try_from(ratio.1).map_err(|_| format!("{context} denominator overflow"))?;
    let required = denominator / gcd_i64(base_abs, denominator);
    let current = u64::try_from(*scale).map_err(|_| "MuseScore tick scale overflow")?;
    let next = current
        .checked_div(gcd_i64(current, required))
        .and_then(|value| value.checked_mul(required))
        .ok_or_else(|| "MuseScore exact tick scale overflow".to_string())?;
    let next = i64::try_from(next).map_err(|_| "MuseScore exact tick scale overflow")?;
    let ticks_per_beat = source_division
        .checked_mul(next)
        .filter(|value| *value <= i64::from(u16::MAX))
        .ok_or_else(|| {
            format!("{context} requires a tick division beyond the supported exact range")
        })?;
    *scale = next;
    debug_assert!(ticks_per_beat > 0);
    Ok(())
}

fn exact_ticks(division: i64, ratio: (i64, i64), context: &str) -> Result<i64, String> {
    if ratio.1 <= 0 {
        return Err(format!("{context} has a non-positive denominator"));
    }
    let numerator = i128::from(division)
        .checked_mul(i128::from(ratio.0))
        .ok_or_else(|| format!("{context} timing overflow"))?;
    let denominator = i128::from(ratio.1);
    if numerator % denominator != 0 {
        return Err(format!(
            "{context} cannot be represented exactly at division {division}"
        ));
    }
    i64::try_from(numerator / denominator).map_err(|_| format!("{context} timing overflow"))
}

fn measure_voice_containers<'a, 'input>(
    measure: roxmltree::Node<'a, 'input>,
) -> Result<Vec<roxmltree::Node<'a, 'input>>, String> {
    let explicit_voices = measure
        .children()
        .filter(|node| node.has_tag_name("voice"))
        .collect::<Vec<_>>();
    let has_direct_sequence = measure
        .children()
        .filter(|node| node.is_element())
        .any(|node| {
            matches!(
                node.tag_name().name(),
                "TimeSig" | "Tempo" | "Tuplet" | "endTuplet" | "location" | "Chord" | "Rest"
            )
        });
    if !explicit_voices.is_empty() && has_direct_sequence {
        return Err(
            "MuseScore measure mixes direct legacy events with explicit voice containers".into(),
        );
    }
    // MuseScore 2.x stored the musical sequence directly under <Measure>;
    // MuseScore 3/4 wrap the same sequence in one or more <voice> elements.
    if explicit_voices.is_empty() {
        Ok(vec![measure])
    } else {
        Ok(explicit_voices)
    }
}

#[derive(Clone, Copy, Debug)]
struct MuseScoreTimeSignature {
    numerator: i64,
    denominator: i64,
    /// Actual-time multiplier for locally stretched notation.
    /// MuseScore stores this as `stretchD / stretchN`.
    stretch: (i64, i64),
    /// Meter duration after applying the local stretch, reduced.
    effective: (i64, i64),
}

fn positive_time_signature_value(
    node: roxmltree::Node,
    tag: &str,
    default: Option<i64>,
) -> Result<i64, String> {
    match child_text(node, tag) {
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|value| (1..=1_000_000).contains(value))
            .ok_or_else(|| format!("MuseScore TimeSig has an invalid {tag}: {value:?}")),
        None => default.ok_or_else(|| format!("MuseScore TimeSig is missing {tag}")),
    }
}

fn musescore_time_signature(node: roxmltree::Node) -> Result<MuseScoreTimeSignature, String> {
    let numerator = positive_time_signature_value(node, "sigN", None)?;
    let denominator = positive_time_signature_value(node, "sigD", None)?;
    let stretch_n = positive_time_signature_value(node, "stretchN", Some(1))?;
    let stretch_d = positive_time_signature_value(node, "stretchD", Some(1))?;
    let stretch = (stretch_d, stretch_n);
    let effective = checked_ratio_mul(
        (numerator, denominator),
        stretch.0,
        stretch.1,
        "MuseScore local time-signature stretch",
    )?;
    Ok(MuseScoreTimeSignature {
        numerator,
        denominator,
        stretch,
        effective,
    })
}

fn checked_meter_values(ratio: (i64, i64), context: &str) -> Result<(u8, u16), String> {
    if ratio.0 <= 0 || ratio.1 <= 0 {
        return Err(format!("{context} is non-positive"));
    }
    let numerator =
        u8::try_from(ratio.0).map_err(|_| format!("{context} numerator exceeds 255"))?;
    let denominator =
        u16::try_from(ratio.1).map_err(|_| format!("{context} denominator exceeds 65535"))?;
    Ok((numerator, denominator))
}

fn musescore_tick_scale(score: roxmltree::Node, source_division: i64) -> Result<i64, String> {
    let mut scale = 1i64;

    for measure in score
        .descendants()
        .filter(|node| node.has_tag_name("Measure"))
    {
        if let Some(value) = measure.attribute("len") {
            let (numerator, denominator) = frac(value)
                .ok_or_else(|| format!("MuseScore measure len fraction is invalid: {value:?}"))?;
            include_tick_ratio(
                &mut scale,
                source_division,
                (
                    4i64.checked_mul(numerator)
                        .ok_or_else(|| "MuseScore measure len numerator overflow".to_string())?,
                    denominator,
                ),
                "MuseScore measure len",
            )?;
        }
    }

    for staff in score.children().filter(|node| node.has_tag_name("Staff")) {
        let mut time_stretch = (1i64, 1i64);
        for measure in staff.children().filter(|node| node.has_tag_name("Measure")) {
            for voice in measure_voice_containers(measure)? {
                let mut tuplet: Option<(i64, i64)> = None;
                for element in voice.children().filter(|node| node.is_element()) {
                    match element.tag_name().name() {
                        "TimeSig" => {
                            let signature = musescore_time_signature(element)?;
                            time_stretch = signature.stretch;
                            let quarter_ratio = checked_ratio_mul(
                                signature.effective,
                                4,
                                1,
                                "MuseScore time-signature duration",
                            )?;
                            include_tick_ratio(
                                &mut scale,
                                source_division,
                                quarter_ratio,
                                "MuseScore time signature",
                            )?;
                        }
                        "Tuplet" => {
                            let normal = child_text(element, "normalNotes")
                                .and_then(|value| value.parse::<i64>().ok())
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    "MuseScore Tuplet has invalid normalNotes".to_string()
                                })?;
                            let actual = child_text(element, "actualNotes")
                                .and_then(|value| value.parse::<i64>().ok())
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    "MuseScore Tuplet has invalid actualNotes".to_string()
                                })?;
                            tuplet = Some((normal, actual));
                        }
                        "endTuplet" => tuplet = None,
                        "location" => {
                            let text = child_text(element, "fractions").ok_or_else(|| {
                                "MuseScore location is missing fractions".to_string()
                            })?;
                            let (numerator, denominator) = frac(text).ok_or_else(|| {
                                format!("MuseScore location fraction is invalid: {text:?}")
                            })?;
                            let base = (
                                4i64.checked_mul(numerator).ok_or_else(|| {
                                    "MuseScore location numerator overflow".to_string()
                                })?,
                                denominator,
                            );
                            let ratio = checked_ratio_mul(
                                base,
                                time_stretch.0,
                                time_stretch.1,
                                "MuseScore stretched location",
                            )?;
                            include_tick_ratio(
                                &mut scale,
                                source_division,
                                ratio,
                                "MuseScore location",
                            )?;
                        }
                        "Chord" | "Rest" if !is_grace(element) => {
                            let duration_type =
                                child_text(element, "durationType").ok_or_else(|| {
                                    format!(
                                        "MuseScore {} is missing durationType",
                                        element.tag_name().name()
                                    )
                                })?;
                            let ratio = if duration_type == "measure" {
                                match child_text(element, "duration") {
                                    Some(value) => {
                                        let (numerator, denominator) =
                                            frac(value).ok_or_else(|| {
                                                format!(
                                                    "MuseScore measure duration is invalid: {value:?}"
                                                )
                                            })?;
                                        Some((
                                            4i64.checked_mul(numerator).ok_or_else(|| {
                                                "MuseScore measure duration numerator overflow"
                                                    .to_string()
                                            })?,
                                            denominator,
                                        ))
                                    }
                                    None => None,
                                }
                            } else {
                                let dots = child_text(element, "dots")
                                    .map(|value| value.parse::<u32>())
                                    .transpose()
                                    .map_err(|_| "MuseScore dots value is invalid".to_string())?
                                    .unwrap_or(0);
                                if dots > 4 {
                                    return Err("MuseScore dots value is invalid".into());
                                }
                                Some(dotted_ratio(
                                    duration_ratio(duration_type).ok_or_else(|| {
                                        format!(
                                            "MuseScore durationType is unsupported: \
                                             {duration_type:?}"
                                        )
                                    })?,
                                    dots,
                                )?)
                            };
                            if let Some(mut ratio) = ratio {
                                // Event emission resolves the source duration,
                                // then the tuplet, then the local time stretch.
                                // Every intermediate must therefore be exactly
                                // representable at the selected tick division.
                                include_tick_ratio(
                                    &mut scale,
                                    source_division,
                                    ratio,
                                    "MuseScore base duration",
                                )?;
                                if let Some((normal, actual)) = tuplet {
                                    ratio = checked_ratio_mul(
                                        ratio,
                                        normal,
                                        actual,
                                        "MuseScore tuplet duration",
                                    )?;
                                    include_tick_ratio(
                                        &mut scale,
                                        source_division,
                                        ratio,
                                        "MuseScore tuplet duration",
                                    )?;
                                }
                                ratio = checked_ratio_mul(
                                    ratio,
                                    time_stretch.0,
                                    time_stretch.1,
                                    "MuseScore stretched note duration",
                                )?;
                                include_tick_ratio(
                                    &mut scale,
                                    source_division,
                                    ratio,
                                    "MuseScore note duration",
                                )?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(scale)
}

fn is_grace(chord: roxmltree::Node) -> bool {
    chord.children().any(|c| {
        matches!(
            c.tag_name().name(),
            "acciaccatura"
                | "appoggiatura"
                | "grace4"
                | "grace8"
                | "grace16"
                | "grace32"
                | "grace8after"
                | "grace16after"
                | "grace32after"
        )
    })
}

/// A `<Note>`'s written pitch, used only to compare the members of one chord.
///
/// Returns `None` rather than an error for anything unreadable: the projection
/// loop below reads the same value and refuses there, naming the note, so this
/// must not be a second place that decides whether a file is valid.
fn chord_note_pitch(note: roxmltree::Node) -> Option<u8> {
    child_text(note, "pitch")
        .and_then(|text| text.parse::<i64>().ok())
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= 127)
}

/// Which reading this part's chords are projected under, and the evidence that
/// decided it.
///
/// The declaration wins whenever the source makes one, because it is a statement
/// and the alternative is a measurement. Only a part that declares nothing at all
/// is measured, and then only against its own chords: a choir that harmonises
/// does so throughout, while a reduction carries the occasional chord under an
/// otherwise single-note melody.
/// Whether a chord writes a word at all.
///
/// MuseScore writes an empty `<Lyrics>` on a tied note to state that nothing is
/// sung there, so the presence of the element proves nothing. Counting those as
/// texted let a single empty continuation on a piano chord tip a whole part into
/// the harmony reading — the exact misreading this rule exists to prevent.
fn chord_carries_a_word(chord: roxmltree::Node) -> bool {
    chord
        .children()
        .filter(|node| node.has_tag_name("Lyrics"))
        .any(|lyric| {
            child(lyric, "text").is_some_and(|text| {
                let mut raw = String::new();
                deep_text_raw(text, &mut raw);
                !raw.trim().is_empty()
            })
        })
}

fn resolve_chord_reading(
    instruments: &[InstrumentInfo],
    measures: &[roxmltree::Node],
) -> (ChordReading, String, bool) {
    let mut texted = 0usize;
    let mut harmonised = 0usize;
    for measure in measures {
        for chord in measure
            .descendants()
            .filter(|node| node.has_tag_name("Chord"))
        {
            if !chord_carries_a_word(chord) {
                continue;
            }
            texted += 1;
            if chord
                .children()
                .filter(|node| node.has_tag_name("Note"))
                .count()
                > 1
            {
                harmonised += 1;
            }
        }
    }
    // Only a staff that actually writes a word over a chord had a reading to
    // take, so only that staff reports one.
    let decided_anything = harmonised > 0;
    if let Some(sings) = crate::engine::midi::part_declares_a_singer(instruments) {
        let named = instruments
            .iter()
            .find_map(|instrument| {
                instrument
                    .sound_id
                    .clone()
                    .or_else(|| instrument.id.clone())
            })
            .unwrap_or_else(|| "its MIDI program".to_string());
        return if sings {
            (
                ChordReading::Harmonised,
                format!("the part declares the singing instrument {named}"),
                decided_anything,
            )
        } else {
            (
                ChordReading::Reduction,
                format!("the part declares {named}, which does not sing"),
                decided_anything,
            )
        };
    }
    let evidence = format!(
        "the part declares no instrument, and {harmonised} of its {texted} chords \
         carrying a word hold more than one note"
    );
    if texted > 0 && harmonised * 2 >= texted {
        (ChordReading::Harmonised, evidence, decided_anything)
    } else {
        (ChordReading::Reduction, evidence, decided_anything)
    }
}

/// Every lyric lane owned by a MuseScore chord. Selection for a repeat pass is
/// deferred to the SVP projector so no source verse is discarded here.
fn chord_lyrics(
    chord: roxmltree::Node,
    source_id: &str,
    tick_scale: i64,
    ticks_per_quarter: i64,
) -> Result<Vec<Lyric>, String> {
    chord
        .children()
        .filter(|child| child.has_tag_name("Lyrics"))
        .enumerate()
        .map(|(index, lyric_node)| {
            // `<no>` is how MuseScore numbers a verse, and it omits it only for
            // the first one. Falling back to the element's position among its
            // siblings invented a verse out of a chord that simply carries the
            // same `<Lyrics>` twice — which real files do: the reported score
            // held 99 such duplicates and every lane of it was written out a
            // second time, note for note, as a phantom verse 2.
            let zero_based = match child_text(lyric_node, "no") {
                Some(text) => text
                    .parse::<u32>()
                    .map_err(|_| format!("MuseScore lyric lane number is invalid: {text:?}"))?,
                None => 0,
            };
            let verse = zero_based
                .checked_add(1)
                .ok_or_else(|| "MuseScore lyric lane number overflow".to_string())?;
            let mut raw = String::new();
            if let Some(text_node) = child(lyric_node, "text") {
                deep_text_raw(text_node, &mut raw);
            }
            // MuseScore indents formatted lyric XML. The projection trims only
            // outer formatting whitespace; `raw` and `fragments` retain the
            // decoded source text verbatim.
            let projected = raw.trim().to_string();
            let state = if projected.is_empty() {
                LyricState::ExplicitEmpty
            } else {
                LyricState::Text(projected)
            };
            let syllabic = match child_text(lyric_node, "syllabic") {
                Some("single") => Some(Syllabic::Single),
                Some("begin") => Some(Syllabic::Begin),
                Some("middle") => Some(Syllabic::Middle),
                Some("end") => Some(Syllabic::End),
                _ => None,
            };
            let extend_ticks = match child_text(lyric_node, "ticks") {
                Some(text) => Some(
                    text.parse::<i64>()
                        .map_err(|_| {
                            format!("MuseScore lyric extension ticks are invalid: {text:?}")
                        })?
                        .checked_mul(tick_scale)
                        .ok_or_else(|| {
                            "MuseScore lyric extension ticks overflow after exact scaling"
                                .to_string()
                        })?,
                ),
                None => None,
            };
            let extend_fraction = match child_text(lyric_node, "ticks_f") {
                Some(text) => Some(frac(text).ok_or_else(|| {
                    format!("MuseScore lyric extension fraction is invalid: {text:?}")
                })?),
                None => None,
            };
            // A score may state the extension only as a fraction. Measured over
            // the pinned corpus, every one of the 10968 lyrics stating both puts
            // `ticks_f` in whole notes against `ticks` in Division units, with no
            // exception — so the fraction is read in the same unit here. Derived
            // exactly or refused, never rounded, as `<ticks>` already is.
            let extend_ticks = match (extend_ticks, extend_fraction) {
                (None, Some((numerator, denominator))) if numerator != 0 => {
                    let quarters = numerator
                        .checked_mul(4)
                        .and_then(|whole_notes| whole_notes.checked_mul(ticks_per_quarter))
                        .ok_or_else(|| {
                            "MuseScore lyric extension fraction overflows in ticks".to_string()
                        })?;
                    if denominator == 0 || quarters % denominator != 0 {
                        return Err(format!(
                            "MuseScore lyric extension fraction {numerator}/{denominator} is not \
                             a whole number of ticks at this division"
                        ));
                    }
                    Some(quarters / denominator)
                }
                (existing, _) => existing,
            };
            Ok(Lyric {
                id: format!("{source_id}-lyric-{index}"),
                raw: raw.clone(),
                raw_bytes: Vec::new(),
                fragments: vec![LyricFragment::Text(raw)],
                lane: verse.to_string(),
                verse,
                verse_from_score: true,
                state,
                syllabic,
                line_break: None,
                time_only: Vec::new(),
                extension: None,
                extend_ticks,
                extend_fraction,
            })
        })
        .collect()
}

/// Retains both numeric statements rather than silently preferring ticks when
/// the fraction contradicts it. Legacy lyric parsing remains unchanged; only
/// independently validated bounds authorize the continuity planner.
#[allow(clippy::too_many_arguments)]
fn chord_extensions(
    chord: roxmltree::Node,
    lyrics: &[Lyric],
    chord_id: &str,
    occurrence: u32,
    playback_segment: u32,
    start_tick: u32,
    division: i64,
    pool: &mut ContinuityEvidencePool,
) -> Result<(Vec<SourceExtension>, Vec<SourceContinuityIssue>), String> {
    let mut extensions = Vec::new();
    let mut issues = Vec::new();
    for (node, lyric) in chord
        .children()
        .filter(|node| node.has_tag_name("Lyrics"))
        .zip(lyrics)
    {
        if child(node, "ticks").is_none() && child(node, "ticks_f").is_none() {
            continue;
        }
        let evidence = pool.evidence(node, &lyric.id)?;
        pool.charge(
            std::mem::size_of::<SourceExtension>()
                + lyric.id.len()
                + lyric.lane.len()
                + chord_id.len(),
        )?;
        let raw_ticks = child_text(node, "ticks").and_then(|text| text.parse().ok());
        let fraction_ticks = lyric.extend_fraction.map(|(numerator, denominator)| {
            numerator.checked_mul(4).and_then(|numerator| {
                exact_ticks(
                    division,
                    (numerator, denominator),
                    "MuseScore lyric extension",
                )
                .ok()
            })
        });
        // Repeated declarations may be redundant, but the first child cannot
        // hide a conflicting or malformed later statement. Compare fractions
        // numerically while preserving every original spelling in raw XML.
        let repeated_fields_agree =
            node.children()
                .filter(|child| child.is_element())
                .all(|child| match child.tag_name().name() {
                    "ticks" => child
                        .text()
                        .and_then(|text| text.trim().parse::<i64>().ok())
                        .is_some_and(|ticks| Some(ticks) == raw_ticks),
                    "ticks_f" => child
                        .text()
                        .and_then(frac)
                        .zip(lyric.extend_fraction)
                        .is_some_and(|((left_n, left_d), (right_n, right_d))| {
                            i128::from(left_n) * i128::from(right_d)
                                == i128::from(right_n) * i128::from(left_d)
                        }),
                    _ => true,
                });
        let contradictory = !repeated_fields_agree
            || match (lyric.extend_ticks, fraction_ticks) {
                (_, Some(None)) => true,
                (Some(ticks), Some(Some(fraction))) => ticks != fraction,
                _ => false,
            };
        let ticks = lyric.extend_ticks.or_else(|| fraction_ticks.flatten());
        let end_tick = (!contradictory)
            .then_some(ticks)
            .flatten()
            .and_then(|ticks| u32::try_from(ticks).ok())
            .and_then(|ticks| start_tick.checked_add(ticks));
        if end_tick.is_none() {
            issues.push(SourceContinuityIssue {
                code: "SOURCE_CONTINUITY_LINK_INVALID",
                message: format!(
                    "Lyric {} on chord {chord_id}, occurrence {occurrence}, segment \
                     {playback_segment}, start {start_tick}: extension ticks {:?} and \
                     fraction {:?} have invalid or contradictory exact bounds \
                     (including repeated ticks/ticks_f declarations)",
                    lyric.id, lyric.extend_ticks, lyric.extend_fraction
                ),
                evidence: evidence.clone(),
            });
        }
        extensions.push(SourceExtension {
            lyric_id: lyric.id.clone(),
            lane: lyric.lane.clone(),
            chord_id: chord_id.to_string(),
            occurrence,
            playback_segment,
            start_tick,
            end_tick,
            extend_ticks: lyric.extend_ticks,
            extend_fraction: lyric.extend_fraction,
            raw_ticks,
            evidence,
        });
    }
    Ok((extensions, issues))
}

/// Playback order of the measures: repeats, voltas, D.S./D.C., Coda, Fine.
/// Playback structure is a property of the score, not of one staff: MuseScore
/// normally writes repeat barlines on the first staff only, and a few scores
/// split them (barlines on one staff, voltas on another). Unrolling each staff
/// against its own marks therefore truncates every staff that carries none.
/// Merging is a union, and two staves stating different values for the same
/// mark is a contradiction we refuse rather than arbitrate.
fn score_playback_order(
    staff_measures: &[Vec<roxmltree::Node>],
) -> Result<Vec<(usize, u32, u32)>, String> {
    let Some(first) = staff_measures.first() else {
        return Ok(Vec::new());
    };
    for (index, measures) in staff_measures.iter().enumerate().skip(1) {
        if measures.len() != first.len() {
            return Err(format!(
                "MuseScore staves disagree on measure count: staff 1 has {}, staff {} has {}",
                first.len(),
                index + 1,
                measures.len()
            ));
        }
    }
    let mut merged = vec![MeasureMarks::default(); first.len()];
    for measures in staff_measures {
        for (target, marks) in merged.iter_mut().zip(measure_marks(measures)?) {
            merge_measure_marks(target, marks, "MuseScore staves")?;
        }
    }
    unroll_with_passes(&merged)
}

fn measure_marks(measures: &[roxmltree::Node]) -> Result<Vec<MeasureMarks>, String> {
    let mut marks = vec![MeasureMarks::default(); measures.len()];
    let mut volta_spans: Vec<(usize, usize, Vec<u32>)> = Vec::new();

    for (i, m) in measures.iter().enumerate() {
        marks[i].start_repeat = m.children().any(|c| c.has_tag_name("startRepeat"));
        if let Some(er) = m.children().find(|c| c.has_tag_name("endRepeat")) {
            marks[i].end_repeat = er
                .text()
                .and_then(|t| t.trim().parse::<u32>().ok())
                .unwrap_or(2)
                .max(2);
        }
        for el in m.descendants().filter(|d| d.is_element()) {
            match el.tag_name().name() {
                "Marker" => {
                    let ty = child_text(el, "type").unwrap_or("");
                    let label = child_text(el, "label").unwrap_or("");
                    match ty {
                        "segno" | "varsegno" => marks[i].segno = true,
                        "codab" | "coda" | "varcoda" | "codetta" => marks[i].coda = true,
                        "toCoda" | "toCodaSym" => marks[i].to_coda = true,
                        "fine" => marks[i].fine = true,
                        _ => match label {
                            // MuseScore legacy: label "coda" = To Coda point,
                            // label "codab" = coda symbol (target)
                            "segno" => marks[i].segno = true,
                            "codab" => marks[i].coda = true,
                            "coda" => marks[i].to_coda = true,
                            "fine" => marks[i].fine = true,
                            _ => {}
                        },
                    }
                }
                "Jump" => {
                    let to = child_text(el, "jumpTo").unwrap_or("");
                    let until = child_text(el, "playUntil").unwrap_or("");
                    let ds = to.contains("segno");
                    marks[i].jump = Some(if until == "fine" {
                        if ds {
                            Jump::DsAlFine
                        } else {
                            Jump::DcAlFine
                        }
                    } else if until.contains("coda") {
                        if ds {
                            Jump::DsAlCoda
                        } else {
                            Jump::DcAlCoda
                        }
                    } else if ds {
                        Jump::Ds
                    } else {
                        Jump::Dc
                    });
                }
                "Spanner" if el.attribute("type") == Some("Volta") => {
                    if let Some(v) = el.children().find(|c| c.has_tag_name("Volta")) {
                        let endings: Vec<u32> = child_text(v, "endings")
                            .unwrap_or("1")
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter_map(|s| s.trim().parse().ok())
                            .collect();
                        let span = el
                            .children()
                            .find(|c| c.has_tag_name("next"))
                            .and_then(|nx| nx.children().find(|c| c.has_tag_name("location")))
                            .and_then(|loc| child_text(loc, "measures"))
                            .and_then(|t| t.trim().parse::<usize>().ok())
                            .unwrap_or(1)
                            .max(1);
                        let endings = if endings.is_empty() { vec![1] } else { endings };
                        volta_spans.push((i, span, endings));
                    }
                }
                _ => {}
            }
        }
    }
    for (start, span, endings) in volta_spans {
        for k in start..(start + span).min(marks.len()) {
            marks[k].volta = Some(endings.clone());
        }
    }
    Ok(marks)
}

/// Reuse the parser's declaration fallback once for topology and linkage evidence.
fn declaration_staff_id(
    part: roxmltree::Node,
    staff: roxmltree::Node,
    part_index: usize,
    staff_index: usize,
    staff_cursor: usize,
    body_ids: &[&str],
) -> String {
    staff
        .attribute("id")
        .map(str::to_string)
        .or_else(|| body_ids.get(staff_cursor).map(|id| (*id).to_string()))
        .or_else(|| {
            // Only the first declaration can be the sole Staff. Avoid scanning
            // a large Part again for every unresolved anonymous declaration.
            (staff_index == 0
                && part
                    .children()
                    .filter(|node| node.has_tag_name("Staff"))
                    .count()
                    == 1)
                .then(|| part.attribute("id"))
                .flatten()
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("{}-{}", part_index + 1, staff_index + 1))
}

/// MuseScore 3.6.2 Staff::write/readProperties uses masterScore staff indices
/// (one-based on disk); Part::readProperties appends Staffs in declaration order.
/// Source: libmscore/staff.cpp:628-634,762-779 and part.cpp:105-112 at v3.6.2.
/// XML IDs identify bodies, not linkedTo destinations. An index into this selected
/// Score is only a candidate: external masters may use the same numbers.
fn linked_staff_views(
    score: roxmltree::Node,
) -> Result<std::collections::HashMap<roxmltree::NodeId, Option<String>>, String> {
    let body_ids: Vec<_> = score
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .filter_map(|node| node.attribute("id"))
        .collect();
    let mut declarations = Vec::new();
    let mut declaration_ids = BTreeSet::new();
    let mut bodies = BTreeMap::new();
    for (part_index, part) in score
        .children()
        .filter(|node| node.has_tag_name("Part"))
        .enumerate()
    {
        for (staff_index, staff) in part
            .children()
            .filter(|node| node.has_tag_name("Staff"))
            .enumerate()
        {
            let id = declaration_staff_id(
                part,
                staff,
                part_index,
                staff_index,
                declarations.len(),
                &body_ids,
            );
            if !declaration_ids.insert(id.clone()) {
                return Err(format!(
                    "MuseScore duplicate resolved staff declaration ID: {id:?}"
                ));
            }
            declarations.push((staff, id));
        }
    }
    for staff in score.children().filter(|node| node.has_tag_name("Staff")) {
        if let Some(id) = staff.attribute("id") {
            if bodies.insert(id, staff).is_some() {
                return Err(format!("MuseScore duplicate score-body staff ID: {id:?}"));
            }
        }
    }
    let mut result = std::collections::HashMap::new();
    if !declarations
        .iter()
        .any(|(staff, _)| child(*staff, "linkedTo").is_some())
    {
        return Ok(result);
    }
    // Cache each body once. An opaque relationship anywhere in the selected
    // body could refer to a staff being removed, so it blocks identity erasure.
    // This bounded proof deliberately supports a subset; unknown content stays.
    let mut normalized_targets = std::collections::HashMap::new();
    let mut body_relationships_known = true;
    for body in score.children().filter(|node| node.has_tag_name("Staff")) {
        let normalized = normalized_staff_content(body);
        body_relationships_known &= normalized.is_some();
        normalized_targets.insert(body.id(), normalized);
    }
    for (staff, id) in &declarations {
        let links: Vec<_> = staff
            .children()
            .filter(|node| node.has_tag_name("linkedTo"))
            .collect();
        if links.is_empty() {
            continue;
        }
        let canonical = (|| {
            let [link] = links.as_slice() else {
                return None;
            };
            if !body_relationships_known || !plain_leaf(*link, &[]) {
                return None;
            }
            let index = link.text()?.trim().parse::<usize>().ok()?.checked_sub(1)?;
            let (target, target_id) = declarations.get(index)?;
            if target == staff
                || target.parent() != staff.parent()
                || child(*target, "linkedTo").is_some()
            {
                return None;
            }
            let body = *bodies.get(id.as_str())?;
            let target_body = *bodies.get(target_id.as_str())?;
            if !body.children().any(|node| node.has_tag_name("Measure")) {
                return None;
            }
            let declaration = normalized_staff_content(*staff)?;
            let target_declaration = normalized_targets
                .entry(target.id())
                .or_insert_with(|| normalized_staff_content(*target))
                .as_ref()?;
            if &declaration != target_declaration {
                return None;
            }
            if normalized_targets.get(&body.id())?.as_ref()?
                != normalized_targets.get(&target_body.id())?.as_ref()?
            {
                return None;
            }
            Some(target_id.clone())
        })();
        result.insert(staff.id(), canonical);
    }
    Ok(result)
}

fn plain_attributes(node: roxmltree::Node, allowed: &[&str]) -> bool {
    node.tag_name().namespace().is_none()
        && node
            .attributes()
            .all(|attribute| attribute.namespace().is_none() && allowed.contains(&attribute.name()))
}

fn plain_leaf(node: roxmltree::Node, attributes: &[&str]) -> bool {
    plain_attributes(node, attributes) && !node.children().any(|node| node.is_element())
}

/// An ignored visual property must be completely understood, including children
/// and attributes. Namespaced lookalikes and extra payload are never erased.
fn harmless_visual(node: roxmltree::Node) -> bool {
    let tag = node.tag_name().name();
    let Some(parent) = node.parent() else {
        return false;
    };
    if node.tag_name().namespace().is_some() || parent.tag_name().namespace().is_some() {
        return false;
    }
    let leaf = |attrs: &[&str]| plain_leaf(node, attrs);
    let number = || {
        node.text()
            .is_some_and(|text| text.trim().parse::<f64>().is_ok_and(f64::is_finite))
    };
    match (parent.tag_name().name(), tag) {
        ("Staff", "linkedTo") => leaf(&[]),
        ("Staff", "StaffType") => {
            plain_attributes(node, &["group"])
                && matches!(node.attribute("group"), Some("pitched" | "tab"))
                && node.children().all(|child| {
                    if child.is_text() {
                        return child.text().unwrap_or("").trim().is_empty();
                    }
                    if !child.is_element() {
                        return true;
                    }
                    plain_leaf(child, &[])
                        && match child.tag_name().name() {
                            "name" => true,
                            "lines" | "lineDistance" => child.text().is_some_and(|text| {
                                text.trim().parse::<f64>().is_ok_and(f64::is_finite)
                            }),
                            _ => false,
                        }
                })
        }
        ("Staff", "defaultClef" | "defaultConcertClef" | "defaultTransposingClef") => {
            leaf(&[])
                && matches!(
                    node.text().map(str::trim),
                    Some("G" | "F" | "C3" | "C4" | "TAB" | "TAB4")
                )
        }
        ("Staff", "barLineSpan") => leaf(&[]) && number(),
        ("Staff", "bracket") => {
            leaf(&["type", "span", "col"])
                && node.text().unwrap_or("").trim().is_empty()
                && node
                    .attributes()
                    .all(|attribute| attribute.value().parse::<i32>().is_ok())
        }
        ("Chord", "stemDirection") => {
            leaf(&[]) && matches!(node.text().map(str::trim), Some("up" | "down" | "auto"))
        }
        ("Note", "fret" | "string") => {
            leaf(&[])
                && node
                    .text()
                    .is_some_and(|text| text.trim().parse::<i32>().is_ok())
        }
        ("Chord" | "Note" | "Rest" | "Lyrics" | "Tempo" | "Clef", "offset" | "pos") => {
            leaf(&["x", "y"])
                && node.text().unwrap_or("").trim().is_empty()
                && node
                    .attributes()
                    .all(|attribute| attribute.value().parse::<f64>().is_ok_and(f64::is_finite))
        }
        ("Chord" | "Note" | "Rest" | "Lyrics" | "Tempo" | "Clef", "visible" | "autoplace") => {
            leaf(&[]) && matches!(node.text().map(str::trim), Some("0" | "1"))
        }
        _ => false,
    }
}

/// Flatten only known harmless rich-text wrappers. Preserve semantic whitespace;
/// unknown markup, namespaced lookalikes and unknown attributes block equivalence.
fn equivalent_text(node: roxmltree::Node, out: &mut String) -> Option<()> {
    let tag = node.tag_name().name();
    let allowed = match tag {
        "text" | "b" | "i" | "u" | "s" | "sup" | "sub" => &[][..],
        "font" => &["face", "size"][..],
        "br" if plain_leaf(node, &[]) && node.text().unwrap_or("").is_empty() => {
            out.push(' ');
            return Some(());
        }
        _ => return None,
    };
    if !plain_attributes(node, allowed) {
        return None;
    }
    if let Some(size) = node.attribute("size") {
        if !size.parse::<f64>().is_ok_and(f64::is_finite) {
            return None;
        }
    }
    for child in node.children() {
        if child.is_text() {
            out.push_str(child.text().unwrap_or(""));
        } else if child.is_element() {
            equivalent_text(child, out)?;
        }
    }
    Some(())
}

/// Exact structural proof over a bounded known subset, not a second rhythm
/// parser or a lossy target projection. Object IDs and unknown relationship
/// syntax fail closed. QName tokens include namespaces; only the outer Staff ID
/// is omitted after the selected body's relationships have passed this proof.
fn normalized_staff_content(staff: roxmltree::Node) -> Option<Vec<String>> {
    fn visit(node: roxmltree::Node, root: roxmltree::Node, out: &mut Vec<String>) -> Option<()> {
        if harmless_visual(node) {
            return Some(());
        }
        let tag = node.tag_name().name();
        let qname = (node.tag_name().namespace(), tag);
        if node == root {
            if !plain_attributes(node, &["id"]) {
                return None;
            }
        } else {
            let allowed = match tag {
                "Measure" => &["len"][..],
                "Spanner" => &["type"][..],
                "voice" | "Chord" | "Rest" | "Note" | "Lyrics" | "TimeSig" | "Tempo"
                | "durationType" | "duration" | "dots" | "pitch" | "tpc" | "tpc2" | "velocity"
                | "veloType" | "play" | "tuning" | "syllabic" | "no" | "ticks" | "ticks_f"
                | "text" | "sigN" | "sigD" | "stretchN" | "stretchD" | "tempo" | "followText"
                | "Tuplet" | "normalNotes" | "actualNotes" | "endTuplet" | "location"
                | "fractions" | "measures" | "voices" | "startRepeat" | "endRepeat" | "Marker"
                | "Jump" | "label" | "type" | "jumpTo" | "playUntil" | "continueAt"
                | "playRepeats" | "Tie" | "Slur" | "Volta" | "endings" | "next" | "prev"
                | "acciaccatura" | "appoggiatura" | "grace4" | "grace8" | "grace16" | "grace32"
                | "grace8after" | "grace16after" | "grace32after" => &[][..],
                // Staff offsets, absolute staff references, object IDs, linked
                // object markup and all unknown syntax need a relationship proof
                // this conservative qualifier does not attempt.
                _ => return None,
            };
            if !plain_attributes(node, allowed) {
                return None;
            }
        }
        out.push(format!("open:{qname:?}"));
        if node != root {
            let mut attributes: Vec<_> = node
                .attributes()
                .map(|attribute| (attribute.namespace(), attribute.name(), attribute.value()))
                .collect();
            attributes.sort_unstable();
            for attribute in attributes {
                out.push(format!("attribute:{attribute:?}"));
            }
        }
        if tag == "text" {
            let mut text = String::new();
            equivalent_text(node, &mut text)?;
            out.push(format!("text:{text}"));
        } else {
            let has_elements = node.children().any(|child| child.is_element());
            for child in node.children() {
                if child.is_element() {
                    visit(child, root, out)?;
                } else if child.is_text() {
                    let text = child.text().unwrap_or("");
                    if !has_elements || !text.trim().is_empty() {
                        out.push(format!("text:{text}"));
                    }
                }
            }
        }
        out.push(format!("close:{qname:?}"));
        Some(())
    }
    let mut out = Vec::new();
    visit(staff, staff, &mut out)?;
    Some(out)
}

pub fn parse_mscx(xml: &str) -> Result<Midi, String> {
    crate::engine::musicxml::check_nesting(xml)?;
    let opts = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 5_000_000, // bounds the memory cost of a forged XML
    };
    let doc = roxmltree::Document::parse_with_options(xml, opts)
        .map_err(|e| format!("invalid XML: {}", e))?;
    let mut continuity_evidence = ContinuityEvidencePool::new(doc.root_element())?;
    let score = doc
        .descendants()
        .find(|n| n.has_tag_name("Score"))
        .ok_or_else(|| "MuseScore: Score element not found".to_string())?;
    let source_division = match child_text(score, "Division") {
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|division| (1..=i64::from(u16::MAX)).contains(division))
            .ok_or_else(|| format!("MuseScore Division is invalid: {value:?}"))?,
        // MuseScore's documented default tick division when the element is
        // absent. A present malformed value is never replaced.
        None => 480,
    };
    let tick_scale = musescore_tick_scale(score, source_division)?;
    let div = source_division
        .checked_mul(tick_scale)
        .ok_or_else(|| "MuseScore exact tick division overflow".to_string())?;
    let tpb = u16::try_from(div).map_err(|_| "MuseScore Division exceeds the SVP time base")?;

    #[derive(Clone, Debug, Default)]
    struct StaffInfo {
        part_id: String,
        name: String,
        role: TrackRoleHint,
        instruments: Vec<InstrumentInfo>,
    }

    let top_level_staff_ids: Vec<&str> = score
        .children()
        .filter(|node| node.has_tag_name("Staff"))
        .filter_map(|node| node.attribute("id"))
        .collect();
    let mut staff_cursor = 0usize;
    let mut staff_info: BTreeMap<String, StaffInfo> = BTreeMap::new();
    let linked_views = linked_staff_views(score)?;
    let mut excluded_staff_ids = BTreeSet::new();
    let mut staff_links = Vec::new();
    let mut resolved_staff_ids = BTreeSet::new();
    let body_measure_ids: BTreeSet<_> = score
        .children()
        .filter(|node| {
            node.has_tag_name("Staff") && node.children().any(|child| child.has_tag_name("Measure"))
        })
        .filter_map(|node| node.attribute("id"))
        .collect();
    let mut declared_parts = Vec::new();
    for (part_index, part) in score
        .children()
        .filter(|n| n.has_tag_name("Part"))
        .enumerate()
    {
        let part_id = part
            .attribute("id")
            .map(|value| format!("musescore-part-{value}"))
            .unwrap_or_else(|| format!("musescore-part-{part_index}"));
        // `longName` is the name the score shows and the one MuseScore exports as
        // MusicXML's `<part-name>`, so reading it is what makes the same score
        // name its lanes identically whichever way the user exported it. On the
        // reported score the three sung parts are all `<trackName>Piano</trackName>`
        // — the template's instrument, never renamed — while `longName` carries
        // the names the author actually gave them.
        //
        // `trackName` was preferred here for `--score-parts` stem alignment, but
        // that alignment is by ordinal: MuseScore rewrites both names and opaque
        // IDs even for a native `.mscz`, so a name was never a usable key.
        let name = part
            .children()
            .find(|c| c.has_tag_name("Instrument"))
            .and_then(|i| child(i, "longName").map(|n| collapse_ws(&deep_text(n))))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                child(part, "trackName")
                    .map(|n| collapse_ws(&deep_text(n)))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        let instrument_node = part.children().find(|c| c.has_tag_name("Instrument"));
        let mut instruments = Vec::new();
        if let Some(instrument_node) = instrument_node {
            let id = instrument_node
                .attribute("id")
                .map(str::to_string)
                .or_else(|| child_text(instrument_node, "instrumentId").map(str::to_string));
            // Kept separately from `id`, which stays whatever it has always been
            // because topology and stem alignment are keyed on it. `instrumentId`
            // is the taxonomy value — `keyboard.piano`, `voice.soprano` — and it
            // is the only statement of what a part *is* that does not depend on
            // the language its name is written in.
            let sound_id = child_text(instrument_node, "instrumentId").map(str::to_string);
            let instrument_name = child(instrument_node, "longName")
                .map(|node| collapse_ws(&deep_text(node)))
                .filter(|value| !value.is_empty())
                .or_else(|| child_text(instrument_node, "trackName").map(str::to_string));
            let percussion = child_text(instrument_node, "useDrumset") == Some("1")
                || instrument_node
                    .descendants()
                    .any(|node| node.has_tag_name("Drum"));
            let channels: Vec<_> = instrument_node
                .children()
                .filter(|node| node.has_tag_name("Channel"))
                .collect();
            if channels.is_empty() {
                instruments.push(InstrumentInfo {
                    id,
                    sound_id: sound_id.clone(),
                    name: instrument_name,
                    percussion,
                    ..InstrumentInfo::default()
                });
            } else {
                for (channel_index, channel_node) in channels.into_iter().enumerate() {
                    let source_channel = channel_node
                        .attribute("channel")
                        .and_then(|value| value.parse::<i32>().ok())
                        .or_else(|| {
                            child_text(channel_node, "channel")
                                .and_then(|value| value.parse::<i32>().ok())
                        });
                    let source_program = child(channel_node, "program")
                        .and_then(|program| program.attribute("value"))
                        .and_then(|value| value.parse::<i32>().ok());
                    let controllers: Vec<(u8, u8)> = channel_node
                        .children()
                        .filter(|node| node.has_tag_name("controller"))
                        .filter_map(|node| {
                            let controller = node.attribute("ctrl")?.parse::<u8>().ok()?;
                            let value = node.attribute("value")?.parse::<u8>().ok()?;
                            Some((controller, value))
                        })
                        .collect();
                    let controller = |number| {
                        controllers
                            .iter()
                            .find_map(|&(key, value)| (key == number).then_some(value))
                    };
                    instruments.push(InstrumentInfo {
                        id: id
                            .clone()
                            .map(|value| format!("{value}:channel:{channel_index}")),
                        sound_id: sound_id.clone(),
                        name: instrument_name.clone(),
                        source_channel,
                        source_program,
                        channel: source_channel.and_then(|value| u8::try_from(value).ok()),
                        program: source_program.and_then(|value| u8::try_from(value).ok()),
                        bank_msb: controller(0),
                        bank_lsb: controller(32),
                        volume: controller(7).map(f64::from),
                        pan: controller(10).map(f64::from),
                        controllers,
                        percussion,
                        ..InstrumentInfo::default()
                    });
                }
            }
        }
        let part_staves: Vec<_> = part
            .children()
            .filter(|child| child.has_tag_name("Staff"))
            .collect();
        let mut declared_staves = Vec::new();
        for (staff_index, staff) in part_staves.iter().copied().enumerate() {
            let staff_id = declaration_staff_id(
                part,
                staff,
                part_index,
                staff_index,
                staff_cursor,
                &top_level_staff_ids,
            );
            staff_cursor += 1;
            if !resolved_staff_ids.insert(staff_id.clone()) {
                return Err(format!(
                    "MuseScore duplicate resolved staff declaration ID: {staff_id:?}"
                ));
            }
            for (link_index, link) in staff
                .children()
                .filter(|node| node.has_tag_name("linkedTo"))
                .enumerate()
            {
                staff_links.push(StaffLink {
                    id: format!(
                        "mscx:part:{part_index}:staff-declaration:{staff_index}:link:{link_index}"
                    ),
                    part_id: part_id.clone(),
                    staff_id: staff_id.clone(),
                    linked_to: link.text().unwrap_or("").to_string(),
                    canonical_staff_id: linked_views.get(&staff.id()).cloned().flatten(),
                    has_body_measures: body_measure_ids.contains(staff_id.as_str()),
                });
            }
            if linked_views.get(&staff.id()).is_some_and(Option::is_some) {
                // Declaration IDs may be absent. Use the resolved identity
                // shared by topology and the score body, never the raw attribute.
                excluded_staff_ids.insert(staff_id);
                continue;
            }
            declared_staves.push(SourceStaff {
                id: staff_id.clone(),
                voices: Vec::new(),
            });
            let group = staff
                .children()
                .find(|node| node.has_tag_name("StaffType"))
                .and_then(|node| node.attribute("group"));
            let percussion = matches!(group, Some("percussion" | "unpitched"))
                || instruments.iter().any(|instrument| instrument.percussion);
            staff_info.insert(
                staff_id,
                StaffInfo {
                    part_id: part_id.clone(),
                    name: name.clone(),
                    role: if percussion {
                        TrackRoleHint::Percussion
                    } else {
                        TrackRoleHint::Ambiguous
                    },
                    instruments: instruments.clone(),
                },
            );
        }
        declared_parts.push(SourcePart {
            id: part_id,
            name,
            source_track_ids: Vec::new(),
            staves: declared_staves,
        });
    }

    // Voice containers exist independently of pitched notes. Seed them before
    // projection so a rest-only source voice remains visible in topology.
    for staff in score.children().filter(|node| node.has_tag_name("Staff")) {
        let staff_id = staff
            .attribute("id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("anonymous-declared-{}", declared_parts.len() + 1));
        let Some(info) = staff_info.get(&staff_id) else {
            continue;
        };
        let mut voice_count = 0usize;
        for measure in staff.children().filter(|node| node.has_tag_name("Measure")) {
            voice_count = voice_count.max(measure_voice_containers(measure)?.len());
        }
        let Some(source_staff) = declared_parts
            .iter_mut()
            .find(|part| part.id == info.part_id)
            .and_then(|part| part.staves.iter_mut().find(|staff| staff.id == staff_id))
        else {
            continue;
        };
        for voice_index in 0..voice_count {
            let number = (voice_index + 1).to_string();
            source_staff.voices.push(SourceVoice {
                id: format!("{}:staff:{}:voice:{}", info.part_id, staff_id, number),
                number,
                projection_track_ids: Vec::new(),
            });
        }
    }

    let mut score_expression = super::score_intensity::source::ScoreInput::default();
    let modern_expression = doc
        .root_element()
        .attribute("version")
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|v| v >= 4);
    let mut tracks = Vec::new();
    let mut global_events = Vec::new();
    let mut local_meter_fallbacks = Vec::new();

    let score_staves: Vec<_> = score
        .children()
        .filter(|n| n.has_tag_name("Staff"))
        .filter(|n| {
            !n.attribute("id")
                .is_some_and(|id| excluded_staff_ids.contains(id))
        })
        .collect();
    let staff_measures: Vec<Vec<_>> = score_staves
        .iter()
        .map(|staff| {
            staff
                .children()
                .filter(|n| n.has_tag_name("Measure"))
                .collect()
        })
        .collect();
    let score_order = score_playback_order(&staff_measures)?;
    let mut unresolved_tie_diagnostics = 0usize;

    for (staff_index, &staff) in score_staves.iter().enumerate() {
        let staff_id = staff
            .attribute("id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("anonymous-{}", tracks.len() + 1));
        let info = staff_info.get(&staff_id).cloned().unwrap_or_default();
        // A part with several staves gives each of them the part's name, which
        // wrote two different lanes into a project under one identical label.
        // Number them only when there is something to tell apart, so a
        // single-staff part keeps the name it has always had.
        // Counted over the staves the score body actually holds, in the order it
        // writes them — not over what the Part declares and not in key order. A
        // Part can declare a staff its body never carries, and staff ids sort
        // lexicographically, so `10` would otherwise come before `9` and the
        // second staff of a piano would be labelled the first.
        let siblings: Vec<&str> = score_staves
            .iter()
            .filter_map(|other| other.attribute("id"))
            .filter(|other| {
                staff_info
                    .get(*other)
                    .is_some_and(|other| other.part_id == info.part_id)
            })
            .collect();
        let staff_ordinal = (siblings.len() > 1)
            .then(|| {
                siblings
                    .iter()
                    .position(|other| *other == staff_id)
                    .map(|index| index + 1)
            })
            .flatten();
        // What a chord carrying one written word asks for, decided once per
        // staff before any of its music is read, so every chord of a part is
        // projected under the same reading.
        let (chord_reading, chord_reading_evidence, chord_reading_decided) =
            resolve_chord_reading(&info.instruments, &staff_measures[staff_index]);
        let mut voice_events: BTreeMap<(usize, Option<usize>), Vec<Event>> = BTreeMap::new();
        let mut unassigned_chord_lyrics = Vec::new();
        // Scoped with `voice_events`, so tie state is per staff without needing
        // a staff component in the key.
        let mut active_ties: BTreeMap<TieKey, PendingTie> = BTreeMap::new();
        let mut outgoing_ties: BTreeMap<TieKey, OutgoingTie> = BTreeMap::new();
        let mut previous_measure: Option<usize> = None;
        let mut playback_segment = 0u32;

        let mut measure_start: i64 = 0;
        let mut measure_len: i64 = 4 * div; // 4/4 by default
        let mut time_stretch = (1i64, 1i64);

        let measures = &staff_measures[staff_index];
        let global_before = global_events.len();
        let local_before = local_meter_fallbacks.len();
        let tie_diagnostics_before = unresolved_tie_diagnostics;
        let written_count = measures.len();
        let traversal: Vec<_> = (0..written_count)
            .map(|i| (i, 0, 1))
            .chain(score_order.iter().copied())
            .collect();
        let mut written_bounds = Vec::with_capacity(written_count);
        let mut written_memberships = Vec::with_capacity(written_count);
        let mut expression_tempo = super::score_intensity::Fraction::integer(120);
        for (visit, &(mi, pass, repeat_pass)) in traversal.iter().enumerate() {
            let recording = visit < written_count;
            if visit == written_count {
                score_expression.finish_staff(&written_bounds, source_division as u16)?;
                voice_events.clear();
                active_ties.clear();
                // The first traversal only captures written expression intent.
                // Its outgoing addresses refer to the discarded event buffers;
                // playback creates and diagnoses its own source occurrences.
                outgoing_ties.clear();
                unresolved_tie_diagnostics = tie_diagnostics_before;
                playback_segment = 0;
                unassigned_chord_lyrics.clear();
                previous_measure = None;
                measure_start = 0;
                measure_len = 4 * div;
                time_stretch = (1, 1);
                global_events.truncate(global_before);
                local_meter_fallbacks.truncate(local_before);
            }
            let forward = previous_measure.is_some_and(|previous| mi == previous + 1);
            // A repeat, volta or jump has just broken notated continuity. A tie
            // location points at the next NOTATED measure, which is no longer
            // the next PLAYED one, so open chains cannot be proven any more.
            if previous_measure.is_some_and(|previous| mi != previous + 1) {
                abandon_tie_starts(
                    &mut outgoing_ties,
                    &mut voice_events,
                    "playback discontinuity abandoned the source declaration",
                    &mut unresolved_tie_diagnostics,
                )?;
                active_ties.clear();
                playback_segment = playback_segment
                    .checked_add(1)
                    .ok_or_else(|| "MuseScore playback segment overflow".to_string())?;
            }
            previous_measure = Some(mi);
            let measure = measures[mi];
            if recording {
                written_memberships.push(score_expression.begin_written_measure(
                    &info.part_id,
                    Some(&staff_id),
                    mi,
                    super::score_intensity::source::time(measure_start, tpb)?,
                )?);
            }
            let mut this_len = measure_len;
            for (voice_index, voice) in measure_voice_containers(measure)?.into_iter().enumerate() {
                let mut pos = measure_start;
                let mut tuplet: Option<(i64, i64)> = None; // (normal, actual)
                for (element_index, el) in voice.children().filter(|n| n.is_element()).enumerate() {
                    if recording {
                        let owner = super::score_intensity::ScoreVoice {
                            part: info.part_id.clone(),
                            staff: staff_id.clone(),
                            voice: (voice_index + 1).to_string(),
                            instrument: info.instruments.first().and_then(|i| i.id.clone()),
                        };
                        score_expression.ms_element(el,
                            &format!("expression:mscx:staff:{staff_id}:measure:{mi}:voice:{voice_index}:element:{element_index}"),
                            &owner, super::score_intensity::source::time(pos,tpb)?, mi, modern_expression, expression_tempo, time_stretch)?;
                    }
                    match el.tag_name().name() {
                        "TimeSig" => {
                            let signature = musescore_time_signature(el)?;
                            time_stretch = signature.stretch;
                            let effective_quarters = checked_ratio_mul(
                                signature.effective,
                                4,
                                1,
                                "MuseScore time-signature duration",
                            )?;
                            measure_len = exact_ticks(
                                div,
                                effective_quarters,
                                "MuseScore time-signature duration",
                            )?;
                            if measure_len <= 0 {
                                return Err(
                                    "MuseScore time-signature duration is non-positive".into()
                                );
                            }
                            this_len = measure_len;
                            let meter_ratio = if signature.stretch == (1, 1) {
                                (signature.numerator, signature.denominator)
                            } else {
                                signature.effective
                            };
                            let (numerator, denominator) = checked_meter_values(
                                meter_ratio,
                                "MuseScore effective time signature",
                            )?;
                            let destination = if signature.stretch == (1, 1) {
                                &mut global_events
                            } else {
                                &mut local_meter_fallbacks
                            };
                            push_meter_event(
                                destination,
                                checked_score_tick(pos)?,
                                numerator,
                                denominator,
                            )?;
                        }
                        "Tempo" => {
                            // <tempo> = quarter notes per second
                            let tempo_text = child_text(el, "tempo")
                                .ok_or_else(|| "MuseScore Tempo is missing tempo".to_string())?;
                            let quarters_per_second = tempo_text
                                .parse::<f64>()
                                .ok()
                                .filter(|value| value.is_finite() && *value > 0.0);
                            if recording {
                                if let Some(exact) = score_expression.ms_tempo(
                                    el,
                                    &format!("expression:tempo:{}", el.id().get()),
                                    super::score_intensity::source::time(pos, tpb)?,
                                    tempo_text,
                                    modern_expression,
                                )? {
                                    expression_tempo = exact;
                                }
                            }
                            let micros = quarters_per_second
                                .map(|value| (1_000_000.0 / value).round())
                                .filter(|value| (1.0..=f64::from(u32::MAX)).contains(value))
                                .map(|value| value as u32)
                                .ok_or_else(|| {
                                    format!("MuseScore tempo is invalid: {tempo_text:?}")
                                })?;
                            push_global_event(
                                &mut global_events,
                                checked_score_tick(pos)?,
                                Kind::Tempo(micros),
                            );
                        }
                        "Tuplet" => {
                            let normal_text = child_text(el, "normalNotes").ok_or_else(|| {
                                "MuseScore Tuplet is missing normalNotes".to_string()
                            })?;
                            let actual_text = child_text(el, "actualNotes").ok_or_else(|| {
                                "MuseScore Tuplet is missing actualNotes".to_string()
                            })?;
                            let normal = normal_text
                                .parse::<i64>()
                                .ok()
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore Tuplet normalNotes is invalid: {normal_text:?}"
                                    )
                                })?;
                            let actual = actual_text
                                .parse::<i64>()
                                .ok()
                                .filter(|value| (1..=64).contains(value))
                                .ok_or_else(|| {
                                    format!(
                                        "MuseScore Tuplet actualNotes is invalid: {actual_text:?}"
                                    )
                                })?;
                            tuplet = Some((normal, actual));
                        }
                        "endTuplet" => tuplet = None,
                        "location" => {
                            let fraction_text = child_text(el, "fractions").ok_or_else(|| {
                                "MuseScore location is missing fractions".to_string()
                            })?;
                            let (numerator, denominator) =
                                frac(fraction_text).ok_or_else(|| {
                                    format!(
                                        "MuseScore location fraction is invalid: {fraction_text:?}"
                                    )
                                })?;
                            let base = (
                                4i64.checked_mul(numerator).ok_or_else(|| {
                                    "MuseScore location numerator overflow".to_string()
                                })?,
                                denominator,
                            );
                            let ratio = checked_ratio_mul(
                                base,
                                time_stretch.0,
                                time_stretch.1,
                                "MuseScore stretched location",
                            )?;
                            let delta = exact_ticks(div, ratio, "MuseScore location")?;
                            pos = pos
                                .checked_add(delta)
                                .ok_or_else(|| "MuseScore cursor overflow".to_string())?;
                        }
                        "Chord" | "Rest" => {
                            let is_rest = el.has_tag_name("Rest");
                            let grace = !is_rest && is_grace(el);
                            let (mut dur, stretch_duration) = if grace {
                                (0, false)
                            } else {
                                let duration_type =
                                    child_text(el, "durationType").ok_or_else(|| {
                                        format!(
                                            "MuseScore {} is missing durationType",
                                            if is_rest { "Rest" } else { "Chord" }
                                        )
                                    })?;
                                let dots = match child_text(el, "dots") {
                                    Some(value) => value
                                        .parse::<u32>()
                                        .ok()
                                        .filter(|dots| *dots <= 4)
                                        .ok_or_else(|| {
                                            format!("MuseScore dots value is invalid: {value:?}")
                                        })?,
                                    None => 0,
                                };
                                if duration_type == "measure" {
                                    match child_text(el, "duration") {
                                        Some(value) => {
                                            let (numerator, denominator) =
                                                frac(value).ok_or_else(|| {
                                                    format!(
                                                        "MuseScore measure duration is invalid: {value:?}"
                                                    )
                                                })?;
                                            (
                                                exact_ticks(
                                                    div,
                                                    (
                                                        4i64.checked_mul(numerator).ok_or_else(
                                                            || {
                                                                "MuseScore measure duration numerator overflow"
                                                                    .to_string()
                                                            },
                                                        )?,
                                                        denominator,
                                                    ),
                                                    "MuseScore measure duration",
                                                )?,
                                                true,
                                            )
                                        }
                                        None => (this_len, false),
                                    }
                                } else {
                                    let ratio = dotted_ratio(
                                        duration_ratio(duration_type).ok_or_else(|| {
                                            format!(
                                                "MuseScore durationType is unsupported: {duration_type:?}"
                                            )
                                        })?,
                                        dots,
                                    )?;
                                    (exact_ticks(div, ratio, "MuseScore note duration")?, true)
                                }
                            };
                            if let Some((n, a)) = tuplet {
                                dur = exact_ticks(dur, (n, a), "MuseScore tuplet duration")?;
                            }
                            if stretch_duration {
                                dur = exact_ticks(
                                    dur,
                                    time_stretch,
                                    "MuseScore stretched note duration",
                                )?;
                            }
                            if !grace && dur <= 0 {
                                return Err(format!(
                                    "MuseScore {} has a non-positive duration",
                                    if is_rest { "Rest" } else { "Chord" }
                                ));
                            }
                            if !is_rest {
                                let on = checked_score_tick(pos)?;
                                let off = if grace {
                                    on
                                } else {
                                    checked_score_tick(pos.checked_add(dur).ok_or_else(|| {
                                        "MuseScore note timing overflow".to_string()
                                    })?)?
                                };
                                let chord_id = format!(
                                    "mscx:staff:{staff_id}:measure:{mi}:voice:{voice_index}:chord:{element_index}"
                                );
                                let lyrics =
                                    chord_lyrics(el, &chord_id, tick_scale, i64::from(tpb))?;
                                let (extensions, extension_issues) = chord_extensions(
                                    el,
                                    &lyrics,
                                    &chord_id,
                                    pass,
                                    playback_segment,
                                    on,
                                    div,
                                    &mut continuity_evidence,
                                )?;
                                let notes: Vec<_> = el
                                    .children()
                                    .filter(|child| child.has_tag_name("Note"))
                                    .collect();
                                let polyphonic = notes.len() > 1;
                                // Only a chord with no note at all has nothing
                                // to carry its word. Leaving a lyric standalone
                                // because no single pitch "owns" it deleted
                                // whole phrases from harmonised passages, and
                                // must never come back under either reading.
                                if notes.is_empty() {
                                    for lyric in &lyrics {
                                        push_event(
                                            &mut unassigned_chord_lyrics,
                                            on,
                                            Kind::Lyrics(lyric.clone()),
                                        );
                                    }
                                }
                                // Under the harmonised reading a chord's members
                                // are simultaneous voices of one line and each
                                // sings the word. Under the reduction reading
                                // they are an accompaniment under one singer:
                                // the highest note takes the word and joins the
                                // voice's own lane, so the sung line stays in
                                // one place instead of scattering across a lane
                                // per chord depth. Ties in pitch go to the
                                // first note in document order, so the choice is
                                // never dependent on iteration order.
                                // A chord that writes no word has nothing to give
                                // to one member, so it keeps the lanes it always
                                // had: the reading must not silently reshuffle
                                // purely instrumental chords, nor the ties that
                                // run through them.
                                let singing_member = match chord_reading {
                                    ChordReading::Harmonised => None,
                                    ChordReading::Reduction
                                        if !polyphonic || !chord_carries_a_word(el) =>
                                    {
                                        None
                                    }
                                    ChordReading::Reduction => notes
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(index, note)| {
                                            chord_note_pitch(*note).map(|pitch| (index, pitch))
                                        })
                                        .max_by_key(|(index, pitch)| {
                                            (*pitch, std::cmp::Reverse(*index))
                                        })
                                        .map(|(index, _)| index),
                                };
                                for (note_index, note) in notes.into_iter().enumerate() {
                                    let pitch_text = child_text(note, "pitch").ok_or_else(|| {
                                        format!(
                                            "MuseScore Note {note_index} in {chord_id} is missing pitch"
                                        )
                                    })?;
                                    let pitch = pitch_text
                                        .parse::<i64>()
                                        .ok()
                                        .and_then(|value| u8::try_from(value).ok())
                                        .filter(|value| *value <= 127)
                                        .ok_or_else(|| {
                                            format!(
                                                "MuseScore Note pitch is invalid: {pitch_text:?}"
                                            )
                                        })?;
                                    let source_id = format!("{chord_id}:note:{note_index}");
                                    if recording && child_text(note, "velocity").is_some() {
                                        score_expression.ms_note_owned(
                                            note,
                                            &source_id,
                                            modern_expression,
                                            &super::score_intensity::ScoreVoice {
                                                part: info.part_id.clone(),
                                                staff: staff_id.clone(),
                                                voice: (voice_index + 1).to_string(),
                                                instrument: None,
                                            },
                                            super::score_intensity::source::time(pos, tpb)?,
                                        )?;
                                    }
                                    let channel = info
                                        .instruments
                                        .first()
                                        .and_then(|instrument| instrument.channel);
                                    // The note the word goes to under a
                                    // reduction joins its voice's own lane, so
                                    // the sung line is continuous; every other
                                    // member keeps a lane of its own because
                                    // they sound together and one lane cannot
                                    // hold two simultaneous notes.
                                    let sings =
                                        singing_member.is_none_or(|singing| singing == note_index);
                                    let chord_member = match singing_member {
                                        Some(singing) if singing == note_index => None,
                                        _ => polyphonic.then_some(note_index),
                                    };
                                    let bucket = (voice_index, chord_member);
                                    let evidence =
                                        continuity_evidence.evidence(note, &source_id)?;
                                    continuity_evidence.charge(
                                        std::mem::size_of::<SourceContinuity>() + chord_id.len(),
                                    )?;
                                    continuity_evidence.extension_copies(
                                        if sings { &extensions } else { &[] },
                                        &extension_issues,
                                    )?;
                                    let mut issues = extension_issues.clone();
                                    let ties = match note_ties(note) {
                                        Ok(ties) => ties,
                                        Err(message) => {
                                            continuity_evidence.charge(
                                                message
                                                    .len()
                                                    .saturating_add(source_id.len())
                                                    .saturating_add(512),
                                            )?;
                                            issues.push(SourceContinuityIssue {
                                                code: "SOURCE_CONTINUITY_LINK_INVALID",
                                                message: format!(
                                                    "{source_id}, occurrence {pass}, segment \
                                                     {playback_segment}, interval {on}..{off}: {message}"
                                                ),
                                                evidence: evidence.clone(),
                                            });
                                            NoteTies::default()
                                        }
                                    };
                                    let pending_continuity = PendingContinuityTie {
                                        source: SourceNoteRef {
                                            source_id: source_id.clone(),
                                            occurrence: pass,
                                            playback_segment,
                                        },
                                        evidence: evidence.clone(),
                                        pitch,
                                        voice: voice_index,
                                        measure: mi,
                                        measure_tick: pos.checked_sub(measure_start).ok_or_else(
                                            || {
                                                "MuseScore tie measure position overflow"
                                                    .to_string()
                                            },
                                        )?,
                                        end_tick: off,
                                        next: ties.start,
                                    };
                                    // Resolve before lyric eligibility and independently of
                                    // whether the historical adapter merges this bare tail.
                                    let incoming_tie = (!grace)
                                        .then(|| {
                                            let key = tie_stop_key(&ties, voice_index, pitch)?;
                                            validated_incoming_tie(
                                                active_ties.get(&key)?,
                                                &ties,
                                                &pending_continuity,
                                                on,
                                                div,
                                            )
                                        })
                                        .flatten();
                                    if let Some(tie) = &incoming_tie {
                                        if let Some(key) = tie_stop_key(&ties, voice_index, pitch) {
                                            if outgoing_ties
                                                .get(&key)
                                                .is_some_and(|start| start.source == tie.head)
                                            {
                                                outgoing_ties.remove(&key);
                                            }
                                        }
                                    }
                                    if (ties.stop.is_some() || ties.legacy_stop.is_some())
                                        && incoming_tie.is_none()
                                    {
                                        let candidate = tie_stop_key(&ties, voice_index, pitch)
                                            .and_then(|key| active_ties.get(&key))
                                            .and_then(|pending| pending.continuity.as_ref())
                                            .map(|head| format!("{:?}", head.source))
                                            .unwrap_or_else(|| {
                                                "missing or ambiguous head".to_string()
                                            });
                                        issues.push(SourceContinuityIssue {
                                            code: "SOURCE_CONTINUITY_LINK_INVALID",
                                            message: format!(
                                                "Tie from {candidate} to {source_id}, occurrence \
                                                 {pass}, segment {playback_segment}, interval \
                                                 {on}..{off}, pitch {pitch}: source locations, \
                                                 exact contact or nominal pitch could not be validated"
                                            ),
                                            evidence: evidence.clone(),
                                        });
                                    }
                                    // Resolve a tie stop before borrowing this
                                    // note's bucket: the head's note-off can
                                    // live in another bucket of the same staff.
                                    // A tie tail carrying its own syllable is not
                                    // a plain sustain: the source asks for that
                                    // word to be sung, so the note must keep its
                                    // own attack. Merging it would delete a real
                                    // lyric from the projection. An explicitly
                                    // empty lyric is the opposite: MuseScore
                                    // writes one on a tied note precisely to say
                                    // that nothing is sung there.
                                    let stop_key = (!sings_a_word(&lyrics))
                                        .then(|| tie_stop_key(&ties, voice_index, pitch))
                                        .flatten();
                                    let continued = stop_key.and_then(|key| {
                                        let pending = active_ties.get(&key)?;
                                        // MuseScore's own back reference decides:
                                        // the chain must stay time-contiguous and
                                        // the previous link must sit exactly where
                                        // the location says it does.
                                        let notated_head = ties
                                            .stop
                                            .map(|stop| {
                                                i64::try_from(mi).ok().and_then(|tail| {
                                                    usize::try_from(tail + stop.measure_delta).ok()
                                                }) == Some(pending.measure_index)
                                            })
                                            .unwrap_or(true);
                                        if pending.end_tick != on || !notated_head {
                                            return None;
                                        }
                                        Some(key)
                                    });
                                    // A merged tail keeps its source identity and
                                    // loses only its played pitch, so the ledger
                                    // still sees every source note while
                                    // Synthesizer V sees one sustained note.
                                    let playback_pitch = if continued.is_some() {
                                        None
                                    } else {
                                        Some(pitch)
                                    };
                                    let head = continued.as_ref().and_then(|key| {
                                        let pending = active_ties.remove(key)?;
                                        let events = voice_events.get_mut(&pending.bucket)?;
                                        let target = events.get_mut(pending.off_index)?;
                                        if !recording {
                                            if let Kind::NoteOff(head) = &target.kind {
                                                if let Some(id) = &head.source_id {
                                                    score_expression.ties.insert(
                                                        (source_id.clone(), pass, on),
                                                        id.clone(),
                                                    );
                                                }
                                            }
                                        }
                                        target.tick = off;
                                        Some(pending)
                                    });
                                    let events = voice_events.entry(bucket).or_default();
                                    let on_index = events.len();
                                    push_event(
                                        events,
                                        on,
                                        Kind::NoteOn(NoteOn {
                                            channel,
                                            key: playback_pitch,
                                            velocity: None,
                                            source: NoteSource {
                                                id: source_id.clone(),
                                                part_id: Some(info.part_id.clone()),
                                                staff_id: Some(staff_id.clone()),
                                                voice: Some((voice_index + 1).to_string()),
                                                chord_id: Some(chord_id.clone()),
                                                instrument_id: info
                                                    .instruments
                                                    .first()
                                                    .and_then(|instrument| instrument.id.clone()),
                                                occurrence: pass,
                                                measure: u32::try_from(mi).ok(),
                                                grace,
                                                unpitched: None,
                                                continuity: Some(Arc::new(SourceContinuity {
                                                    evidence,
                                                    chord_id: chord_id.clone(),
                                                    playback_segment,
                                                    extensions: if sings {
                                                        extensions.clone()
                                                    } else {
                                                        Vec::new()
                                                    },
                                                    incoming_tie,
                                                    issues,
                                                })),
                                            },
                                            // MuseScore owns lyrics at Chord level, so which
                                            // note receives one is decided by the reading
                                            // above: every member under a harmonised part,
                                            // the highest note alone under a reduction.
                                            lyrics: if sings { lyrics.clone() } else { Vec::new() },
                                        }),
                                    );
                                    push_event(
                                        events,
                                        off,
                                        Kind::NoteOff(NoteOff {
                                            channel,
                                            key: playback_pitch,
                                            velocity: None,
                                            source_id: Some(source_id.clone()),
                                        }),
                                    );
                                    let off_index = events.len() - 1;
                                    if let Some(key) = tie_start_key(&ties, voice_index, pitch) {
                                        if let Some(previous) = outgoing_ties.remove(&key) {
                                            abandon_tie_start(
                                                previous, &mut voice_events,
                                                "another outgoing declaration overwrote the unresolved source link",
                                                &mut unresolved_tie_diagnostics,
                                            )?;
                                        }
                                        if outgoing_ties.len() >= MAX_UNRESOLVED_TIE_STARTS {
                                            return Err(format!(
                                                "SOURCE_CONTINUITY_LIMIT: MuseScore pending outgoing ties exceed \
                                                 {MAX_UNRESOLVED_TIE_STARTS}"
                                            ));
                                        }
                                        outgoing_ties.insert(
                                            key.clone(),
                                            OutgoingTie {
                                                bucket,
                                                on_index,
                                                source: pending_continuity.source.clone(),
                                                start_tick: on,
                                                end_tick: off,
                                            },
                                        );
                                        // Overlapping starts sharing a positional key cannot
                                        // select a head by source order. Keep the existing
                                        // merge path, but withhold new continuity proof.
                                        let ambiguous = active_ties
                                            .get(&key)
                                            .is_some_and(|pending| pending.end_tick > on);
                                        let continuity =
                                            (!grace && !ambiguous).then_some(pending_continuity);
                                        // A middle link keeps pointing at the
                                        // head's note-off, which carries the
                                        // whole chain's duration, but advances
                                        // the position the next link matches on.
                                        active_ties.insert(
                                            key,
                                            match head {
                                                Some(head) => PendingTie {
                                                    end_tick: off,
                                                    measure_index: mi,
                                                    continuity,
                                                    ..head
                                                },
                                                None => PendingTie {
                                                    bucket,
                                                    off_index,
                                                    end_tick: off,
                                                    measure_index: mi,
                                                    continuity,
                                                },
                                            },
                                        );
                                    }
                                }
                            }
                            if !grace {
                                pos = pos
                                    .checked_add(dur)
                                    .ok_or_else(|| "MuseScore cursor overflow".to_string())?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // irregular measure (anacrusis): len="a/b" attribute
            if let Some(value) = measure.attribute("len") {
                let (numerator, denominator) = frac(value).ok_or_else(|| {
                    format!("MuseScore measure len fraction is invalid: {value:?}")
                })?;
                this_len = exact_ticks(
                    div,
                    (
                        4i64.checked_mul(numerator).ok_or_else(|| {
                            "MuseScore measure len numerator overflow".to_string()
                        })?,
                        denominator,
                    ),
                    "MuseScore measure len",
                )?;
                if this_len <= 0 {
                    return Err("MuseScore measure len is non-positive".into());
                }
            }
            if recording {
                written_bounds.push((
                    super::score_intensity::source::time(measure_start, tpb)?,
                    super::score_intensity::source::time(
                        measure_start
                            .checked_add(this_len)
                            .ok_or("Expression measure overflow")?,
                        tpb,
                    )?,
                ));
                score_expression.end_written_measure(written_bounds[mi].1)?;
            } else {
                score_expression.record_measure_run(
                    written_memberships[mi],
                    &info.part_id,
                    &staff_id,
                    super::score_intensity::source::time(measure_start, tpb)?,
                    repeat_pass,
                    forward,
                )?;
            }
            measure_start = measure_start
                .checked_add(this_len)
                .ok_or_else(|| "MuseScore measure timeline overflow".to_string())?;
        }

        abandon_tie_starts(
            &mut outgoing_ties,
            &mut voice_events,
            "end of score abandoned the source declaration",
            &mut unresolved_tie_diagnostics,
        )?;
        for ((voice_index, chord_member), mut events) in voice_events {
            sort_and_reindex(&mut events);
            if !events
                .iter()
                .any(|event| matches!(event.kind, Kind::NoteOn(_)))
            {
                continue;
            }
            let mut track = Track {
                id: match chord_member {
                    Some(member) => format!(
                        "mscx:staff:{staff_id}:voice:{}:polyphonic-member:{}",
                        voice_index + 1,
                        member + 1
                    ),
                    None => format!("mscx:staff:{staff_id}:voice:{}", voice_index + 1),
                },
                name: if info.name.is_empty() {
                    match chord_member {
                        Some(member) => {
                            format!("Staff {staff_id} — polyphonic member {}", member + 1)
                        }
                        None => format!("Staff {staff_id}"),
                    }
                } else {
                    let mut name = info.name.clone();
                    if let Some(ordinal) = staff_ordinal {
                        name = format!("{name} — staff {ordinal}");
                    }
                    if let Some(member) = chord_member {
                        format!("{name} — polyphonic member {}", member + 1)
                    } else if voice_index == 0 {
                        name
                    } else {
                        format!("{name} — voice {}", voice_index + 1)
                    }
                },
                source: TrackSource {
                    source_track: tracks.len(),
                    part_id: Some(info.part_id.clone()),
                    staff_id: Some(staff_id.clone()),
                    voice: Some((voice_index + 1).to_string()),
                },
                role_hint: info.role,
                text_profile: MidiTextProfile::Generic,
                instruments: info.instruments.clone(),
                instrument: info.instruments.first().cloned(),
                chord_reading: chord_reading_decided
                    .then(|| (chord_reading, chord_reading_evidence.clone())),
                events,
            };
            if track
                .events
                .iter()
                .any(|event| matches!(&event.kind, Kind::NoteOn(note) if !note.lyrics.is_empty()))
            {
                track.role_hint = TrackRoleHint::Vocal;
            } else {
                // The reading is a statement about what is sung, so only a lane
                // that sings reports it. Repeating it on every silent member of
                // every chord would bury it.
                track.chord_reading = None;
            }
            tracks.push(track);
        }
        if !unassigned_chord_lyrics.is_empty() {
            sort_and_reindex(&mut unassigned_chord_lyrics);
            tracks.push(Track {
                id: format!("mscx:staff:{staff_id}:chord-lyrics"),
                name: if info.name.is_empty() {
                    format!("Staff {staff_id} — unassigned chord lyrics")
                } else {
                    format!("{} — unassigned chord lyrics", info.name)
                },
                source: TrackSource {
                    source_track: tracks.len(),
                    part_id: Some(info.part_id.clone()),
                    staff_id: Some(staff_id.clone()),
                    voice: None,
                },
                role_hint: TrackRoleHint::Ambiguous,
                text_profile: MidiTextProfile::Generic,
                instruments: Vec::new(),
                instrument: None,
                chord_reading: None,
                events: unassigned_chord_lyrics,
            });
        }
    }

    for event in local_meter_fallbacks {
        let Kind::TimeSig { num, den, .. } = event.kind else {
            unreachable!("local meter fallback must be a time signature");
        };
        push_meter_event(&mut global_events, event.tick, num, den)?;
    }

    if !global_events.is_empty() {
        if tracks.is_empty() {
            tracks.push(Track {
                id: "mscx:metadata".into(),
                name: "Score metadata".into(),
                source: TrackSource::default(),
                role_hint: TrackRoleHint::Ambiguous,
                text_profile: MidiTextProfile::Generic,
                instruments: Vec::new(),
                instrument: None,
                chord_reading: None,
                events: Vec::new(),
            });
        }
        tracks[0].events.extend(global_events);
        sort_and_reindex(&mut tracks[0].events);
    }
    score_expression.finish()?;
    let mut metadata_budget = super::score_intensity::ProvenanceBudget::default();
    metadata_budget.charge(tracks.len().saturating_mul(64), tracks.len())?;
    let occupied_voices: BTreeSet<_> = tracks
        .iter()
        .filter_map(|track| {
            Some((
                track.source.part_id.as_deref()?,
                track.source.staff_id.as_deref()?,
                track.source.voice.as_deref()?,
            ))
        })
        .collect();
    let mut intensity_metadata = Vec::new();
    for part in &declared_parts {
        for staff in &part.staves {
            let voices = staff
                .voices
                .iter()
                .map(|voice| voice.number.as_str())
                .chain(staff.voices.is_empty().then_some("1"));
            if !score_expression.has_missing_declaration_owner(
                &part.id,
                &staff.id,
                voices,
                &occupied_voices,
                &mut metadata_budget,
            )? {
                continue;
            }
            let info = staff_info.get(&staff.id);
            if let Some(info) = info {
                for instrument in &info.instruments {
                    let bytes = [&instrument.id, &instrument.sound_id, &instrument.name]
                        .into_iter()
                        .flatten()
                        .fold(std::mem::size_of::<InstrumentInfo>(), |n, s| {
                            n.saturating_add(s.len())
                        })
                        .saturating_add(instrument.controllers.len().saturating_mul(2));
                    metadata_budget.charge(bytes.saturating_mul(2), 1)?;
                }
            }
            metadata_budget.charge(
                part.id
                    .len()
                    .saturating_mul(3)
                    .saturating_add(staff.id.len().saturating_mul(2))
                    .saturating_add(part.name.len())
                    .saturating_add(512),
                1,
            )?;
            let instruments = info
                .map(|info| info.instruments.clone())
                .unwrap_or_default();
            intensity_metadata.push(Track {
                id: format!("mscx:staff:{}:intensity-metadata", staff.id),
                name: if part.name.is_empty() {
                    part.id.clone()
                } else {
                    part.name.clone()
                },
                source: TrackSource {
                    source_track: tracks.len() + intensity_metadata.len(),
                    part_id: Some(part.id.clone()),
                    staff_id: Some(staff.id.clone()),
                    voice: None,
                },
                role_hint: TrackRoleHint::Ambiguous,
                text_profile: MidiTextProfile::Generic,
                instrument: instruments.first().cloned(),
                instruments,
                chord_reading: None,
                events: Vec::new(),
            });
        }
    }
    tracks.extend(intensity_metadata);
    if tracks.is_empty() {
        return Err("no usable staff in the MuseScore file".into());
    }
    let topology = SourceTopology::from_declared_parts(declared_parts, &tracks);
    Ok(Midi {
        ticks_per_beat: tpb,
        time_base: TimeBase::PulsesPerQuarter(tpb),
        format: 1,
        source_format: SourceFormat::MuseScore,
        topology,
        score_intensity: (!score_expression.is_empty()).then_some(score_expression),
        staff_links,
        tracks,
    })
}

fn push_event(events: &mut Vec<Event>, tick: u32, kind: Kind) {
    let order = events.len() as u32;
    events.push(Event::new(tick, order, kind));
}

fn checked_score_tick(value: i64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "MuseScore tick exceeds the supported range".into())
}

fn push_meter_event(
    events: &mut Vec<Event>,
    tick: u32,
    numerator: u8,
    denominator: u16,
) -> Result<(), String> {
    if let Some((existing_num, existing_den)) = events.iter().find_map(|event| {
        if event.tick != tick {
            return None;
        }
        match event.kind {
            Kind::TimeSig { num, den, .. } => Some((num, den)),
            _ => None,
        }
    }) {
        let existing_duration = u32::from(existing_num) * u32::from(denominator);
        let candidate_duration = u32::from(numerator) * u32::from(existing_den);
        if existing_duration != candidate_duration {
            return Err(format!(
                "MuseScore time signatures at tick {tick} disagree about the global measure \
                 duration ({existing_num}/{existing_den} versus {numerator}/{denominator})"
            ));
        }
        // Equivalent local signatures (for example stretched 9/8 and global
        // 3/4) share one temporal meter in SVP. The original notation remains
        // preserved in the source score inside the bundle.
        return Ok(());
    }
    push_event(
        events,
        tick,
        Kind::TimeSig {
            num: numerator,
            den: denominator,
            clocks_per_click: None,
            notated_32nds: None,
        },
    );
    Ok(())
}

fn push_global_event(events: &mut Vec<Event>, tick: u32, kind: Kind) {
    let duplicate = events.iter().any(|event| {
        if event.tick != tick {
            return false;
        }
        match (&event.kind, &kind) {
            (Kind::Tempo(left), Kind::Tempo(right)) => left == right,
            (
                Kind::TimeSig {
                    num: left_num,
                    den: left_den,
                    ..
                },
                Kind::TimeSig {
                    num: right_num,
                    den: right_den,
                    ..
                },
            ) => left_num == right_num && left_den == right_den,
            _ => false,
        }
    });
    if !duplicate {
        push_event(events, tick, kind);
    }
}

fn sort_and_reindex(events: &mut [Event]) {
    events.sort_by_key(|event| (event.tick, event.order));
    for (order, event) in events.iter_mut().enumerate() {
        event.order = u32::try_from(order).unwrap_or(u32::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn mscx(lyric_text_xml: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Soprano</trackName>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Lyrics>
              {}
            </Lyrics>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#,
            lyric_text_xml
        )
    }

    fn zipped_score(bytes: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file("score.mscx", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn timeline_score(measures: &str) -> Vec<u8> {
        zipped_score(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02"><Score><Division>480</Division>
  <Part><Staff id="1"/><trackName>Soprano</trackName></Part>
  <Staff id="1">{measures}</Staff>
</Score></museScore>"#
            )
            .as_bytes(),
        )
    }

    #[test]
    fn extracted_part_repair_removes_only_a_proven_extra_measure() {
        let source = timeline_score(
            "<Measure><voice><Chord><durationType>whole</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure>\
             <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>62</pitch></Note></Chord></voice></Measure>",
        );
        let extracted = timeline_score(
            "<Measure><voice><Chord><durationType>whole</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure>\
             <Measure><voice><Rest><durationType>whole</durationType></Rest></voice></Measure>\
             <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>62</pitch></Note></Chord></voice></Measure>",
        );

        let repaired = repair_extracted_part_timeline(&source, &extracted, 0).unwrap();
        let xml = extract_mscz(&repaired).unwrap();
        let document = roxmltree::Document::parse(&xml).unwrap();
        let staff = document
            .descendants()
            .find(|node| {
                node.has_tag_name("Staff")
                    && node.attribute("id") == Some("1")
                    && node.children().any(|child| child.has_tag_name("Measure"))
            })
            .unwrap();
        assert_eq!(
            staff
                .children()
                .filter(|node| node.has_tag_name("Measure"))
                .count(),
            2
        );
    }

    #[test]
    fn extracted_part_repair_refuses_unprovable_measure_changes() {
        let source = timeline_score(
            "<Measure><voice><Chord><durationType>whole</durationType><Note><pitch>60</pitch></Note></Chord></voice></Measure>",
        );
        let extracted = timeline_score(
            "<Measure><voice><Chord><durationType>whole</durationType><Note><pitch>61</pitch></Note></Chord></voice></Measure>",
        );

        assert!(repair_extracted_part_timeline(&source, &extracted, 0).is_err());
    }

    fn zipped_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_linked_staff_is_a_view_of_its_staff_and_never_a_second_one() {
        // A tablature beside the notation is the same instrument playing the
        // same notes, and the score body writes a full copy of the measures
        // under it. Read as a staff of its own it doubled every note.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02"><Score><Division>480</Division>
<Part><trackName>Guitare</trackName>
  <Staff id="1"><StaffType group="pitched"><name>stdNormal</name></StaffType></Staff>
  <Staff id="2"><linkedTo>1</linkedTo><StaffType group="tab"><name>tab6StrCommon</name></StaffType></Staff>
</Part>
<Staff id="1"><Measure><voice>
  <Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord>
</voice></Measure></Staff>
<Staff id="2"><Measure><voice>
  <Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord>
</voice></Measure></Staff>
</Score></museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        assert_eq!(midi.topology.parts.len(), 1);
        assert_eq!(midi.topology.parts[0].staves.len(), 1);
        let sounded = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter(|event| matches!(&event.kind, Kind::NoteOn(note) if note.velocity != Some(0)))
            .count();
        assert_eq!(
            sounded, 1,
            "the tablature is the same note, not a second one"
        );
    }

    fn linked_score(declarations: &str, staves: &[(&str, &str)]) -> String {
        let bodies: String = staves
            .iter()
            .map(|(id, body)| format!(r#"<Staff id="{id}">{body}</Staff>"#))
            .collect();
        format!(
            "<museScore><Score><Division>480</Division>{declarations}{bodies}</Score></museScore>"
        )
    }

    const LINK_MUSIC: &str = "<Measure><voice><Chord><durationType>quarter</durationType><Lyrics><text>word</text></Lyrics><Note><pitch>60</pitch></Note></Chord></voice></Measure>";

    fn link_interpretations(midi: &Midi) -> Vec<(String, Option<String>)> {
        midi.staff_links
            .iter()
            .map(|link| (link.staff_id.clone(), link.canonical_staff_id.clone()))
            .collect()
    }

    #[test]
    fn linked_self_missing_and_empty_references_keep_actual_music() {
        for target in ["1", "99", ""] {
            let declaration =
                format!(r#"<Part><Staff id="1"><linkedTo>{target}</linkedTo></Staff></Part>"#);
            let xml = linked_score(&declaration, &[("1", LINK_MUSIC)]);
            for bytes in [xml.as_bytes().to_vec(), zipped_score(xml.as_bytes())] {
                let midi = parse(&bytes).unwrap();
                assert_eq!(midi.topology.staff_count(), 1);
                assert_eq!(played_notes(&midi), vec![(0, 480, 60)]);
                assert_eq!(link_interpretations(&midi), vec![("1".into(), None)]);
                let report = crate::engine::convert::convert_midi(&midi, "english");
                assert!(report
                    .source_warnings
                    .iter()
                    .any(|warning| warning.code == "MUSESCORE_STAFF_LINK_UNRESOLVED"));
            }
        }
    }

    #[test]
    fn linked_external_id_collision_never_crosses_parts() {
        let xml = linked_score(
            r#"<Part><trackName>Soprano</trackName><Staff id="1"/></Part>
            <Part><trackName>Piano</trackName><Instrument><instrumentId>keyboard.piano</instrumentId></Instrument><Staff id="2"><linkedTo>1</linkedTo></Staff></Part>"#,
            &[("1", LINK_MUSIC), ("2", LINK_MUSIC)],
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.topology.staff_count(), 2);
        assert_eq!(played_notes(&midi).len(), 2);
        let piano = midi
            .tracks
            .iter()
            .find(|track| track.source.staff_id.as_deref() == Some("2"))
            .unwrap();
        assert_eq!(piano.name, "Piano");
        assert_eq!(piano.source.part_id.as_deref(), Some("musescore-part-1"));
        assert_eq!(
            piano.instruments[0].sound_id.as_deref(),
            Some("keyboard.piano")
        );
        assert_eq!(link_interpretations(&midi), vec![("2".into(), None)]);
    }

    #[test]
    fn linked_cycles_and_unresolved_chains_keep_every_staff() {
        for last in ["1", "99"] {
            let declaration = format!(
                r#"<Part><Staff id="1"><linkedTo>2</linkedTo></Staff><Staff id="2"><linkedTo>{last}</linkedTo></Staff></Part>"#
            );
            let midi = parse_mscx(&linked_score(
                &declaration,
                &[("1", LINK_MUSIC), ("2", LINK_MUSIC)],
            ))
            .unwrap();
            assert_eq!(midi.topology.staff_count(), 2);
            assert_eq!(played_notes(&midi).len(), 2);
            assert!(link_interpretations(&midi)
                .iter()
                .all(|(_, target)| target.is_none()));
        }
    }

    #[test]
    fn linked_mixed_views_keep_external_music_and_canonical_identity() {
        // Put the view before its canonical target to rule out order-based guesses.
        let xml = linked_score(
            r#"<Part><Staff id="2"><linkedTo>2</linkedTo></Staff><Staff id="1"/><Staff id="3"><linkedTo>99</linkedTo></Staff></Part>"#,
            &[("2", LINK_MUSIC), ("1", LINK_MUSIC), ("3", LINK_MUSIC)],
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(
            midi.topology.parts[0]
                .staves
                .iter()
                .map(|staff| staff.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "3"]
        );
        assert_eq!(played_notes(&midi).len(), 2);
        assert_eq!(
            link_interpretations(&midi),
            vec![("2".into(), Some("1".into())), ("3".into(), None)]
        );
    }

    #[test]
    fn linked_divergent_rhythm_lyrics_voices_and_playback_controls_survive() {
        let declaration =
            r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff></Part>"#;
        let variants = [
            LINK_MUSIC.replace("quarter", "half"),
            LINK_MUSIC.replace("word", "different"),
            LINK_MUSIC.replace("<voice>", "<voice/><voice>"),
            LINK_MUSIC.replace("<pitch>60</pitch>", "<pitch>62</pitch>"),
            LINK_MUSIC.replace("<Note>", "<Note><velocity>50</velocity>"),
            LINK_MUSIC.replace("<voice>", "<voice><Tempo><tempo>1.5</tempo></Tempo>"),
            LINK_MUSIC.replace(
                "<Measure>",
                "<Measure><startRepeat/><endRepeat>2</endRepeat>",
            ),
            LINK_MUSIC.replace("<text>word</text>", "<text>word</text><ticks>480</ticks>"),
            LINK_MUSIC.replace(
                "<Note>",
                "<Note><unknownPlaybackControl>1</unknownPlaybackControl>",
            ),
        ];
        for distinct in variants {
            let midi = parse_mscx(&linked_score(
                declaration,
                &[("1", LINK_MUSIC), ("2", &distinct)],
            ))
            .unwrap();
            assert_eq!(midi.topology.staff_count(), 2, "{distinct}");
            let control = parse_mscx(&linked_score(
                &declaration.replace("<linkedTo>1</linkedTo>", ""),
                &[("1", LINK_MUSIC), ("2", &distinct)],
            ))
            .unwrap();
            assert_eq!(
                midi.tracks, control.tracks,
                "all music, IDs and event order: {distinct}"
            );
            assert_eq!(midi.topology, control.topology);
            assert!(played_notes(&midi).len() >= 2);
            assert_eq!(
                link_interpretations(&midi),
                vec![("2".into(), None)],
                "{distinct}"
            );
        }
    }

    #[test]
    fn linked_equivalent_views_ignore_only_proven_benign_formatting() {
        let declaration = r#"<Part><Staff id="1"><StaffType group="pitched"/></Staff><Staff id="2"><linkedTo>1</linkedTo><StaffType group="tab"/></Staff></Part>"#;
        let notation = LINK_MUSIC.to_string();
        let tab = LINK_MUSIC
            .replace(
                "<Chord>",
                "<Chord><stemDirection>down</stemDirection><offset x=\"1\" y=\"2\"/>",
            )
            .replace("<Note>", "<Note><fret>3</fret><string>2</string>")
            .replace("<text>word</text>", "<text><font size=\"12\"/>word</text>");
        let xml = linked_score(declaration, &[("1", &notation), ("2", &tab)]);
        for bytes in [xml.as_bytes().to_vec(), zipped_score(xml.as_bytes())] {
            let midi = parse(&bytes).unwrap();
            assert_eq!(midi.topology.staff_count(), 1);
            assert_eq!(played_notes(&midi), vec![(0, 480, 60)]);
            assert_eq!(
                link_interpretations(&midi),
                vec![("2".into(), Some("1".into()))]
            );
        }
    }

    #[test]
    fn linked_anonymous_declaration_excludes_its_resolved_body_once() {
        let xml = linked_score(
            r#"<Part><Staff id="1"/><Staff><linkedTo>1</linkedTo></Staff></Part>"#,
            &[("1", LINK_MUSIC), ("7", LINK_MUSIC)],
        );
        for bytes in [xml.as_bytes().to_vec(), zipped_score(xml.as_bytes())] {
            let midi = parse(&bytes).unwrap();
            assert_eq!(played_notes(&midi), vec![(0, 480, 60)]);
            assert_eq!(midi.topology.staff_count(), 1);
            assert_eq!(midi.topology.parts[0].staves[0].id, "1");
            assert_eq!(
                link_interpretations(&midi),
                vec![("7".into(), Some("1".into()))]
            );
            assert_eq!(midi.tracks.len(), 1);
        }
    }

    #[test]
    fn linked_to_uses_declaration_index_with_nontrivial_ids_and_body_order() {
        let declaration = r#"<Part><Staff id="17"/></Part><Part><Staff id="42"/><Staff id="9"><linkedTo>2</linkedTo></Staff></Part><Part><Staff id="2"/></Part>"#;
        let other = LINK_MUSIC.replace("60", "65");
        let midi = parse_mscx(&linked_score(
            declaration,
            &[
                ("9", LINK_MUSIC),
                ("17", LINK_MUSIC),
                ("42", LINK_MUSIC),
                ("2", &other),
            ],
        ))
        .unwrap();
        assert_eq!(
            link_interpretations(&midi),
            vec![("9".into(), Some("42".into()))]
        );
        assert_eq!(midi.topology.parts[1].staves[0].id, "42");
        assert_eq!(
            played_notes(&midi),
            vec![(0, 480, 60), (0, 480, 60), (0, 480, 65)]
        );
        let self_link = linked_score(
            r#"<Part><Staff id="42"><linkedTo>1</linkedTo></Staff></Part>"#,
            &[("42", LINK_MUSIC)],
        );
        let midi = parse_mscx(&self_link).unwrap();
        assert_eq!(link_interpretations(&midi), vec![("42".into(), None)]);
        assert_eq!(played_notes(&midi), vec![(0, 480, 60)]);
    }

    #[test]
    fn linked_unknown_markup_namespaces_and_object_ids_never_authorize_deletion() {
        let declaration =
            r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff></Part>"#;
        let variants = [
            LINK_MUSIC.replace(
                "<Note>",
                "<Note><x:velocity xmlns:x=\"urn:playback\">50</x:velocity>",
            ),
            LINK_MUSIC.replace(
                "<Note>",
                "<Note><x:offset xmlns:x=\"urn:playback\" x=\"1\"/>",
            ),
            LINK_MUSIC.replace("<Note>", "<Note id=\"possibly-referenced\">"),
            LINK_MUSIC.replace("<Chord>", "<Chord id=\"possibly-referenced\">"),
            LINK_MUSIC.replace("<Measure>", "<Measure id=\"possibly-referenced\">"),
            LINK_MUSIC.replace("<text>word</text>", "<text><unknown>word</unknown></text>"),
            LINK_MUSIC.replace(
                "<text>word</text>",
                "<text><b playback=\"false\">word</b></text>",
            ),
            LINK_MUSIC.replace(
                "<text>word</text>",
                "<text><font size=\"12\" unknown=\"1\"/>word</text>",
            ),
            LINK_MUSIC.replace(
                "<text>word</text>",
                "<text><x:b xmlns:x=\"urn:semantic\">word</x:b></text>",
            ),
            LINK_MUSIC.replace("<text>word</text>", "<text><b>word </b></text>"),
            LINK_MUSIC.replace("<Note>", "<Note><offset x=\"1\"><play>0</play></offset>"),
            LINK_MUSIC.replace("<Note>", "<Note><string semantic=\"different\">2</string>"),
            LINK_MUSIC.replace(
                "<Note>",
                "<Note><pos xmlns:x=\"urn:semantic\" x:reference=\"2\"/>",
            ),
        ];
        for distinct in variants {
            let xml = linked_score(declaration, &[("1", LINK_MUSIC), ("2", &distinct)]);
            let midi = parse_mscx(&xml).unwrap();
            let control = parse_mscx(&xml.replace("<linkedTo>1</linkedTo>", "")).unwrap();
            assert_eq!(midi.tracks, control.tracks, "{distinct}");
            assert_eq!(midi.topology, control.topology, "{distinct}");
            assert_eq!(
                played_notes(&midi),
                vec![(0, 480, 60), (0, 480, 60)],
                "{distinct}"
            );
            assert_eq!(link_interpretations(&midi), vec![("2".into(), None)]);
        }
        // Even equal opaque relationships do not prove that IDs can be erased.
        let opaque = LINK_MUSIC.replace(
            "<Note>",
            "<Note id=\"shared\"><reference>shared</reference>",
        );
        let midi = parse_mscx(&linked_score(
            declaration,
            &[("1", &opaque), ("2", &opaque)],
        ))
        .unwrap();
        assert_eq!(played_notes(&midi).len(), 2);
        assert_eq!(link_interpretations(&midi), vec![("2".into(), None)]);
    }

    #[test]
    fn linked_visual_subtrees_validate_every_attribute_and_child() {
        for staff_type in [
            r#"<StaffType group="tab"><name semantic="different">tab6</name></StaffType>"#,
            r#"<StaffType group="tab"><name><unknown/></name></StaffType>"#,
            r#"<StaffType group="tab"><lines xmlns:x="urn:semantic" x:ref="1">6</lines></StaffType>"#,
            r#"<x:StaffType xmlns:x="urn:semantic" group="tab"/>"#,
            r#"<defaultClef><unknown>G</unknown></defaultClef>"#,
            r#"<bracket type="1" span="2"><unknown/></bracket>"#,
        ] {
            let declaration = format!(
                r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo>{staff_type}</Staff></Part>"#
            );
            let midi = parse_mscx(&linked_score(
                &declaration,
                &[("1", LINK_MUSIC), ("2", LINK_MUSIC)],
            ))
            .unwrap();
            assert_eq!(
                played_notes(&midi),
                vec![(0, 480, 60), (0, 480, 60)],
                "{staff_type}"
            );
            assert_eq!(link_interpretations(&midi), vec![("2".into(), None)]);
        }
    }

    #[test]
    fn linked_duplicate_declarations_and_bodies_fail_explicitly() {
        let declaration =
            r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff></Part>"#;
        for (declaration, bodies, error) in [
            (
                declaration.replace("<Staff id=\"1\"/>", "<Staff id=\"1\"/><Staff id=\"1\"/>"),
                vec![("1", LINK_MUSIC), ("2", LINK_MUSIC)],
                "duplicate resolved staff declaration ID",
            ),
            (
                declaration.into(),
                vec![("1", LINK_MUSIC), ("1", LINK_MUSIC), ("2", LINK_MUSIC)],
                "duplicate score-body staff ID",
            ),
        ] {
            let error_message = parse_mscx(&linked_score(&declaration, &bodies)).unwrap_err();
            assert!(error_message.contains(error), "{error_message}");
        }
    }

    #[test]
    fn linked_multiple_links_and_chain_to_unlinked_target_keep_unresolved_music() {
        let declaration = r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo><linkedTo>3</linkedTo></Staff><Staff id="3"/></Part>"#;
        let midi = parse_mscx(&linked_score(
            declaration,
            &[("1", LINK_MUSIC), ("2", LINK_MUSIC), ("3", LINK_MUSIC)],
        ))
        .unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 60); 3]);
        assert_eq!(midi.staff_links.len(), 2);
        assert!(midi
            .staff_links
            .iter()
            .all(|link| link.canonical_staff_id.is_none()));
        assert_ne!(midi.staff_links[0].id, midi.staff_links[1].id);
        let declaration = r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff><Staff id="3"><linkedTo>2</linkedTo></Staff></Part>"#;
        let midi = parse_mscx(&linked_score(
            declaration,
            &[("1", LINK_MUSIC), ("2", LINK_MUSIC), ("3", LINK_MUSIC)],
        ))
        .unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 60); 2]);
        assert_eq!(
            link_interpretations(&midi),
            vec![("2".into(), Some("1".into())), ("3".into(), None)]
        );
        assert_eq!(
            midi.topology.parts[0]
                .staves
                .iter()
                .map(|staff| staff.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "3"]
        );
    }

    #[test]
    fn linked_unmatched_and_missing_id_metadata_is_source_level_and_truthful() {
        for (declaration,bodies,expected_id,has_measures) in [
            (r#"<Part><Staff><linkedTo>99</linkedTo></Staff></Part>"#,vec![("7",LINK_MUSIC)],"7",true),
            (r#"<Part><Staff id="1"/></Part><Part><Staff id="9"><linkedTo>99</linkedTo></Staff></Part>"#,vec![("1",LINK_MUSIC)],"9",false),
            (r#"<Part><Staff id="1"/></Part><Part><Staff id="9"><linkedTo>99</linkedTo></Staff></Part>"#,vec![("1",LINK_MUSIC),("9","<Measure><voice><Rest><durationType>quarter</durationType></Rest></voice></Measure>")],"9",true),
        ] {
            let xml=linked_score(declaration,&bodies);
            let midi=parse_mscx(&xml).unwrap();
            let control=parse_mscx(&xml.replace("<linkedTo>99</linkedTo>","")).unwrap();
            assert_eq!(midi.tracks,control.tracks);
            assert_eq!(midi.topology,control.topology);
            assert_eq!(midi.staff_links[0].staff_id,expected_id);
            assert_eq!(midi.staff_links[0].has_body_measures,has_measures);
            let outcome=crate::engine::convert::convert_midi(&midi,"english");
            assert!(outcome.ok);
            assert_eq!(outcome.source_warnings.len(),1);
            assert_eq!(outcome.source_warnings[0].source_id,Some(format!("mscx:staff:{expected_id}")));
            assert_eq!(outcome.source_warnings[0].message.contains("no score-body measures were found"),!has_measures);
            assert!(outcome.tracks.iter().flat_map(|track|&track.warnings).all(|warning|!warning.code.starts_with("MUSESCORE_STAFF_LINK")));
        }
    }

    #[test]
    fn linked_target_without_body_or_only_in_embedded_score_is_unresolved() {
        let declaration =
            r#"<Part><Staff id="1"/><Staff id="2"><linkedTo>1</linkedTo></Staff></Part>"#;
        let xml = linked_score(declaration, &[("2", LINK_MUSIC)]);
        for xml in [
            xml.clone(),
            xml.replace(
                "</Score>",
                &format!(r#"<Score><Staff id="1">{LINK_MUSIC}</Staff></Score></Score>"#),
            ),
        ] {
            let midi = parse_mscx(&xml).unwrap();
            assert_eq!(played_notes(&midi), vec![(0, 480, 60)]);
            assert_eq!(link_interpretations(&midi), vec![("2".into(), None)]);
        }
    }

    #[test]
    fn a_container_names_the_source_parts_it_holds() {
        let two = zipped_score(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="4.70"><Score><Division>480</Division>
<Part id="2"><trackName>Bass</trackName></Part>
<Part id="3"><trackName>Drums</trackName></Part>
</Score></museScore>"#,
        );
        assert_eq!(
            container_part_ids(&two),
            Some(vec![Some("2".into()), Some("3".into())])
        );

        // MuseScore 3 writes Parts without an id, so a container written that
        // way names nothing and cannot place itself on the source.
        let unnamed = zipped_score(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02"><Score><Division>480</Division>
<Part><trackName>Bass</trackName></Part>
</Score></museScore>"#,
        );
        assert_eq!(container_part_ids(&unnamed), Some(vec![None]));

        assert_eq!(container_part_ids(b"not a score"), None);
    }

    fn container(paths: &[&str]) -> String {
        let roots = paths
            .iter()
            .map(|path| format!(r#"<rootfile full-path="{path}"/>"#))
            .collect::<Vec<_>>()
            .join("");
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>{roots}</rootfiles>
</container>"#
        )
    }

    fn latin1(value: &str) -> Vec<u8> {
        value
            .chars()
            .map(|character| {
                u8::try_from(u32::from(character))
                    .unwrap_or_else(|_| panic!("test encoder does not cover {character:?}"))
            })
            .collect()
    }

    fn lyrics_of(midi: &Midi) -> Vec<String> {
        midi.tracks
            .iter()
            .flat_map(|track| track.events.iter())
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.iter()),
                _ => None,
            })
            .flatten()
            .filter_map(|lyric| match &lyric.state {
                LyricState::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plain_lyric_text() {
        let midi = parse_mscx(&mscx("<text>let</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn musescore_two_direct_measure_elements_are_one_implicit_voice() {
        let legacy = mscx("<text>let</text>")
            .replace(r#"version="3.02""#, r#"version="2.06""#)
            .replace("        <voice>\n", "")
            .replace("        </voice>\n", "");
        let midi = parse_mscx(&legacy).expect("MuseScore 2 direct measure elements must parse");
        assert_eq!(midi.topology.part_count(), 1);
        assert_eq!(midi.topology.voice_count(), 1);
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn musescore_two_direct_measure_tuplet_expands_the_exact_tick_scale() {
        let legacy = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="2.06">
  <Score>
    <Division>480</Division>
    <Part><trackName>Soprano</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure>
        <Tuplet id="1"><normalNotes>1</normalNotes><actualNotes>7</actualNotes></Tuplet>
        <Chord>
          <Tuplet>1</Tuplet>
          <durationType>quarter</durationType>
          <Note><pitch>60</pitch></Note>
        </Chord>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(legacy).expect("legacy implicit-voice tuplet must be exact");
        assert_eq!(midi.ticks_per_beat, 3_360);
        let ticks = midi.tracks[0]
            .events
            .iter()
            .map(|event| event.tick)
            .collect::<Vec<_>>();
        assert_eq!(ticks, vec![0, 480]);
    }

    #[test]
    fn mixed_direct_and_explicit_voice_encodings_are_rejected() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure>
        <Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord>
        <voice>
          <Chord><durationType>quarter</durationType><Note><pitch>62</pitch></Note></Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let error = parse_mscx(xml).expect_err("mixed encodings have ambiguous ownership");
        assert!(error.contains("mixes direct legacy events"));
    }

    #[test]
    fn local_time_signature_stretch_scales_meter_locations_and_notes_exactly() {
        let legacy = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="2.06">
  <Score>
    <Division>480</Division>
    <Part><trackName>Soprano</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure>
        <TimeSig>
          <sigN>9</sigN><sigD>8</sigD><stretchN>3</stretchN><stretchD>2</stretchD>
        </TimeSig>
        <location><fractions>1/8</fractions></location>
        <Chord><durationType>eighth</durationType><Note><pitch>60</pitch></Note></Chord>
      </Measure>
      <Measure>
        <Chord><durationType>eighth</durationType><Note><pitch>62</pitch></Note></Chord>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(legacy).expect("local time stretch must remain exact");
        assert_eq!(midi.ticks_per_beat, 480);
        let note_ticks = midi.tracks[0]
            .events
            .iter()
            .filter(|event| matches!(event.kind, Kind::NoteOn(_) | Kind::NoteOff(_)))
            .map(|event| event.tick)
            .collect::<Vec<_>>();
        assert_eq!(note_ticks, vec![160, 320, 1_440, 1_600]);

        let meters = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| match event.kind {
                Kind::TimeSig { num, den, .. } => Some((event.tick, num, den)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(meters, vec![(0, 3, 4)]);
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        let project =
            crate::engine::target::svp::serialize(outcome.svp.as_ref().expect("SVP project"))
                .expect("exactly representable");
        let meter = &project.time.meter;
        assert_eq!(
            meter
                .iter()
                .map(|meter| (meter.index, meter.numerator, meter.denominator))
                .collect::<Vec<_>>(),
            vec![(0, 3, 4)]
        );
    }

    #[test]
    fn equivalent_local_and_global_meters_merge_but_conflicts_fail() {
        let mut events = Vec::new();
        push_meter_event(&mut events, 0, 3, 4).unwrap();
        push_meter_event(&mut events, 0, 6, 8).unwrap();
        assert_eq!(events.len(), 1);
        let error = push_meter_event(&mut events, 0, 5, 8)
            .expect_err("different temporal measure lengths must conflict");
        assert!(error.contains("disagree"), "unexpected error: {error}");
    }

    #[test]
    fn raw_and_zipped_latin1_follow_the_xml_declaration() {
        let xml = mscx("<text>café</text>").replace("UTF-8", "ISO-8859-1");
        let bytes = latin1(&xml);
        let raw = parse(&bytes).expect("raw MSCX uses its declared encoding");
        assert_eq!(lyrics_of(&raw), vec!["café"]);

        let archive = zipped_score(&bytes);
        let zipped = parse(&archive).expect("zipped MSCX uses the entry declaration");
        assert_eq!(lyrics_of(&zipped), vec!["café"]);
    }

    #[test]
    fn mscz_uses_container_master_even_when_an_excerpt_is_first() {
        let excerpt = mscx("<text>excerpt</text>");
        let master = mscx("<text>master</text>");
        let manifest = container(&["Excerpts/Soprano.mscx", "Master Score.mscx"]);
        let archive = zipped_entries(&[
            ("Excerpts/Soprano.mscx", excerpt.as_bytes()),
            ("META-INF/container.xml", manifest.as_bytes()),
            ("Master Score.mscx", master.as_bytes()),
        ]);

        let midi = parse(&archive).expect("the declared top-level master must be selected");
        assert_eq!(lyrics_of(&midi), vec!["master"]);
    }

    #[test]
    fn mscz_rejects_ambiguous_declared_masters() {
        let first = mscx("<text>first</text>");
        let second = mscx("<text>second</text>");
        let manifest = container(&["First.mscx", "Second.mscx"]);
        let archive = zipped_entries(&[
            ("First.mscx", first.as_bytes()),
            ("Second.mscx", second.as_bytes()),
            ("META-INF/container.xml", manifest.as_bytes()),
        ]);

        let error = parse(&archive).expect_err("two declared top-level masters are ambiguous");
        assert!(error.contains("ambiguous"), "unexpected error: {error}");
    }

    #[test]
    fn mscz_never_promotes_an_excerpt_when_no_master_is_declared() {
        let excerpt = mscx("<text>excerpt</text>");
        let manifest = container(&["Excerpts/Soprano.mscx"]);
        let archive = zipped_entries(&[
            ("Excerpts/Soprano.mscx", excerpt.as_bytes()),
            ("META-INF/container.xml", manifest.as_bytes()),
        ]);

        let error = parse(&archive).expect_err("an excerpt must not silently become the master");
        assert!(error.contains("only Excerpts"), "unexpected error: {error}");
    }

    #[test]
    fn mscz_rejects_a_missing_declared_root() {
        let excerpt = mscx("<text>excerpt</text>");
        let manifest = container(&["Missing.mscx"]);
        let archive = zipped_entries(&[
            ("Excerpts/Soprano.mscx", excerpt.as_bytes()),
            ("META-INF/container.xml", manifest.as_bytes()),
        ]);

        let error = parse(&archive).expect_err("a declared root must exist in the archive");
        assert!(error.contains("missing"), "unexpected error: {error}");
    }

    #[test]
    fn mscz_rejects_traversal_and_absolute_declared_roots() {
        let master = mscx("<text>master</text>");
        for unsafe_path in [
            "../Master.mscx",
            "/Master.mscx",
            "C:/Master.mscx",
            r"C:\Master.mscx",
        ] {
            let manifest = container(&[unsafe_path]);
            let archive = zipped_entries(&[
                ("Master.mscx", master.as_bytes()),
                ("META-INF/container.xml", manifest.as_bytes()),
            ]);

            let error = parse(&archive).expect_err("unsafe roots must not be resolved");
            assert!(error.contains("unsafe"), "unexpected error: {error}");
        }
    }

    #[test]
    fn mscz_without_container_accepts_one_unique_nested_score() {
        let only = mscx("<text>only</text>");
        let archive = zipped_entries(&[("Scores/Only.mscx", only.as_bytes())]);

        let midi = parse(&archive).expect("one unique MSCX is an unambiguous fallback");
        assert_eq!(lyrics_of(&midi), vec!["only"]);
    }

    #[test]
    fn mscz_without_container_prefers_one_top_level_score_over_excerpts() {
        let excerpt = mscx("<text>excerpt</text>");
        let master = mscx("<text>master</text>");
        let archive = zipped_entries(&[
            ("Excerpts/Soprano.mscx", excerpt.as_bytes()),
            ("Master.mscx", master.as_bytes()),
        ]);

        let midi = parse(&archive).expect("the sole top-level MSCX is the safe fallback");
        assert_eq!(lyrics_of(&midi), vec!["master"]);
    }

    #[test]
    fn mscz_without_container_rejects_multiple_nested_scores() {
        let first = mscx("<text>first</text>");
        let second = mscx("<text>second</text>");
        let archive = zipped_entries(&[
            ("Scores/First.mscx", first.as_bytes()),
            ("Scores/Second.mscx", second.as_bytes()),
        ]);

        let error = parse(&archive).expect_err("multiple fallback roots are ambiguous");
        assert!(error.contains("ambiguous"), "unexpected error: {error}");
    }

    #[test]
    fn musescore_detection_scans_past_long_prologs() {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!--{}-->\n{}",
            "padding".repeat(200),
            mscx("<text>let</text>")
                .split_once("<museScore")
                .map(|(_, tail)| format!("<museScore{tail}"))
                .unwrap()
        );
        assert!(is_musescore_xml(xml.as_bytes()));
        assert!(crate::engine::musicxml::looks_like_xml(xml.as_bytes()));
        let midi = parse(xml.as_bytes()).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn fractional_tick_durations_raise_the_exact_timebase_instead_of_truncating() {
        let xml = mscx("<text>tiny</text>").replace(
            "<durationType>quarter</durationType>",
            "<durationType>256th</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 960);
        let ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| {
                matches!(event.kind, Kind::NoteOn(_) | Kind::NoteOff(_)).then_some(event.tick)
            })
            .collect();
        assert_eq!(ticks, vec![0, 15]);
    }

    #[test]
    fn exact_tuplet_scale_is_computed_before_events_are_emitted() {
        let xml = mscx("<text>seven</text>").replace(
            "<Chord>\n            <durationType>quarter</durationType>",
            "<Tuplet><normalNotes>1</normalNotes><actualNotes>7</actualNotes></Tuplet>\
             <Chord>\n            <durationType>quarter</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 3_360);
        let off = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| matches!(event.kind, Kind::NoteOff(_)).then_some(event.tick))
            .unwrap();
        assert_eq!(off, 480);
    }

    #[test]
    fn tuplet_scaling_keeps_the_base_duration_exact_too() {
        let xml = mscx("<text>exact</text>")
            .replace("<Division>480</Division>", "<Division>3</Division>")
            .replace(
                "<Chord>\n            <durationType>quarter</durationType>",
                "<Tuplet><normalNotes>2</normalNotes><actualNotes>3</actualNotes></Tuplet>\
                 <Chord>\n            <durationType>eighth</durationType>",
            );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(midi.ticks_per_beat, 6);
        let off = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| matches!(event.kind, Kind::NoteOff(_)).then_some(event.tick))
            .unwrap();
        assert_eq!(off, 2);
    }

    #[test]
    fn unrepresentable_fraction_is_rejected_instead_of_truncated() {
        let xml = mscx("<text>let</text>").replace(
            "<Chord>\n            <durationType>quarter</durationType>",
            "<location><fractions>1/999983</fractions></location>\
             <Chord>\n            <durationType>quarter</durationType>",
        );
        let error = parse_mscx(&xml).expect_err("oversized exact PPQ must be explicit");
        assert!(
            error.contains("exact range") || error.contains("tick division"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_chord_sings_its_word_in_every_voice() {
        // The members of a chord are simultaneous voices of one line. Reading
        // them as candidates to choose between meant no voice sang the word at
        // all, which deleted whole harmonised phrases from the projection.
        let xml = mscx("<text>together</text>").replace(
            "<Note><pitch>60</pitch></Note>",
            "<Note><pitch>60</pitch></Note><Note><pitch>64</pitch></Note>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let attached_count = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.len()),
                _ => None,
            })
            .sum::<usize>();
        let standalone_count = midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter(|event| matches!(event.kind, Kind::Lyrics(_)))
            .count();
        assert_eq!(attached_count, 2, "both voices carry the word");
        assert_eq!(standalone_count, 0, "nothing is left source-only");
        assert_eq!(midi.topology.part_count(), 1);
        assert_eq!(midi.topology.staff_count(), 1);
        assert_eq!(midi.topology.voice_count(), 1);
        assert_eq!(
            midi.topology.projection_lane_count(),
            2,
            "one monophonic lane per chord member"
        );

        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert!(outcome.ok, "{:?}", outcome.msg);
        assert_eq!(outcome.topology, midi.topology);
        assert_eq!(outcome.placed, 2);
        let sung: Vec<_> =
            crate::engine::target::svp::serialize(outcome.svp.as_ref().expect("valid SVP"))
                .expect("exactly representable")
                .tracks
                .iter()
                .flat_map(|track| track.main_group.notes.iter())
                .map(|note| (note.pitch, note.lyrics.clone()))
                .collect();
        assert_eq!(
            sung,
            vec![(60, "together".into()), (64, "together".into())],
            "each voice keeps its own pitch and the shared word"
        );
    }

    #[test]
    fn lyric_text_with_leading_font_elements() {
        // MuseScore stores styled lyrics as <font .../> elements inside <text>;
        // the syllable is a text node placed after them.
        let midi = parse_mscx(&mscx(
            r#"<text><font size="9.2"></font><font face="Arial"></font>let</text>"#,
        ))
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn lyric_text_interleaved_with_formatting() {
        let midi = parse_mscx(&mscx(r#"<text>shi<font face="Arial"></font>ne,</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["shine,"]);
    }

    #[test]
    fn empty_formatted_lyric_is_preserved_as_explicit_empty() {
        let midi = parse_mscx(&mscx(r#"<text><font size="9.2"></font></text>"#)).unwrap();
        assert!(lyrics_of(&midi).is_empty());
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .expect("the empty source lyric remains attached");
        assert_eq!(lyric.state, LyricState::ExplicitEmpty);
    }

    #[test]
    fn sym_glyph_name_is_not_injected() {
        // <sym> holds a SMuFL glyph identifier, not renderable lyric text.
        let midi = parse_mscx(&mscx(r#"<text>a<sym>space</sym>b</text>"#)).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["ab"]);
    }

    #[test]
    fn pretty_printed_text_is_trimmed() {
        let midi = parse_mscx(&mscx(
            "<text>\n  <font size=\"9.2\"></font>\n  let\n</text>",
        ))
        .unwrap();
        assert_eq!(lyrics_of(&midi), vec!["let"]);
    }

    #[test]
    fn xml_entities_are_decoded() {
        let midi = parse_mscx(&mscx("<text>rock &amp; roll</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["rock & roll"]);
    }

    #[test]
    fn every_styling_wrapper_yields_its_text() {
        // Any combination of style tags, nesting, sizes and faces must never
        // hide the syllable.
        for (xml, want) in [
            (r#"<text><b>bold</b></text>"#, "bold"),
            (r#"<text><i>ital</i></text>"#, "ital"),
            (r#"<text><u>under</u></text>"#, "under"),
            (r#"<text><s>strike</s></text>"#, "strike"),
            (r#"<text><b><i><u>all</u></i></b></text>"#, "all"),
            (
                r#"<text><font face="Comic Sans MS"></font><b>mix</b>ed</text>"#,
                "mixed",
            ),
            (
                r#"<text><font size="24"></font><font size="6"></font>tiny</text>"#,
                "tiny",
            ),
            (r#"<text>x<sup>2</sup></text>"#, "x2"),
            (r#"<text>H<sub>2</sub>O</text>"#, "H2O"),
            (
                r#"<text><font face="Arial"><b>deep</b></font></text>"#,
                "deep",
            ),
            (r#"<text><b>a<sym>space</sym>b</b></text>"#, "ab"),
            (
                r#"<text><font size="9.2"/><font face="Edwin"/>self-closed</text>"#,
                "self-closed",
            ),
        ] {
            let midi = parse_mscx(&mscx(xml)).unwrap();
            assert_eq!(lyrics_of(&midi), vec![want], "input: {}", xml);
        }
    }

    #[test]
    fn styled_track_name_is_read() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <Instrument><longName><b>Sopra</b>no</longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Soprano"]);
    }

    #[test]
    fn br_separates_words_in_names_and_lyrics() {
        // Lyric: <br/> must never fuse adjacent words.
        let midi = parse_mscx(&mscx("<text>a<br/>b</text>")).unwrap();
        assert_eq!(lyrics_of(&midi), vec!["a b"]);
        // Name: real-world case from tests/fixtures/help.mscz.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <Instrument><longName>Batterie ou<br/>persussions<br/>corporelles</longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Batterie ou persussions corporelles"]);
    }

    #[test]
    fn multiline_track_name_is_collapsed_to_one_line() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Soprano
Melodie</trackName>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Soprano Melodie"]);
    }

    #[test]
    fn deeply_nested_forged_xml_is_rejected_cleanly() {
        let mut xml = String::from(r#"<museScore version="3.02"><Score><Division>480</Division>"#);
        for _ in 0..250 {
            xml.push_str("<b>");
        }
        xml.push('x');
        for _ in 0..250 {
            xml.push_str("</b>");
        }
        xml.push_str("</Score></museScore>");
        let err = match parse_mscx(&xml) {
            Err(e) => e,
            Ok(_) => panic!("expected a nesting error"),
        };
        assert!(
            err.contains("nesting"),
            "expected a clean nesting error, got: {}",
            err
        );
    }

    #[test]
    fn empty_long_name_falls_back_to_track_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part>
      <trackName>Voix</trackName>
      <Instrument><longName> </longName></Instrument>
      <Staff id="1"/>
    </Part>
    <Staff id="1">
      <Measure>
        <voice>
          <Chord>
            <durationType>quarter</durationType>
            <Note><pitch>60</pitch></Note>
          </Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<String> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Voix"]);
    }

    #[test]
    fn present_invalid_division_is_rejected_instead_of_replaced() {
        for invalid in ["0", "480.5"] {
            let xml = mscx("<text>let</text>").replace(
                "<Division>480</Division>",
                &format!("<Division>{invalid}</Division>"),
            );
            let error = parse_mscx(&xml).expect_err("invalid Division must fail");
            assert!(error.contains("Division"), "unexpected error: {error}");
        }
    }

    #[test]
    fn missing_or_unknown_duration_is_never_replaced_by_a_quarter() {
        for replacement in ["", "<durationType>mystery</durationType>"] {
            let xml = mscx("<text>let</text>")
                .replace("<durationType>quarter</durationType>", replacement);
            let error = parse_mscx(&xml).expect_err("duration must fail explicitly");
            assert!(
                error.contains("durationType"),
                "unexpected error for {replacement:?}: {error}"
            );
        }
    }

    #[test]
    fn out_of_range_pitch_is_rejected_instead_of_clamped() {
        let xml = mscx("<text>let</text>").replace("<pitch>60</pitch>", "<pitch>200</pitch>");
        let error = parse_mscx(&xml).expect_err("invalid pitch must fail");
        assert!(error.contains("pitch"), "unexpected error: {error}");
    }

    #[test]
    fn grace_note_keeps_zero_playback_duration_and_is_counted_as_source() {
        let xml = mscx("<text>let</text>").replace(
            "<durationType>quarter</durationType>",
            "<acciaccatura/><durationType>eighth</durationType>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let note_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(_) | Kind::NoteOff(_) => Some(event.tick),
                _ => None,
            })
            .collect();
        assert_eq!(note_ticks, vec![0, 0]);
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        assert_eq!(outcome.tracks[0].notes, 1);
        assert_eq!(outcome.placed, 0);
        assert!(
            crate::engine::target::svp::serialize(outcome.svp.as_ref().unwrap())
                .expect("exactly representable")
                .tracks
                .is_empty()
        );
    }

    /// One score body, parameterised by what the part declares and by the notes
    /// of its single lyric-bearing chord.
    fn chord_score(instrument: &str, pitches: &[u16], lyrics: &str) -> String {
        let notes: String = pitches
            .iter()
            .map(|pitch| format!("<Note><pitch>{pitch}</pitch></Note>"))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Part</trackName>{instrument}<Staff id="1"/></Part>
    <Staff id="1">
      <Measure><voice>
        <Chord><durationType>whole</durationType>{lyrics}{notes}</Chord>
      </voice></Measure>
    </Staff>
  </Score>
</museScore>"#
        )
    }

    /// Every note the parser emitted, with the lyric text it carries.
    fn sung_pitches(midi: &Midi) -> Vec<(u8, Option<String>)> {
        let mut out: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|track| track.events.iter())
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) if note.velocity != Some(0) => Some((
                    note.key?,
                    note.lyrics.first().map(|lyric| match &lyric.state {
                        LyricState::Text(text) => text.clone(),
                        other => format!("{other:?}"),
                    }),
                )),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// MuseScore omits `<no>` only for the first verse, so two `<Lyrics>` that
    /// both omit it are one verse written twice — not a second verse. Reading
    /// the sibling index as a verse number copied a whole score's lanes out a
    /// second time, note for note.
    #[test]
    fn two_lyrics_without_a_number_stay_one_verse() {
        let xml = chord_score(
            "",
            &[60],
            "<Lyrics><text>dit</text></Lyrics><Lyrics><text>dit</text></Lyrics>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let lanes: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|track| track.events.iter())
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note.lyrics.iter().map(|l| l.lane.clone())),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(lanes, vec!["1", "1"], "both belong to the first verse");
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        let project = outcome.svp.expect("a projection");
        assert_eq!(project.tracks.len(), 1, "one verse means one lane");
    }

    /// A verse the source really numbers still becomes its own lane.
    #[test]
    fn a_numbered_second_verse_still_makes_its_own_lane() {
        let xml = chord_score(
            "",
            &[60],
            "<Lyrics><text>one</text></Lyrics><Lyrics><no>1</no><text>two</text></Lyrics>",
        );
        let midi = parse_mscx(&xml).unwrap();
        let outcome = crate::engine::convert::convert_midi(&midi, "english");
        let project = outcome.svp.expect("a projection");
        assert_eq!(project.tracks.len(), 2);
    }

    /// A declared non-singing instrument reads a chord as an accompaniment: one
    /// singer takes the word, and a singer sings the top line of a reduction.
    #[test]
    fn a_piano_chord_gives_its_word_to_the_highest_note_only() {
        let xml = chord_score(
            "<Instrument id=\"piano\"><instrumentId>keyboard.piano</instrumentId></Instrument>",
            &[60, 64, 67],
            "<Lyrics><text>dit</text></Lyrics>",
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(
            sung_pitches(&midi),
            vec![(60, None), (64, None), (67, Some("dit".into())),],
            "every note survives; only the highest carries the word"
        );
    }

    /// A declared singing instrument reads the same chord as harmony, which is
    /// what a choir writes, so nothing is taken away from any member.
    #[test]
    fn a_choral_chord_keeps_the_word_on_every_member() {
        for declared in [
            "<Instrument id=\"soprano\"><instrumentId>voice.soprano</instrumentId></Instrument>",
            "<Instrument id=\"voice\"><instrumentId>voice.vocals</instrumentId></Instrument>",
        ] {
            let xml = chord_score(declared, &[60, 64, 67], "<Lyrics><text>dit</text></Lyrics>");
            let midi = parse_mscx(&xml).unwrap();
            assert_eq!(
                sung_pitches(&midi),
                vec![
                    (60, Some("dit".into())),
                    (64, Some("dit".into())),
                    (67, Some("dit".into())),
                ],
                "{declared} must keep every voice singing"
            );
        }
    }

    /// Nothing declared: the part is measured against its own chords. A part
    /// whose texted chords are polyphonic throughout is a harmony; one that
    /// carries the occasional chord under a single-note melody is not.
    #[test]
    fn an_undeclared_part_is_measured_against_its_own_chords() {
        let harmonised = chord_score("", &[60, 64], "<Lyrics><text>dit</text></Lyrics>");
        assert_eq!(
            sung_pitches(&parse_mscx(&harmonised).unwrap()),
            vec![(60, Some("dit".into())), (64, Some("dit".into()))],
            "its only texted chord is polyphonic, so it reads as harmony"
        );

        // The same chord, now one of four texted chords, three of which are a
        // single note. That is a melody with an occasional chord under it.
        let reduction = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Part</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure><voice>
        <Chord><durationType>quarter</durationType><Lyrics><text>a</text></Lyrics><Note><pitch>60</pitch></Note></Chord>
        <Chord><durationType>quarter</durationType><Lyrics><text>b</text></Lyrics><Note><pitch>62</pitch></Note></Chord>
        <Chord><durationType>quarter</durationType><Lyrics><text>c</text></Lyrics><Note><pitch>64</pitch></Note></Chord>
        <Chord><durationType>quarter</durationType><Lyrics><text>d</text></Lyrics><Note><pitch>65</pitch></Note><Note><pitch>72</pitch></Note></Chord>
      </voice></Measure>
    </Staff>
  </Score>
</museScore>"#;
        let sung = sung_pitches(&parse_mscx(reduction).unwrap());
        assert_eq!(sung.iter().filter(|(_, lyric)| lyric.is_some()).count(), 4);
        assert!(
            sung.contains(&(72, Some("d".into()))) && sung.contains(&(65, None)),
            "the chord's word goes to its highest note alone: {sung:?}"
        );
    }

    /// MuseScore writes an empty `<Lyrics>` on a tied note to say nothing is sung
    /// there. Counting it as a word let one such element on a piano chord tip a
    /// whole part into the harmony reading — the misreading this rule exists to
    /// prevent, arriving through the rule itself.
    #[test]
    fn an_empty_lyric_element_does_not_make_a_chord_texted() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Part</trackName><Staff id="1"/></Part>
    <Staff id="1"><Measure><voice>
      <Chord><durationType>half</durationType><Lyrics><text>word</text></Lyrics><Note><pitch>60</pitch></Note></Chord>
      <Chord><durationType>half</durationType><Lyrics><text></text></Lyrics><Note><pitch>64</pitch></Note><Note><pitch>72</pitch></Note></Chord>
    </voice></Measure></Staff>
  </Score>
</museScore>"#;
        let sung = sung_pitches(&parse_mscx(xml).unwrap());
        assert_eq!(
            sung.iter().filter(|(_, lyric)| lyric.is_some()).count(),
            3,
            "the empty element is still carried, it just is not a word: {sung:?}"
        );
        assert!(
            sung.contains(&(60, Some("word".into()))),
            "the only real word is untouched: {sung:?}"
        );
    }

    /// A real MuseScore file often states no `<instrumentId>`, and the parser
    /// suffixes a per-channel identity onto the `id` it falls back to. Comparing
    /// that whole string against the taxonomy answered "not a singer" for every
    /// such choir.
    #[test]
    fn a_channel_suffixed_identifier_still_declares_a_singer() {
        let singer = InstrumentInfo {
            id: Some("soprano:channel:0".into()),
            ..InstrumentInfo::default()
        };
        assert_eq!(singer.declares_a_singer(), Some(true));
        let empty_sound_id = InstrumentInfo {
            sound_id: Some("   ".into()),
            id: Some("voice.alto".into()),
            ..InstrumentInfo::default()
        };
        assert_eq!(
            empty_sound_id.declares_a_singer(),
            Some(true),
            "an empty taxonomy value must not shadow a usable id"
        );
        let piano = InstrumentInfo {
            sound_id: Some("keyboard.piano".into()),
            ..InstrumentInfo::default()
        };
        assert_eq!(piano.declares_a_singer(), Some(false));
        assert_eq!(InstrumentInfo::default().declares_a_singer(), None);
    }

    /// A part with several staves gave every one of them the part's name, so a
    /// project opened with two different lanes under one identical label.
    #[test]
    fn the_staves_of_one_part_are_told_apart_by_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Piano</trackName><Staff id="1"/><Staff id="2"/></Part>
    <Staff id="1"><Measure><voice>
      <Chord><durationType>whole</durationType><Lyrics><text>up</text></Lyrics><Note><pitch>72</pitch></Note></Chord>
    </voice></Measure></Staff>
    <Staff id="2"><Measure><voice>
      <Chord><durationType>whole</durationType><Lyrics><text>down</text></Lyrics><Note><pitch>48</pitch></Note></Chord>
    </voice></Measure></Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let names: Vec<_> = midi.tracks.iter().map(|track| track.name.clone()).collect();
        assert_eq!(names, vec!["Piano — staff 1", "Piano — staff 2"]);
    }

    /// Staff ids are strings, so `10` sorts before `9`. Numbering them by key
    /// order labelled the second staff of a piano as the first.
    #[test]
    fn staves_are_numbered_in_the_order_the_score_writes_them() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Piano</trackName><Staff id="9"/><Staff id="10"/></Part>
    <Staff id="9"><Measure><voice>
      <Chord><durationType>whole</durationType><Lyrics><text>up</text></Lyrics><Note><pitch>72</pitch></Note></Chord>
    </voice></Measure></Staff>
    <Staff id="10"><Measure><voice>
      <Chord><durationType>whole</durationType><Lyrics><text>down</text></Lyrics><Note><pitch>48</pitch></Note></Chord>
    </voice></Measure></Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let named: Vec<_> = midi
            .tracks
            .iter()
            .map(|track| (track.name.as_str(), track.source.staff_id.clone()))
            .collect();
        assert_eq!(
            named,
            vec![
                ("Piano — staff 1", Some("9".to_string())),
                ("Piano — staff 2", Some("10".to_string())),
            ]
        );
    }

    /// A single-staff part must keep exactly the name it has always had, even
    /// when the Part element declares a staff the score body never carries.
    #[test]
    fn a_part_with_one_written_staff_keeps_its_plain_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/><Staff id="2"/></Part>
    <Staff id="1"><Measure><voice>
      <Chord><durationType>whole</durationType><Lyrics><text>la</text></Lyrics><Note><pitch>60</pitch></Note></Chord>
    </voice></Measure></Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        assert_eq!(
            midi.tracks
                .iter()
                .map(|track| track.name.clone())
                .collect::<Vec<_>>(),
            vec!["Voice"]
        );
    }

    #[test]
    fn repeat_occurrences_reemit_tempo_and_meter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure>
        <startRepeat/>
        <voice>
          <TimeSig><sigN>3</sigN><sigD>4</sigD></TimeSig>
          <Tempo><tempo>1.5</tempo></Tempo>
          <Chord><durationType>quarter</durationType><Note><pitch>60</pitch></Note></Chord>
        </voice>
        <endRepeat>2</endRepeat>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        let tempo_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| matches!(event.kind, Kind::Tempo(_)).then_some(event.tick))
            .collect();
        let meter_ticks: Vec<_> = midi.tracks[0]
            .events
            .iter()
            .filter_map(|event| matches!(event.kind, Kind::TimeSig { .. }).then_some(event.tick))
            .collect();
        assert_eq!(tempo_ticks, vec![0, 1_440]);
        assert_eq!(meter_ticks, vec![0, 1_440]);
    }

    #[test]
    fn globals_survive_when_the_first_staff_contains_only_rests() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Rest</trackName><Staff id="1"/></Part>
    <Part><trackName>Voice</trackName><Staff id="2"/></Part>
    <Staff id="1">
      <Measure><voice><Rest><durationType>quarter</durationType></Rest></voice></Measure>
    </Staff>
    <Staff id="2">
      <Measure>
        <voice>
          <TimeSig><sigN>6</sigN><sigD>8</sigD></TimeSig>
          <Tempo><tempo>1.2</tempo></Tempo>
          <Chord><durationType>quarter</durationType><Note><pitch>62</pitch></Note></Chord>
        </voice>
      </Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::TimeSig { num: 6, den: 8, .. })));
        assert!(midi
            .tracks
            .iter()
            .flat_map(|track| &track.events)
            .any(|event| matches!(event.kind, Kind::Tempo(_))));
        assert_eq!(midi.topology.part_count(), 2);
        assert_eq!(midi.topology.parts[0].name, "Rest");
        assert_eq!(midi.topology.parts[0].staves.len(), 1);
        assert_eq!(midi.topology.parts[0].staves[0].voices.len(), 1);
        assert!(midi.topology.parts[0].staves[0].voices[0]
            .projection_track_ids
            .is_empty());
    }

    #[test]
    fn musescore_dtd_is_rejected() {
        let xml = mscx("<text>let</text>").replace(
            "<museScore",
            "<!DOCTYPE museScore SYSTEM \"file:///tmp/forbidden.dtd\">\n<museScore",
        );
        let error = match parse_mscx(&xml) {
            Err(error) => error,
            Ok(_) => panic!("MuseScore DTDs must stay disabled"),
        };
        assert!(error.contains("DTD") || error.contains("XML"));
    }

    #[test]
    fn negative_lyric_extension_is_preserved_without_becoming_a_continuation() {
        let midi = parse_mscx(&mscx(
            "<text>let</text><ticks>-1680</ticks><ticks_f>-7/8</ticks_f>",
        ))
        .unwrap();
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(lyric.extend_ticks, Some(-1680));
        assert_eq!(lyric.extend_fraction, Some((-7, 8)));
    }

    /// A score may state a lyric extension only as a fraction, and the field was
    /// parsed and never read, so that melisma was projected as no extension at
    /// all. Measured over the pinned corpus: every one of the 10968 lyrics
    /// stating both units agrees on `ticks = ticks_f x 4 x Division`, which the
    /// neighbouring `-7/8` / `-1680` case states too.
    #[test]
    fn an_extension_written_only_as_a_fraction_is_read_in_ticks() {
        for (fraction, expected) in [("-7/8", -1680), ("1/2", 960), ("1/4", 480)] {
            let midi = parse_mscx(&mscx(&format!(
                "<text>let</text><ticks_f>{fraction}</ticks_f>"
            )))
            .unwrap();
            let lyric = midi.tracks[0]
                .events
                .iter()
                .find_map(|event| match &event.kind {
                    Kind::NoteOn(note) => note.lyrics.first(),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                lyric.extend_ticks,
                Some(expected),
                "{fraction} of a whole note at Division 480"
            );
        }
    }

    /// An extension the division cannot state exactly is refused rather than
    /// rounded, the same as every other duration here: a rounded melisma holds
    /// a syllable over a note the score never asked for.
    #[test]
    fn a_fractional_extension_off_the_division_is_refused_instead_of_rounded() {
        let error = parse_mscx(&mscx("<text>let</text><ticks_f>1/7</ticks_f>"))
            .expect_err("a seventh of a whole note is not a whole number of ticks at 480");
        assert!(error.contains("1/7"), "{error}");
    }

    /// Where the score states both, the explicit tick count is the one read; the
    /// fraction is a second statement of the same thing, not a correction.
    #[test]
    fn an_explicit_tick_count_wins_over_the_fraction_beside_it() {
        let midi = parse_mscx(&mscx(
            "<text>let</text><ticks>960</ticks><ticks_f>1/2</ticks_f>",
        ))
        .unwrap();
        let lyric = midi.tracks[0]
            .events
            .iter()
            .find_map(|event| match &event.kind {
                Kind::NoteOn(note) => note.lyrics.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(lyric.extend_ticks, Some(960));
        assert_eq!(lyric.extend_fraction, Some((1, 2)));
    }

    /// Builds a one-staff score whose measures are supplied verbatim, so a tie
    /// case can be written exactly as MuseScore stores it.
    fn tie_score(measures: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">{measures}</Staff>
  </Score>
</museScore>"#
        )
    }

    #[test]
    fn repeated_note_xml_and_projection_evidence_share_original_bytes() {
        let payload = "ignored ".repeat(128 * 1024);
        let xml = tie_score(&format!(
            "<Measure><startRepeat/><voice><Chord><durationType>whole</durationType>\
             <Note><pitch>60</pitch><Unknown>{payload}</Unknown></Note>\
             </Chord></voice><endRepeat>32</endRepeat></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        let notes: Vec<_> = midi
            .tracks
            .iter()
            .flat_map(|t| &t.events)
            .filter_map(|e| {
                if let Kind::NoteOn(note) = &e.kind {
                    Some(note)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(notes.len(), 32);
        let first = notes[0].source.continuity.as_ref().unwrap();
        assert!(first.evidence.raw_xml.contains(&payload));
        for (occurrence, note) in notes.iter().enumerate() {
            let proof = note.source.continuity.as_ref().unwrap();
            assert_eq!(note.source.occurrence, occurrence as u32);
            assert!(Arc::ptr_eq(
                &first.evidence.raw_xml,
                &proof.evidence.raw_xml
            ));
            let retained = note.source.clone();
            assert!(Arc::ptr_eq(proof, retained.continuity.as_ref().unwrap()));
            let evidence_clone = proof.evidence.clone();
            assert!(Arc::ptr_eq(
                &proof.evidence.raw_xml,
                &evidence_clone.raw_xml
            ));
        }
    }

    #[test]
    fn continuity_pool_checks_payload_and_metadata_before_allocation() {
        let xml = "<museScore version=\"3.02\"><Note><pitch>60</pitch></Note></museScore>";
        let doc = roxmltree::Document::parse(xml).unwrap();
        let node = doc.descendants().find(|n| n.has_tag_name("Note")).unwrap();
        let mut pool = ContinuityEvidencePool::new(doc.root_element()).unwrap();
        let first = pool.evidence(node, "note:0").unwrap();
        let used = pool.bytes;
        let second = pool.evidence(node, "note:0").unwrap();
        assert!(Arc::ptr_eq(&first.raw_xml, &second.raw_xml));
        assert_eq!(
            pool.bytes - used,
            std::mem::size_of::<SourceEvidenceRef>() + "note:0".len()
        );
        pool.limit = pool.bytes;
        assert!(pool
            .evidence(node, "note:0")
            .is_err_and(|e| e.starts_with("SOURCE_CONTINUITY_LIMIT:")));
        assert_eq!(pool.xml.len(), 1);

        let mut fresh = ContinuityEvidencePool::new(doc.root_element()).unwrap();
        fresh.limit =
            fresh.bytes + std::mem::size_of::<SourceEvidenceRef>() + 6 + node.range().len() + 63;
        assert!(fresh
            .evidence(node, "note:0")
            .is_err_and(|e| e.starts_with("SOURCE_CONTINUITY_LIMIT:")));
        assert!(fresh.xml.is_empty(), "refuse before retaining a payload");
    }

    /// Played notes with their duration, rebuilt by pairing note-on/note-off the
    /// way the projector does. A merged tie appears here as one longer note; an
    /// unmerged one as two notes.
    fn played_notes(midi: &Midi) -> Vec<(u32, u32, u8)> {
        let mut played = Vec::new();
        for track in &midi.tracks {
            for (index, event) in track.events.iter().enumerate() {
                let Kind::NoteOn(note) = &event.kind else {
                    continue;
                };
                let Some(key) = note.key else { continue };
                let end = track.events[index + 1..]
                    .iter()
                    .find_map(|candidate| match &candidate.kind {
                        Kind::NoteOff(off) if off.source_id == Some(note.source.id.clone()) => {
                            Some(candidate.tick)
                        }
                        _ => None,
                    })
                    .unwrap_or(event.tick);
                played.push((event.tick, end - event.tick, key));
            }
        }
        played.sort_unstable();
        played
    }

    const TIE_START: &str = r#"<Spanner type="Tie"><Tie/><next><location><fractions>1/4</fractions></location></next></Spanner>"#;
    const TIE_STOP: &str = r#"<Spanner type="Tie"><prev><location><fractions>-1/4</fractions></location></prev></Spanner>"#;

    fn quarter(pitch: u8, spanners: &str) -> String {
        format!(
            "<Chord><durationType>quarter</durationType><Note><pitch>{pitch}</pitch>{spanners}</Note></Chord>"
        )
    }

    fn source_notes(midi: &Midi) -> Vec<&NoteOn> {
        midi.tracks
            .iter()
            .flat_map(|track| &track.events)
            .filter_map(|event| match &event.kind {
                Kind::NoteOn(note) => Some(note),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn continuity_keeps_version_and_exact_chord_lyric_ownership_on_all_members() {
        let xml = tie_score(
            "<Measure><voice><Chord><durationType>quarter</durationType>\
             <Lyrics><text>same</text><ticks>480</ticks><ticks_f>1/4</ticks_f></Lyrics>\
             <Note><pitch>60</pitch></Note><Note><pitch>64</pitch></Note></Chord>\
             <Chord><durationType>quarter</durationType><Note><pitch>64</pitch></Note></Chord>\
             </voice></Measure>",
        )
        .replace("<Score>", "<programVersion>3.6.2</programVersion><Score>");
        let midi = parse_mscx(&xml).unwrap();
        let notes = source_notes(&midi);
        assert_eq!(notes.len(), 3);
        assert!(notes.iter().all(|note| note.source.continuity.is_some()));
        let owners: Vec<_> = notes
            .iter()
            .filter_map(|note| {
                note.source
                    .continuity
                    .as_ref()
                    .filter(|proof| !proof.extensions.is_empty())
            })
            .collect();
        assert_eq!(owners.len(), 2);
        assert_eq!(owners[0].chord_id, owners[1].chord_id);
        assert_eq!(owners[0].extensions, owners[1].extensions);
        let extension = &owners[0].extensions[0];
        assert_eq!(extension.start_tick, 0);
        assert_eq!(extension.end_tick, Some(480));
        assert_eq!(extension.extend_fraction, Some((1, 4)));
        assert_eq!(extension.raw_ticks, Some(480));
        assert_eq!(extension.evidence.source_format, SourceFormat::MuseScore);
        assert_eq!(extension.evidence.source_version.as_deref(), Some("3.02"));
        assert_eq!(extension.evidence.program_version.as_deref(), Some("3.6.2"));
        assert_eq!(extension.evidence.source_id, extension.lyric_id);
        assert!(extension
            .evidence
            .raw_xml
            .contains("<ticks_f>1/4</ticks_f>"));
        for note in notes {
            assert_eq!(
                note.source.continuity.as_ref().unwrap().evidence.source_id,
                note.source.id
            );
        }
    }

    #[test]
    fn typed_extension_bounds_preserve_fractions_sentinels_and_conflicts() {
        for (raw, expected_end, expected_ticks, expected_raw_ticks) in [
            ("<ticks_f>1/2</ticks_f>", Some(960), Some(960), None),
            ("<ticks>0</ticks>", Some(0), Some(0), Some(0)),
            ("<ticks_f>0/1</ticks_f>", Some(0), None, None),
            (
                "<ticks>1</ticks><ticks_f>1/1920</ticks_f>",
                Some(1),
                Some(1),
                Some(1),
            ),
            (
                "<ticks>480</ticks><ticks_f>1/2</ticks_f>",
                None,
                Some(480),
                Some(480),
            ),
            ("<ticks>-1</ticks>", None, Some(-1), Some(-1)),
            (
                "<ticks>4294967296</ticks>",
                None,
                Some(4294967296),
                Some(4294967296),
            ),
        ] {
            let midi = parse_mscx(&mscx(&format!("<text>word</text>{raw}"))).unwrap();
            let note = source_notes(&midi)[0];
            let continuity = note.source.continuity.as_ref().unwrap();
            let extension = &continuity.extensions[0];
            assert_eq!(extension.end_tick, expected_end, "{raw}");
            assert_eq!(extension.extend_ticks, expected_ticks, "{raw}");
            assert_eq!(extension.raw_ticks, expected_raw_ticks, "{raw}");
            assert_eq!(
                continuity.issues.is_empty(),
                expected_end.is_some(),
                "{raw}"
            );
            assert!(extension.evidence.raw_xml.contains(raw));
        }
    }

    #[test]
    fn lyric_free_notes_have_typed_identity_without_extension_authority() {
        let midi = parse_mscx(&tie_score(&format!(
            "<Measure><voice>{}</voice></Measure>",
            quarter(60, "")
        )))
        .unwrap();
        let proof = source_notes(&midi)[0].source.continuity.as_ref().unwrap();
        assert!(proof.extensions.is_empty());
        assert!(proof.incoming_tie.is_none());
        assert!(proof.issues.is_empty());
    }

    #[test]
    fn text_bearing_tie_tails_keep_validated_source_links_on_each_repeat_pass() {
        let xml = tie_score(&format!(
            "<Measure><startRepeat/><voice>\
             <Chord><durationType>quarter</durationType><Lyrics><text>first</text></Lyrics>\
             <Note><pitch>65</pitch>{TIE_START}</Note></Chord>\
             <Chord><durationType>quarter</durationType><Lyrics><no>1</no><text>second</text></Lyrics>\
             <Note><pitch>65</pitch>{TIE_STOP}</Note></Chord>\
             </voice><endRepeat>2</endRepeat></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(
            played_notes(&midi),
            vec![
                (0, 480, 65),
                (480, 480, 65),
                (1920, 480, 65),
                (2400, 480, 65)
            ]
        );
        let notes = source_notes(&midi);
        for (pass, pair) in notes.chunks_exact(2).enumerate() {
            let head = pair[0];
            let tail = pair[1];
            let proof = tail.source.continuity.as_ref().unwrap();
            let tie = proof.incoming_tie.as_ref().unwrap();
            assert_eq!(proof.playback_segment, pass as u32);
            assert_eq!(tie.head.source_id, head.source.id);
            assert_eq!(tie.tail.source_id, tail.source.id);
            assert_eq!(tie.head.occurrence, pass as u32);
            assert_eq!(tie.tail.occurrence, pass as u32);
            assert_eq!(tie.contact_tick, pass as u32 * 1920 + 480);
            assert_eq!(tie.pitch, 65);
            assert!(tie.evidence[0].raw_xml.contains(TIE_START));
            assert!(tie.evidence[1].raw_xml.contains(TIE_STOP));
            assert!(proof.issues.is_empty());
        }
    }

    #[test]
    fn measure_relative_tie_locations_validate_the_text_bearing_tail() {
        let start = r#"<Spanner type="Tie"><Tie/><next><location><measures>1</measures><fractions>-7/8</fractions></location></next></Spanner>"#;
        let stop = r#"<Spanner type="Tie"><prev><location><measures>-1</measures><fractions>7/8</fractions></location></prev></Spanner>"#;
        let xml = tie_score(&format!(
            "<Measure><voice><Rest><durationType>half</durationType><dots>2</dots></Rest>\
             <Chord><durationType>eighth</durationType><Lyrics><text>word</text></Lyrics>\
             <Note><pitch>64</pitch>{start}</Note></Chord></voice></Measure>\
             <Measure><voice><Chord><durationType>half</durationType>\
             <Lyrics><no>1</no><text>other verse</text></Lyrics>\
             <Note><pitch>64</pitch>{stop}</Note></Chord></voice></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(1680, 240, 64), (1920, 960, 64)]);
        let proof = source_notes(&midi)[1].source.continuity.as_ref().unwrap();
        assert_eq!(proof.incoming_tie.as_ref().unwrap().contact_tick, 1920);
        assert!(proof.issues.is_empty());
    }

    #[test]
    fn merged_bare_tail_chain_keeps_each_immediate_source_relationship() {
        let midi = parse_mscx(&tie_score(&format!(
            "<Measure><voice>{}{}{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(65, &format!("{TIE_STOP}{TIE_START}")),
            quarter(65, TIE_STOP)
        )))
        .unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 1440, 65)]);
        let notes = source_notes(&midi);
        assert_eq!(notes.len(), 3);
        for index in 1..notes.len() {
            assert_eq!(notes[index].key, None);
            let proof = notes[index].source.continuity.as_ref().unwrap();
            assert_eq!(
                proof.incoming_tie.as_ref().unwrap().head.source_id,
                notes[index - 1].source.id
            );
        }
    }

    #[test]
    fn invalid_text_bearing_links_retain_raw_evidence_without_recovery_proof() {
        for (start, stop, tail_pitch, gap) in [
            (
                TIE_START.to_string(),
                TIE_STOP.replace("-1/4", "-1/2"),
                65,
                "",
            ),
            (
                TIE_START.replace("1/4", "1/2"),
                TIE_STOP.to_string(),
                65,
                "",
            ),
            (TIE_START.to_string(), TIE_STOP.to_string(), 66, ""),
            (
                TIE_START.to_string(),
                TIE_STOP.to_string(),
                65,
                "<Rest><durationType>quarter</durationType></Rest>",
            ),
            (
                TIE_START.to_string(),
                TIE_STOP.replace("-1/4", "broken"),
                65,
                "",
            ),
            (
                TIE_START.to_string(),
                TIE_STOP.replace("<fractions>", "<staves>1</staves><fractions>"),
                65,
                "",
            ),
            (
                TIE_START.to_string(),
                TIE_STOP.replace("<fractions>", "<notes>1</notes><fractions>"),
                65,
                "",
            ),
        ] {
            let midi = parse_mscx(&tie_score(&format!(
                "<Measure><voice>{}{gap}<Chord><durationType>quarter</durationType>\
                 <Lyrics><no>1</no><text>tail</text></Lyrics>\
                 <Note><pitch>{tail_pitch}</pitch>{stop}</Note></Chord></voice></Measure>",
                quarter(65, &start)
            )))
            .unwrap();
            let proof = source_notes(&midi)[1].source.continuity.as_ref().unwrap();
            assert!(proof.incoming_tie.is_none(), "{start} {stop}");
            assert!(proof
                .issues
                .iter()
                .any(|issue| issue.code == "SOURCE_CONTINUITY_LINK_INVALID"));
            assert!(proof.evidence.raw_xml.contains(&stop));
            assert_eq!(played_notes(&midi).len(), 2);
        }
    }

    #[test]
    fn overlapping_same_key_tie_heads_do_not_choose_the_last_chord_member() {
        let xml = tie_score(&format!(
            "<Measure><voice><Chord><durationType>quarter</durationType>\
             <Note><pitch>65</pitch>{TIE_START}</Note><Note><pitch>65</pitch>{TIE_START}</Note>\
             </Chord><Chord><durationType>quarter</durationType><Lyrics><text>tail</text></Lyrics>\
             <Note><pitch>65</pitch>{TIE_STOP}</Note></Chord></voice></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        let notes = source_notes(&midi);
        let tail = notes.iter().find(|note| !note.lyrics.is_empty()).unwrap();
        let proof = tail.source.continuity.as_ref().unwrap();
        assert!(proof.incoming_tie.is_none());
        assert_eq!(proof.issues[0].code, "SOURCE_CONTINUITY_LINK_INVALID");
    }

    #[test]
    fn a_tie_becomes_one_sustained_note() {
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(65, TIE_STOP)
        ));
        let midi = parse_mscx(&xml).unwrap();
        // 480 + 480 ticks sung as a single attack, not two.
        assert_eq!(played_notes(&midi), vec![(0, 960, 65)]);
    }

    #[test]
    fn a_tie_chain_accumulates_every_link() {
        let middle = format!("{TIE_STOP}{TIE_START}");
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(65, &middle),
            quarter(65, TIE_STOP)
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 1_440, 65)]);
    }

    #[test]
    fn a_tie_tail_that_carries_a_syllable_keeps_its_own_attack() {
        // The score puts a word on the tied note, so it is not a plain sustain:
        // merging it would delete that word from the projection entirely.
        let xml = tie_score(&format!(
            "<Measure><voice>\
               <Chord><durationType>quarter</durationType><Lyrics><text>appreci</text></Lyrics><Note><pitch>65</pitch>{TIE_START}</Note></Chord>\
               <Chord><durationType>quarter</durationType><Lyrics><text>ate</text></Lyrics><Note><pitch>65</pitch>{TIE_STOP}</Note></Chord>\
             </voice></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 65)]);
    }

    #[test]
    fn a_tie_tail_with_an_explicitly_empty_lyric_still_merges() {
        // MuseScore writes an empty `<Lyrics>` on a tied note precisely to say
        // that nothing is sung there, so it argues for merging, not against it.
        let xml = tie_score(&format!(
            "<Measure><voice>\
               <Chord><durationType>quarter</durationType><Lyrics><text>shine</text></Lyrics><Note><pitch>65</pitch>{TIE_START}</Note></Chord>\
               <Chord><durationType>quarter</durationType><Lyrics><text></text></Lyrics><Note><pitch>65</pitch>{TIE_STOP}</Note></Chord>\
             </voice></Measure>"
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 960, 65)]);
    }

    #[test]
    fn a_repeated_note_without_a_tie_stays_two_notes() {
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, ""),
            quarter(65, "")
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 65)]);
    }

    #[test]
    fn a_non_tie_spanner_on_a_note_never_merges() {
        // Real scores carry TextLine and Glissando spanners on `<Note>` too, so
        // the type filter is what keeps them from being read as ties.
        let start = r#"<Spanner type="TextLine"><next><location><fractions>1/4</fractions></location></next></Spanner>"#;
        let stop = r#"<Spanner type="Glissando"><prev><location><fractions>-1/4</fractions></location></prev></Spanner>"#;
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, start),
            quarter(65, stop)
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 65)]);
    }

    #[test]
    fn a_slur_between_equal_pitches_never_merges() {
        // A slur is phrasing, not sustain, and MuseScore stores it on the
        // `<Chord>` rather than on the `<Note>`.
        let xml = tie_score(
            r#"<Measure><voice>
              <Chord><durationType>quarter</durationType><Spanner type="Slur"><next><location><fractions>1/4</fractions></location></next></Spanner><Note><pitch>65</pitch></Note></Chord>
              <Chord><durationType>quarter</durationType><Note><pitch>65</pitch></Note></Chord>
            </voice></Measure>"#,
        );
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 65)]);
    }

    #[test]
    fn a_dangling_tie_start_keeps_its_note_audible() {
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(60, "")
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 60)]);
    }

    #[test]
    fn a_pairing_contradicted_by_the_location_is_refused() {
        // The tail says its head is one measure back, but the only candidate is
        // in the same measure. Pairing on pitch alone would sustain a note the
        // score never sustains, so the merge must be refused.
        let stop = r#"<Spanner type="Tie"><prev><location><measures>-1</measures></location></prev></Spanner>"#;
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(65, stop)
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 480, 65), (480, 480, 65)]);
    }

    #[test]
    fn a_musescore_2_x_tie_merges_through_its_spanner_id() {
        let xml = tie_score(&format!(
            "<Measure><voice>{}{}</voice></Measure>",
            quarter(65, r#"<Tie id="8"/>"#),
            quarter(65, r#"<endSpanner id="8"/>"#)
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(played_notes(&midi), vec![(0, 960, 65)]);
    }

    #[test]
    fn a_tie_does_not_reach_across_a_repeat_jump() {
        // The head sits inside the repeat and the tail after it. Their location
        // evidence describes notated neighbours, but playback replays the
        // repeated measure in between, so the notes are no longer adjacent and
        // the chain cannot be proven. Every occurrence stays its own attack.
        let xml = tie_score(&format!(
            "<Measure><startRepeat/><voice>{}</voice><endRepeat>2</endRepeat></Measure>\
             <Measure><voice>{}</voice></Measure>",
            quarter(65, TIE_START),
            quarter(65, TIE_STOP)
        ));
        let midi = parse_mscx(&xml).unwrap();
        assert_eq!(
            played_notes(&midi),
            vec![(0, 480, 65), (1_920, 480, 65), (3_840, 480, 65)]
        );
    }

    #[test]
    fn every_staff_unrolls_the_repeat_written_on_the_first_one() {
        // MuseScore normally stores repeat barlines on the first staff only.
        // Unrolling each staff against its own marks silently truncated the
        // others by the whole repeated section.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Upper</trackName><Staff id="1"/></Part>
    <Part><trackName>Lower</trackName><Staff id="2"/></Part>
    <Staff id="1">
      <Measure><startRepeat/><voice><Chord><durationType>whole</durationType><Note><pitch>72</pitch></Note></Chord></voice><endRepeat>2</endRepeat></Measure>
    </Staff>
    <Staff id="2">
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        assert_eq!(
            played_notes(&midi),
            vec![
                (0, 1_920, 48),
                (0, 1_920, 72),
                (1_920, 1_920, 48),
                (1_920, 1_920, 72)
            ]
        );
    }

    #[test]
    fn repeat_marks_split_across_staves_are_unioned() {
        // One corpus score writes the barlines on one staff and the volta on
        // another; taking either staff alone loses half the structure.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Upper</trackName><Staff id="1"/></Part>
    <Part><trackName>Lower</trackName><Staff id="2"/></Part>
    <Staff id="1">
      <Measure><startRepeat/><voice><Chord><durationType>whole</durationType><Note><pitch>72</pitch></Note></Chord></voice></Measure>
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>74</pitch></Note></Chord></voice><endRepeat>2</endRepeat></Measure>
    </Staff>
    <Staff id="2">
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>
      <Measure><Spanner type="Volta"><Volta><endings>1</endings></Volta><next><location><measures>1</measures></location></next></Spanner><voice><Chord><durationType>whole</durationType><Note><pitch>50</pitch></Note></Chord></voice></Measure>
    </Staff>
  </Score>
</museScore>"#;
        let midi = parse_mscx(xml).unwrap();
        // The volta is a first ending, so the second pass skips measure 2.
        let ticks: Vec<u32> = played_notes(&midi)
            .iter()
            .filter(|(_, _, pitch)| *pitch == 72)
            .map(|(onset, _, _)| *onset)
            .collect();
        assert_eq!(ticks, vec![0, 3_840]);
    }

    #[test]
    fn staves_that_disagree_on_measure_count_are_rejected() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Upper</trackName><Staff id="1"/></Part>
    <Part><trackName>Lower</trackName><Staff id="2"/></Part>
    <Staff id="1">
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>72</pitch></Note></Chord></voice></Measure>
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>74</pitch></Note></Chord></voice></Measure>
    </Staff>
    <Staff id="2">
      <Measure><voice><Chord><durationType>whole</durationType><Note><pitch>48</pitch></Note></Chord></voice></Measure>
    </Staff>
  </Score>
</museScore>"#;
        let error = parse_mscx(xml).unwrap_err();
        assert!(
            error.contains("disagree on measure count"),
            "unexpected error: {error}"
        );
    }
}
