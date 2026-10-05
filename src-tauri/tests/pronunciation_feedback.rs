use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use verse_lib::{
    engine::{
        convert::convert_midi_with_snapshot,
        musicxml,
        target::{self, ExportTarget, PronunciationProfile},
    },
    pronunciation::{
        assets::{external_paths, Calibration},
        exchange::Package,
        feedback::compare,
        hash,
        laya::{decode, Evidence},
        memory::{Correction, Memory, Scope},
        source_map::{parse_ustx, Reference},
        Snapshot, Work, POLICY,
    },
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "verse-pronunciation-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn source() -> Vec<u8> {
    br#"<score-partwise><part-list><score-part id="P1"><part-name>Voice</part-name><score-instrument id="v"><instrument-name>Voice</instrument-name><instrument-sound>voice.soprano</instrument-sound></score-instrument></score-part></part-list><part id="P1"><measure number="1"><attributes><divisions>480</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>bonjour</text></lyric></note><note><pitch><step>D</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>ami</text></lyric></note><note><pitch><step>E</step><octave>4</octave></pitch><duration>480</duration><voice>1</voice><lyric><text>ici</text></lyric></note></measure></part></score-partwise>"#.to_vec()
}
fn reference() -> (
    Reference,
    Vec<u8>,
    verse_lib::engine::convert::ConvertOutcome,
) {
    let bytes = source();
    let midi = musicxml::parse(&bytes).unwrap();
    let outcome = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    assert!(outcome.ok, "{:?}", outcome.msg);
    let project = outcome.svp.as_ref().unwrap();
    let emitted = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let reference = Reference::build(
        hash(&bytes),
        hash(b"snapshot"),
        project,
        &outcome.pronunciation_words,
        &emitted,
    )
    .unwrap();
    assert_eq!(reference.words.len(), 3);
    (reference, emitted, outcome)
}
fn correction() -> Correction {
    let (reference, _, _) = reference();
    let mut edited = reference.tracks.clone();
    let bound = &reference.words[1];
    edited[bound.track].notes[bound.notes[0]].phonemizer = Some(
        verse_lib::pronunciation::Language::English
            .native_name()
            .into(),
    );
    let mut c = compare(&reference, hash(b"corrected"), &edited)
        .unwrap()
        .proposals
        .remove(0);
    c.provenance.confirmed_after_listening = true;
    c.scope = Scope::CompatibleContext;
    c.after.phones = None;
    c.after.alphabet = None;
    c.seal().unwrap();
    c.validate().unwrap();
    c
}
#[test]
fn source_words_reference_actual_sorted_emitted_geometry() {
    let (_, _, mut outcome) = reference();
    let project = outcome.svp.as_mut().unwrap();
    project.tracks[0].notes.reverse();
    let emitted = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let reference = Reference::build(
        hash(&source()),
        hash(b"snapshot"),
        project,
        &outcome.pronunciation_words,
        &emitted,
    )
    .unwrap();
    for bound in &reference.words {
        assert_eq!(
            bound.word.id,
            project.tracks[0]
                .notes
                .iter()
                .find(|n| n
                    .source_evidence
                    .as_ref()
                    .is_some_and(|e| e.note_id == bound.word.id))
                .unwrap()
                .source_evidence
                .as_ref()
                .unwrap()
                .note_id
        );
        assert_eq!(
            reference.tracks[bound.track].notes[bound.notes[0]].position,
            match bound.word.key.as_str() {
                "bonjour" => 0,
                "ami" => 480,
                "ici" => 960,
                _ => panic!(),
            }
        );
    }
    let changed = String::from_utf8(emitted)
        .unwrap()
        .replacen("tone: 60", "tone: 61", 1);
    assert!(Reference::build(
        hash(&source()),
        hash(b"snapshot"),
        project,
        &outcome.pronunciation_words,
        changed.as_bytes()
    )
    .is_err());
}
#[test]
fn automatic_recalculation_is_not_manual_confirmation() {
    let (r, bytes, _) = reference();
    let tracks = parse_ustx(&bytes).unwrap();
    let review = compare(&r, hash(&bytes), &tracks).unwrap();
    assert!(review.proposals.is_empty());
    let mut c = correction();
    c.provenance.confirmed_after_listening = false;
    c.seal().unwrap();
    assert_eq!(
        c.validate().unwrap_err().code,
        "PRONUNCIATION_CORRECTION_INVALID"
    );
}
#[test]
fn restart_upgrade_rename_and_reference_integrity() {
    let temp = Temp::new();
    let (r, bytes, _) = reference();
    {
        let mut m = Memory::open(&temp.0).unwrap();
        m.save_reference(&r, &bytes).unwrap();
        m.store(&[correction()], true).unwrap();
    }
    let renamed = temp.0.join("renamed-source.xml");
    std::fs::write(&renamed, source()).unwrap();
    let m = Memory::open(&temp.0).unwrap();
    assert_eq!(m.snapshot().unwrap().len(), 1);
    assert_eq!(m.reference(&r.id).unwrap().export_sha256, r.export_sha256);
    std::fs::write(
        temp.0
            .join("references")
            .join(format!("{}.ustx", r.export_sha256)),
        b"changed",
    )
    .unwrap();
    assert!(m.reference(&r.id).is_err());
}
#[test]
fn migration_backs_up_and_downgrade_does_not_modify_future_schema() {
    let temp = Temp::new();
    let db = temp.0.join("pronunciation.sqlite3");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TABLE corrections(id TEXT PRIMARY KEY,fingerprint TEXT NOT NULL UNIQUE,payload TEXT NOT NULL,status TEXT NOT NULL);CREATE TABLE export_references(id TEXT PRIMARY KEY,export_hash TEXT NOT NULL,source_hash TEXT NOT NULL,payload TEXT NOT NULL);PRAGMA user_version=1;").unwrap();
    drop(connection);
    Memory::open(&temp.0).unwrap();
    assert!(std::fs::read_dir(&temp.0).unwrap().any(|p| p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("pronunciation-before-v2")));
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    let before = std::fs::read(&db).unwrap();
    assert!(Memory::open(&temp.0).is_err());
    assert_eq!(std::fs::read(db).unwrap(), before);
}
#[test]
fn exchange_atomic_idempotent_pending_revocation_and_conflicts() {
    let temp = Temp::new();
    let mut memory = Memory::open(&temp.0).unwrap();
    let c = correction();
    memory.store(std::slice::from_ref(&c), true).unwrap();
    let package = Package::confirmed(&memory).unwrap();
    let output = temp.0.join("corrections.json");
    package.write(&output).unwrap();
    assert!(package.write(&output).is_err());
    let imported = Temp::new();
    let mut m = Memory::open(&imported.0).unwrap();
    let p = Package::read(&output).unwrap();
    p.import_pending(&mut m).unwrap();
    p.import_pending(&mut m).unwrap();
    assert_eq!(m.history().unwrap().len(), 1);
    assert!(m.snapshot().unwrap().is_empty());
    m.set_status(&c.id, "active").unwrap();
    assert_eq!(m.snapshot().unwrap().len(), 1);
    m.set_status(&c.id, "revoked").unwrap();
    p.import_pending(&mut m).unwrap();
    assert!(m.snapshot().unwrap().is_empty());
    let mut invalid = p.clone();
    invalid.corrections.push({
        let mut c = c.clone();
        c.fingerprint = "bad".into();
        c
    });
    assert!(invalid.import_pending(&mut m).is_err());
    assert_eq!(m.history().unwrap().len(), 1);
    let mut conflicting = c.clone();
    conflicting.after.language = Some(verse_lib::pronunciation::Language::Spanish);
    conflicting.after.phonemizer = verse_lib::pronunciation::Language::Spanish
        .native_name()
        .into();
    conflicting.seal().unwrap();
    assert!(verse_lib::pronunciation::memory::unique_match(
        &[c.clone(), conflicting],
        &c.word,
        &c.provenance.source_sha256,
        None
    )
    .is_err());
}
#[test]
fn correction_scope_context_inventory_manual_and_source_isolation() {
    let c = correction();
    assert!(c.matches(&c.word, &hash(b"other song"), None));
    let mut other = c.word.clone();
    other.context[0] = "hello".into();
    assert!(!c.matches(&other, &c.provenance.source_sha256, None));
    other = c.word.clone();
    other.manual = true;
    assert!(!c.matches(&other, &c.provenance.source_sha256, None));
    let mut occurrence = c.clone();
    occurrence.scope = Scope::OccurrenceOnly;
    occurrence.seal().unwrap();
    assert!(!occurrence.matches(&c.word, &hash(b"changed source"), None));
    let mut phones = c;
    phones.after.phones = Some(vec!["en/ah".into()]);
    phones.after.alphabet = Some("cmu39".into());
    phones.seal().unwrap();
    assert_eq!(
        phones.validate().unwrap_err().code,
        "PRONUNCIATION_INVENTORY_REQUIRED"
    );
}
#[test]
fn correction_language_wins_preserves_music_and_synthv_isolation() {
    let bytes = source();
    let midi = musicxml::parse(&bytes).unwrap();
    let c = correction();
    let snapshot = Snapshot::baseline(hash(&bytes), vec![c.clone()]).unwrap();
    let baseline = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    let corrected = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&snapshot),
    );
    let b = baseline.svp.unwrap();
    let p = corrected.svp.unwrap();
    for (b, p) in b.tracks[0].notes.iter().zip(&p.tracks[0].notes) {
        assert_eq!(
            (
                b.onset_ticks,
                b.duration_ticks,
                b.pitch,
                &b.source_evidence,
                &b.performance
            ),
            (
                p.onset_ticks,
                p.duration_ticks,
                p.pitch,
                &p.source_evidence,
                &p.performance
            )
        );
    }
    assert_eq!(
        p.tracks[0].notes[1].pronunciation_language,
        Some(verse_lib::pronunciation::Language::English.projected())
    );
    let svp = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Svp,
        PronunciationProfile::Automatic,
        Some(&snapshot),
    );
    let json =
        String::from_utf8(target::serialize_to(ExportTarget::Svp, &svp.svp.unwrap()).unwrap())
            .unwrap();
    for token in ["phonemizer", "fr/", "en/"] {
        assert!(!json.contains(token));
    }
}

#[test]
fn correction_never_leaks_to_another_verse_sharing_source_note_ids() {
    let xml = String::from_utf8(source()).unwrap().replace(
        "</lyric>",
        "</lyric><lyric number=\"2\"><text>bonjour</text></lyric>",
    );
    let midi = musicxml::parse(xml.as_bytes()).unwrap();
    let b = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    assert!(b.ok);
    let mut c = correction();
    c.scope = Scope::OccurrenceOnly;
    c.provenance.source_sha256 = hash(xml.as_bytes());
    c.word = b
        .pronunciation_words
        .iter()
        .find(|w| w.owner.verse == 1 && w.key == "ami")
        .unwrap()
        .clone();
    c.seal().unwrap();
    let s = Snapshot::baseline(hash(xml.as_bytes()), vec![c]).unwrap();
    let h = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&s),
    );
    let b = b.svp.unwrap();
    let h = h.svp.unwrap();
    let mut checked = 0;
    for track in &b.tracks {
        for note in &track.notes {
            let lyric = match &note.lyric {
                verse_lib::engine::projection::ProjectedLyric::Source(s)
                | verse_lib::engine::projection::ProjectedLyric::Pronounced { source: s, .. }
                | verse_lib::engine::projection::ProjectedLyric::PronouncedSplit { source: s } => s,
                _ => continue,
            };
            if lyric.verse != 2 {
                continue;
            }
            let candidate = h
                .tracks
                .iter()
                .flat_map(|t| &t.notes)
                .find(|n| n.source_evidence == note.source_evidence)
                .unwrap();
            assert_eq!(
                candidate.pronunciation_language,
                note.pronunciation_language
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
    let bytes = target::serialize_to(ExportTarget::Ustx, &h).unwrap();
    let reference = Reference::build(
        hash(xml.as_bytes()),
        hash(b"snapshot"),
        &h,
        &h_words(&midi),
        &bytes,
    )
    .unwrap();
    assert!(reference.words.iter().any(|w| w.word.owner.verse == 2));
    fn h_words(
        midi: &verse_lib::engine::midi::Midi,
    ) -> Vec<verse_lib::pronunciation::source_map::Word> {
        convert_midi_with_snapshot(
            midi,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            None,
        )
        .pronunciation_words
    }
}
#[test]
fn unrelated_music_and_ambiguous_tracks_never_enter_memory() {
    let (r, _, _) = reference();
    let mut tracks = r.tracks.clone();
    tracks[0].notes[0].tone += 1;
    let review = compare(&r, hash(b"edited"), &tracks).unwrap();
    assert!(review.proposals.is_empty());
    assert!(!review.diagnostics.is_empty());
    tracks[0].notes[1].phonemizer = Some(
        verse_lib::pronunciation::Language::English
            .native_name()
            .into(),
    );
    let review = compare(&r, hash(b"unrelated tuning plus correction"), &tracks).unwrap();
    assert_eq!(review.proposals.len(), 1);
    assert_eq!(review.proposals[0].word.key, "ami");
    let mut duplicated = r.tracks.clone();
    duplicated.push(duplicated[0].clone());
    assert!(compare(&r, hash(b"duplicate"), &duplicated)
        .unwrap()
        .proposals
        .is_empty());
}
fn calibration(threshold: f64) -> Calibration {
    Calibration {
        model_sha256: hash(b"graph"),
        external_data_sha256: hash(b"weights"),
        tokenizer_sha256: hash(b"tokenizer"),
        policy: POLICY.into(),
        bucket: "choice:3-5".into(),
        temperature: 1.0,
        threshold,
        fitted: true,
        calibration_families_sha256: hash(b"calibration families"),
    }
}
#[test]
fn confidence_contract_thresholds_finite_fields_gate_and_fallback() {
    let calibration = calibration(0.25);
    let evidence = decode(&[0.; 4], &calibration).unwrap();
    assert!(evidence.accepted(0.25));
    assert!(!evidence.accepted(0.2501));
    assert!(!evidence.accepted(0.));
    for invalid in [f32::NAN, f32::INFINITY] {
        assert!(decode(&[invalid, 0., 0., 0.], &calibration).is_err());
    }
    let mut e = evidence;
    e.gate = "unevaluated".into();
    assert!(!e.accepted(0.25));
    let json = r#"{"probabilities":[true,0,0,0],"answer_confidence":1,"gate":"passed"}"#;
    assert!(serde_json::from_str::<Evidence>(json).is_err());
    assert!(calibration
        .validate(
            &hash(b"wrong graph"),
            &hash(b"weights"),
            &hash(b"tokenizer")
        )
        .is_err());
    let bytes = source();
    let midi = musicxml::parse(&bytes).unwrap();
    let s = Snapshot::baseline(hash(&bytes), vec![]).unwrap();
    let b = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    let h = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&s),
    );
    assert_eq!(b.svp, h.svp);
}
fn field(number: u8, body: &[u8]) -> Vec<u8> {
    assert!(body.len() < 128);
    let mut data = vec![number << 3 | 2, body.len() as u8];
    data.extend(body);
    data
}
fn graph(path: &str) -> Vec<u8> {
    let mut entry = field(1, b"location");
    entry.extend(field(2, path.as_bytes()));
    let mut tensor = field(13, &entry);
    tensor.extend([14 << 3, 1]);
    let graph = field(5, &tensor);
    field(7, &graph)
}
#[test]
fn external_onnx_paths_are_checked_without_initializing_runtime() {
    assert_eq!(external_paths(&graph("model.onnx.data")).unwrap().len(), 1);
    for path in [
        "../escape.data",
        "/tmp/external",
        "unexpected.data",
        "model.onnx.data/../escape",
    ] {
        assert_eq!(
            external_paths(&graph(path)).unwrap_err().code,
            "LAYA_GRAPH_EXTERNAL_DATA"
        );
    }
    assert!(external_paths(&[0xff]).is_err());
}
#[test]
fn work_deadline_and_cancellation_are_per_operation() {
    let work = Work::new(std::time::Duration::from_secs(30));
    work.cancelled.store(true, Ordering::Relaxed);
    assert_eq!(work.check().unwrap_err().code, "LAYA_CANCELLED");
    let expired = Work {
        deadline: std::time::Instant::now() - std::time::Duration::from_secs(1),
        cancelled: Default::default(),
    };
    assert_eq!(expired.check().unwrap_err().code, "LAYA_DEADLINE");
}

#[test]
fn calibration_is_bound_to_external_weights_not_only_shared_graph() {
    let c = calibration(0.8);
    c.validate(&hash(b"graph"), &hash(b"weights"), &hash(b"tokenizer"))
        .unwrap();
    assert_eq!(
        c.validate(
            &hash(b"graph"),
            &hash(b"another checkpoint weights"),
            &hash(b"tokenizer")
        )
        .unwrap_err()
        .code,
        "LAYA_CALIBRATION_UNQUALIFIED"
    );
}

#[test]
fn same_language_attested_reading_reuses_across_compatible_songs_without_label_guessing() {
    use verse_lib::pronunciation::Language;
    let xml = String::from_utf8(source())
        .unwrap()
        .replace("bonjour", "you")
        .replace("ami", "read")
        .replace("ici", "books");
    let midi = musicxml::parse(xml.as_bytes()).unwrap();
    let baseline = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    let project = baseline.svp.as_ref().unwrap();
    let bytes = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let reference = Reference::build(
        hash(xml.as_bytes()),
        hash(b"reading-plan"),
        project,
        &baseline.pronunciation_words,
        &bytes,
    )
    .unwrap();
    let bound = reference
        .words
        .iter()
        .find(|w| w.word.key == "read")
        .unwrap();
    assert_eq!(bound.before.language, Some(Language::English));
    let mut edited = reference.tracks.clone();
    edited[bound.track].notes[bound.notes[0]].lyric = "read(2)[en/r en/iy en/d]".into();
    let mut correction = compare(&reference, hash(b"explicit dictionary reading"), &edited)
        .unwrap()
        .proposals
        .remove(0);
    assert_eq!(correction.before.language, correction.after.language);
    assert_eq!(
        correction.after.authority,
        Some(verse_lib::pronunciation::reading::authority(
            Language::English
        ))
    );
    correction.scope = Scope::CompatibleContext;
    correction.provenance.confirmed_after_listening = true;
    correction.seal().unwrap();
    correction.validate().unwrap();
    for score in [
        xml.clone(),
        xml.replace(
            "<part-name>Voice</part-name>",
            "<part-name>Another source</part-name>",
        ),
    ] {
        let midi = musicxml::parse(score.as_bytes()).unwrap();
        let snapshot =
            Snapshot::baseline(hash(score.as_bytes()), vec![correction.clone()]).unwrap();
        let outcome = convert_midi_with_snapshot(
            &midi,
            "english",
            None,
            ExportTarget::Ustx,
            PronunciationProfile::Automatic,
            Some(&snapshot),
        );
        let project = outcome.svp.unwrap();
        let note = &project.tracks[0].notes[1];
        match &note.lyric {
            verse_lib::engine::projection::ProjectedLyric::Pronounced {
                source,
                text,
                phonemes,
            } => {
                assert_eq!(source.raw, "read");
                assert_eq!(text, "read");
                assert_eq!(phonemes, "en/r en/iy en/d");
            }
            other => panic!("Reading not applied: {other:?}"),
        }
        assert!(outcome
            .tracks
            .iter()
            .flat_map(|t| &t.warnings)
            .any(|d| d.code == "PRONUNCIATION_READING_MEMORY_APPLIED"));
    }
    edited[bound.track].notes[bound.notes[0]].lyric = "read(2)".into();
    let mut unqualified = compare(&reference, hash(b"native label only"), &edited)
        .unwrap()
        .proposals
        .remove(0);
    assert!(unqualified.after.authority.is_none());
    unqualified.provenance.confirmed_after_listening = true;
    unqualified.seal().unwrap();
    unqualified.validate().unwrap();
    let snapshot = Snapshot::baseline(hash(xml.as_bytes()), vec![unqualified]).unwrap();
    let outcome = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&snapshot),
    );
    assert!(outcome
        .tracks
        .iter()
        .flat_map(|t| &t.warnings)
        .any(|d| d.code == "PRONUNCIATION_READING_AUTHORITY_UNQUALIFIED"));
    assert!(
        verse_lib::pronunciation::reading::known_reading("read", "read(1)", Language::English)
            .is_err(),
        "The pinned Verse dictionary attests read(2), not read(1)"
    );
}

#[test]
fn explicit_source_hold_is_not_a_reading_attack_but_split_is() {
    use verse_lib::engine::{midi::LyricState, projection::ProjectedLyric};
    let (_, _, outcome) = reference();
    let mut notes = outcome.svp.unwrap().tracks.remove(0).notes;
    for note in &mut notes {
        let source = match &note.lyric {
            ProjectedLyric::Source(s)
            | ProjectedLyric::Pronounced { source: s, .. }
            | ProjectedLyric::PronouncedSplit { source: s } => s.clone(),
            _ => panic!(),
        };
        note.lyric = ProjectedLyric::Source(source);
    }
    if let ProjectedLyric::Source(s) = &mut notes[1].lyric {
        s.state = LyricState::Continuation;
    }
    let word = verse_lib::pronunciation::source_map::word(
        &notes,
        &[0, 1],
        "bonjour".into(),
        vec!["bonjour".into()],
        0,
    )
    .unwrap();
    assert_eq!(word.attacks, 1);
    assert_eq!(word.members.len(), 2);
    if let ProjectedLyric::Source(s) = &mut notes[1].lyric {
        s.state = LyricState::SyllableSplit;
    }
    let word = verse_lib::pronunciation::source_map::word(
        &notes,
        &[0, 1],
        "bonjour".into(),
        vec!["bonjour".into()],
        0,
    )
    .unwrap();
    assert_eq!(word.attacks, 2);
}

#[test]
fn unknown_native_aliases_are_retained_as_proposals_but_timing_and_reuse_are_excluded() {
    use verse_lib::pronunciation::source_map::AliasOverride;
    let (reference, bytes, _) = reference();
    let mut native = parse_ustx(&bytes).unwrap();
    native[0].notes[1].alias_overrides.push(AliasOverride {
        member: 0,
        index: 1,
        phone: "bank-specific alias".into(),
    });
    let review = compare(&reference, hash(b"native alias edit"), &native).unwrap();
    assert_eq!(review.proposals.len(), 1);
    let mut correction = review.proposals[0].clone();
    assert_eq!(
        correction.after.aliases.as_ref().unwrap()[0].phone,
        "bank-specific alias"
    );
    assert!(correction.after.authority.is_none());
    correction.provenance.confirmed_after_listening = true;
    correction.seal().unwrap();
    correction.validate().unwrap();
    correction.scope = Scope::CompatibleContext;
    correction.seal().unwrap();
    assert_eq!(
        correction.validate().unwrap_err().code,
        "PRONUNCIATION_INVENTORY_REQUIRED"
    );
    let timing=String::from_utf8(bytes).unwrap().replacen("phoneme_overrides: []","phoneme_overrides:\n          - index: 0\n            offset: 7\n            preutter_delta: 2",1);
    let tracks = parse_ustx(timing.as_bytes()).unwrap();
    assert!(compare(&reference, hash(timing.as_bytes()), &tracks)
        .unwrap()
        .proposals
        .is_empty());
}

#[test]
fn observed_singer_survives_comparison_confirmation_and_json_without_qualifying_aliases() {
    let temp = Temp::new();
    let (reference, bytes, baseline) = reference();
    let singer = "Observed English voice — unknown inventories";
    let mut edited: serde_yaml::Value = serde_yaml::from_slice(&bytes).unwrap();
    edited["tracks"][0]["singer"] = serde_yaml::Value::String(singer.into());
    let singer_only = serde_yaml::to_string(&edited).unwrap();
    assert!(
        compare(
            &reference,
            hash(singer_only.as_bytes()),
            &parse_ustx(singer_only.as_bytes()).unwrap()
        )
        .unwrap()
        .proposals
        .is_empty(),
        "Assigning a singer alone is not a pronunciation correction"
    );
    edited["voice_parts"][0]["notes"][1]["phoneme_overrides"] =
        serde_yaml::from_str("- index: 1\n  phoneme: bank-specific-alias\n").unwrap();
    let corrected_path = temp.0.join("corrected.ustx");
    std::fs::write(&corrected_path, serde_yaml::to_string(&edited).unwrap()).unwrap();
    let (digest, tracks) = Reference::read_corrected(&corrected_path).unwrap();
    let mut memory = Memory::open(&temp.0).unwrap();
    memory.save_reference(&reference, &bytes).unwrap();
    let stored = memory.reference(&reference.id).unwrap();
    let review = compare(&stored, digest, &tracks).unwrap();
    assert_eq!(review.proposals.len(), 1);
    let mut correction = review.proposals[0].clone();
    let proposal = serde_json::to_value(&correction).unwrap();
    assert_eq!(proposal["provenance"]["observed_singer"], singer);
    assert!(!correction.provenance.confirmed_after_listening);
    assert!(correction.voice.is_none());
    assert_eq!(correction.provenance.symbol_validation, "unknown");
    assert!(correction.after.authority.is_none());

    correction.provenance.confirmed_after_listening = true;
    correction.seal().unwrap();
    correction.validate().unwrap();
    memory.store(&[correction.clone()], true).unwrap();
    drop(memory);
    let restarted = Memory::open(&temp.0).unwrap();
    let package = Package::confirmed(&restarted).unwrap();
    let path = temp.0.join("contribution.json");
    package.write(&path).unwrap();
    let imported = Package::read(&path).unwrap();
    assert_eq!(imported.corrections, vec![correction.clone()]);
    let exported = serde_json::to_value(&imported).unwrap();
    let record = &exported["corrections"][0];
    assert_eq!(record["provenance"]["observed_singer"], singer);
    assert!(record["voice"].is_null());
    assert!(record["provenance"].get("inventory_sha256").is_none());
    assert!(record["provenance"].get("configuration_sha256").is_none());

    let snapshot = Snapshot::baseline(hash(&source()), imported.corrections).unwrap();
    let outcome = convert_midi_with_snapshot(
        &musicxml::parse(&source()).unwrap(),
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&snapshot),
    );
    assert_eq!(
        outcome.svp, baseline.svp,
        "Observed names cannot enable raw alias reapplication"
    );
    correction.scope = Scope::CompatibleContext;
    correction.seal().unwrap();
    assert_eq!(
        correction.validate().unwrap_err().code,
        "PRONUNCIATION_INVENTORY_REQUIRED"
    );
}

#[test]
fn legacy_records_without_singer_observation_preserve_the_original_fingerprint() {
    #[derive(serde::Serialize)]
    struct LegacyProvenance<'a> {
        source_sha256: &'a str,
        export_sha256: &'a str,
        corrected_sha256: &'a str,
        confirmed_after_listening: bool,
        symbol_validation: &'a str,
        policy: &'a str,
    }
    let c = correction();
    let p = &c.provenance;
    let old = LegacyProvenance {
        source_sha256: &p.source_sha256,
        export_sha256: &p.export_sha256,
        corrected_sha256: &p.corrected_sha256,
        confirmed_after_listening: p.confirmed_after_listening,
        symbol_validation: &p.symbol_validation,
        policy: &p.policy,
    };
    let legacy_fingerprint = hash(
        &serde_json::to_vec(&(
            &c.word,
            &c.before,
            &c.after,
            &c.dialect,
            &c.accepted_variant,
            &c.scope,
            &c.target,
            &c.profile,
            &c.voice,
            &old,
        ))
        .unwrap(),
    );
    assert_eq!(c.fingerprint, legacy_fingerprint);
    let legacy_json = serde_json::to_vec(&c).unwrap();
    assert!(serde_json::to_value(&c).unwrap()["provenance"]
        .get("observed_singer")
        .is_none());
    let decoded: Correction = serde_json::from_slice(&legacy_json).unwrap();
    assert!(decoded.provenance.observed_singer.is_none());
    decoded.validate().unwrap();
    assert_eq!(decoded.fingerprint, legacy_fingerprint);
}

#[test]
fn named_reference_metadata_survives_restart_and_is_absent_from_contribution_json() {
    let temp = Temp::new();
    let (mut reference, bytes, _) = reference();
    reference.source_label = Some("Original choir.mscz".into());
    reference.export_label = Some("Choir export.ustx".into());
    reference.created_at_unix_seconds = Some(1791190000);
    {
        let mut memory = Memory::open(&temp.0).unwrap();
        memory.save_reference(&reference, &bytes).unwrap();
        memory.store(&[correction()], true).unwrap();
    }
    let memory = Memory::open(&temp.0).unwrap();
    let named = memory.references().unwrap();
    assert_eq!(
        named[0].source_label.as_deref(),
        Some("Original choir.mscz")
    );
    assert_eq!(named[0].export_label.as_deref(), Some("Choir export.ustx"));
    assert_eq!(named[0].created_at_unix_seconds, Some(1791190000));
    let json = serde_json::to_string(&Package::confirmed(&memory).unwrap()).unwrap();
    assert!(!json.contains("Original choir.mscz"));
    assert!(!json.contains("Choir export.ustx"));
}

#[test]
fn old_reading_authority_is_preserved_as_incompatible_after_upgrade() {
    let mut c = correction();
    c.after.lexical_reading = Some("ami(2)".into());
    c.accepted_variant = Some("ami(2)".into());
    c.after.phones = Some(vec!["en/r".into(), "en/iy".into(), "en/d".into()]);
    c.after.alphabet = Some("cmu39".into());
    c.after.authority = Some(format!(
        "{}:dictionary:{}",
        POLICY,
        hash(b"old pinned dictionary")
    ));
    c.seal().unwrap();
    c.validate().unwrap();
    let temp = Temp::new();
    let mut memory = Memory::open(&temp.0).unwrap();
    memory.store(&[c.clone()], true).unwrap();
    drop(memory);
    let mut memory = Memory::open(&temp.0).unwrap();
    assert!(memory.snapshot().unwrap().is_empty());
    assert_eq!(memory.history().unwrap()[0].status, "incompatible");
    assert!(memory.set_status(&c.id, "active").is_err());
}

#[test]
#[allow(clippy::approx_constant)] // Exact Python decimal rounding fixtures, not approximations of pi.
fn python_four_decimal_rounding_ties_and_gate_boundaries() {
    use verse_lib::pronunciation::laya::round_answer_confidence as rounded;
    // Python 3.13 round() receipts, including exact binary half-even ties and
    // the multiplication-created tie at 0.52345.
    for (value, expected, below, above) in [
        (0.52345, 0.5234, 0.5234, 0.5235),
        (0.03125, 0.0312, 0.0312, 0.0313),
        (0.15625, 0.1562, 0.1562, 0.1563),
        (0.71875, 0.7188, 0.7187, 0.7188),
        (0.50005, 0.5, 0.5, 0.5001),
        (0.99995, 1.0, 0.9999, 1.0),
    ] {
        assert_eq!(rounded(value), expected);
        assert_eq!(rounded(f64::from_bits(value.to_bits() - 1)), below);
        assert_eq!(rounded(f64::from_bits(value.to_bits() + 1)), above);
        if value >= 0.25 {
            let remaining = (1.0 - value) / 3.0;
            let evidence = Evidence {
                probabilities: [value, remaining, remaining, remaining],
                answer_confidence: expected,
                gate: "passed".into(),
            };
            assert!(evidence.accepted(f64::from_bits(expected.to_bits() - 1)));
            assert!(evidence.accepted(expected));
            assert!(!evidence.accepted(f64::from_bits(expected.to_bits() + 1)));
        }
    }
    let c = calibration(0.25);
    let decoded = decode(&[0.0; 4], &c).unwrap();
    assert_eq!(decoded.answer_confidence, rounded(decoded.probabilities[0]));
    assert!(decoded.accepted(c.threshold));
    assert!(!decoded.accepted(f64::from_bits(c.threshold.to_bits() + 1)));
    // Pinned NumPy float32 softmax at the reference's decimal boundary.
    let decoded = decode(&[1.192_481_f32, 0.0, 0.0, 0.0], &calibration(0.5235)).unwrap();
    assert_eq!(decoded.answer_confidence, 0.5235);
    assert!(decoded.accepted(0.5235));
    assert!(!decoded.accepted(f64::from_bits(0.5235f64.to_bits() + 1)));
}

#[test]
fn ten_thousand_record_snapshot_is_indexed_shared_and_conflict_preserving() {
    use verse_lib::pronunciation::memory::CorrectionSet;
    let base = correction();
    let mut records = Vec::new();
    for index in 0..10_000 {
        let mut c = base.clone();
        let key = format!("word{index}");
        c.word.key = key.clone();
        c.word.context[c.word.context_target] = key.clone();
        c.word.original = vec![key.clone()];
        c.before.lexical_reading = Some(key.clone());
        c.after.lexical_reading = Some(key);
        c.seal().unwrap();
        records.push(c);
    }
    let set = std::sync::Arc::new(CorrectionSet::new(records).unwrap());
    let a = Snapshot::with_corrections(hash(b"song-a"), set.clone()).unwrap();
    let b = Snapshot::with_corrections(hash(b"song-b"), set.clone()).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a.corrections, &b.corrections));
    for index in 0..2_000 {
        let word = &set[index * 5].word;
        let c = set
            .find(word, &hash(b"other source"), None)
            .unwrap()
            .unwrap();
        assert_eq!(c.word.key, word.key);
    }
    let mut conflicting = base.clone();
    conflicting.after.language = Some(verse_lib::pronunciation::Language::Spanish);
    conflicting.after.phonemizer = verse_lib::pronunciation::Language::Spanish
        .native_name()
        .into();
    conflicting.seal().unwrap();
    let conflict = CorrectionSet::new(vec![base.clone(), conflicting]).unwrap();
    assert!(conflict
        .find(&base.word, &base.provenance.source_sha256, None)
        .is_err());
    let temp = Temp::new();
    Memory::open(&temp.0).unwrap();
    let connection = rusqlite::Connection::open(temp.0.join("pronunciation.sqlite3")).unwrap();
    connection.execute_batch("WITH RECURSIVE rows(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM rows WHERE x<10001) INSERT INTO corrections SELECT CAST(x AS TEXT),CAST(x AS TEXT),'{}','revoked' FROM rows;").unwrap();
    drop(connection);
    assert_eq!(
        Memory::open(&temp.0)
            .unwrap()
            .trusted_snapshot()
            .err()
            .unwrap()
            .code,
        "PRONUNCIATION_MEMORY_LIMIT"
    );
}

#[test]
fn local_asset_gate_checks_receipt_bytes_weights_and_every_mandatory_file() {
    use verse_lib::pronunciation::assets::{self, Asset, Manifest, Qualification};
    let temp = Temp::new();
    std::fs::create_dir(temp.0.join("tokenizer")).unwrap();
    let mut files = Vec::new();
    let mut put = |name: &str, bytes: Vec<u8>| {
        std::fs::write(temp.0.join(name), &bytes).unwrap();
        let asset = Asset {
            path: name.into(),
            bytes: bytes.len() as u64,
            sha256: hash(&bytes),
        };
        files.push(asset.clone());
        asset
    };
    let graph = put("model.onnx", graph("model.onnx.data"));
    let weights = put(
        "model.onnx.data",
        b"contract-fixture-not-trained-weights".to_vec(),
    );
    let tokenizer = put("tokenizer/tokenizer.json", b"{}".to_vec());
    put("tokenizer/tokenizer_config.json", b"{}".to_vec());
    put("rl_agent_config.json", b"{}".to_vec());
    let runtime = put(
        "native.dylib",
        b"contract-fixture-never-initialized".to_vec(),
    );
    let mut c = calibration(0.8);
    c.model_sha256 = graph.sha256.clone();
    c.external_data_sha256 = weights.sha256.clone();
    c.tokenizer_sha256 = tokenizer.sha256.clone();
    let calibration = put("calibration.json", serde_json::to_vec(&c).unwrap());
    let identities = serde_json::json!({"schema_version":1,"policy":POLICY,"target":"test-cpu","graph_sha256":graph.sha256,"external_data_sha256":weights.sha256,"tokenizer_sha256":tokenizer.sha256,"calibration_sha256":calibration.sha256,"runtime_sha256":runtime.sha256});
    let mut evidence = Vec::new();
    for (name, observation) in [
        (
            "rights.json",
            serde_json::json!({"kind":"rights","checkpoint_license_sha256":hash(b"license"),"tokenizer_permission_sha256":hash(b"permission"),"review_sha256":hash(b"review")}),
        ),
        (
            "parity.json",
            serde_json::json!({"kind":"offline_parity","token_cases":6,"shape_cases":4,"max_logit_error":0.0001,"max_action_logit_error":0.0001,"network_attempts":0,"missing_corrupt_cases":7,"immutable_assets":true}),
        ),
        (
            "resources.json",
            serde_json::json!({"kind":"resources","peak_rss_bytes":1000,"package_bytes":1000,"cold_seconds":1.0,"warm_p95_seconds":0.1,"hardware":"contract-only fixture","threads":2}),
        ),
    ] {
        let mut data = identities.clone();
        data["observation"] = observation;
        let bytes = serde_json::to_vec(&data).unwrap();
        evidence.push(put(name, bytes));
    }
    let qualification = Qualification {
        schema_version: 1,
        policy: POLICY.into(),
        target: "test-cpu".into(),
        graph_sha256: graph.sha256,
        external_data_sha256: weights.sha256,
        tokenizer_sha256: tokenizer.sha256,
        calibration_sha256: calibration.sha256,
        runtime_sha256: runtime.sha256,
        evidence,
        rights_evidence: "rights.json".into(),
        offline_parity_evidence: "parity.json".into(),
        independent_evaluation_evidence: None,
        resource_evidence: "resources.json".into(),
    };
    let receipt = put(
        "qualification.json",
        serde_json::to_vec(&qualification).unwrap(),
    );
    let manifest = Manifest {
        schema_version: 1,
        policy: POLICY.into(),
        target: "test-cpu".into(),
        runtime_path: "native.dylib".into(),
        files,
        calibration: c,
        redistribution_qualified: true,
        telemetry_free_build: true,
        native_parity_qualified: true,
        independent_benefit_qualified: true,
        qualification_receipt_sha256: Some(receipt.sha256),
        fusion_weight: 1.0,
        max_tokens: 1024,
        max_rows: 2,
        threads: 2,
        deadline_ms: 30000,
    };
    std::fs::write(
        temp.0.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    // Contract-only data can exercise preflight; it is never a native session
    // or real model qualification result. Missing independent benefit refuses
    // production even with every unchecked boolean set true.
    assets::validate(&temp.0, "test-cpu", false).unwrap();
    assert_eq!(
        assets::validate(&temp.0, "test-cpu", true)
            .unwrap_err()
            .code,
        "LAYA_QUALIFICATION_MISSING"
    );
    for mandatory in [
        "model.onnx",
        "model.onnx.data",
        "tokenizer/tokenizer.json",
        "tokenizer/tokenizer_config.json",
        "rl_agent_config.json",
        "calibration.json",
        "native.dylib",
        "qualification.json",
    ] {
        let path = temp.0.join(mandatory);
        let before = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(
            assets::validate(&temp.0, "test-cpu", false).is_err(),
            "{mandatory}"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            assets::validate(&temp.0, "test-cpu", false).is_err(),
            "{mandatory}"
        );
        std::fs::write(path, before).unwrap();
    }
    let mut changed = manifest.clone();
    changed.qualification_receipt_sha256 = Some(hash(b"unchecked hash"));
    std::fs::write(
        temp.0.join("manifest.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert_eq!(
        assets::validate(&temp.0, "test-cpu", false)
            .unwrap_err()
            .code,
        "LAYA_QUALIFICATION_INVALID"
    );
    let mut changed = manifest.clone();
    let replacement = b"different-checkpoint-same-graph";
    std::fs::write(temp.0.join("model.onnx.data"), replacement).unwrap();
    let sidecar = changed
        .files
        .iter_mut()
        .find(|a| a.path == "model.onnx.data")
        .unwrap();
    sidecar.bytes = replacement.len() as u64;
    sidecar.sha256 = hash(replacement);
    std::fs::write(
        temp.0.join("manifest.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert_eq!(
        assets::validate(&temp.0, "test-cpu", false)
            .unwrap_err()
            .code,
        "LAYA_CALIBRATION_UNQUALIFIED"
    );
}

#[test]
fn changed_phonemizer_does_not_relabel_stale_hint_alphabet() {
    use verse_lib::pronunciation::{
        source_map::{reading, NativeNote},
        Language,
    };
    let mut note = NativeNote {
        alias_overrides: Vec::new(),
        position: 0,
        duration: 480,
        tone: 60,
        lyric: "ciel[en/s en/iy en/l]".into(),
        phonemizer: Some(Language::English.native_name().into()),
    };
    let before = reading(&note, Language::English.phonemizer()).unwrap();
    assert_eq!(before.alphabet.as_deref(), Some("cmu39"));
    note.phonemizer = Some(Language::French.native_name().into());
    let after = reading(&note, Language::English.phonemizer()).unwrap();
    assert_eq!(after.language, Some(Language::French));
    assert_eq!(after.alphabet, before.alphabet);
    assert_eq!(after.phones, before.phones);
    note.lyric = "word[fr/s en/iy]".into();
    assert!(reading(&note, "").unwrap().alphabet.is_none());
    note.lyric = "word[a S u]".into();
    assert!(reading(&note, "").unwrap().alphabet.is_none());
    let (r, _, _) = reference();
    let bound = &r.words[1];
    let mut edited = r.tracks.clone();
    edited[bound.track].notes[bound.notes[0]].phonemizer =
        Some(Language::English.native_name().into());
    let review = compare(&r, hash(b"language-only-edit"), &edited).unwrap();
    let correction = &review.proposals[0];
    assert!(correction.after.phones.is_none());
    assert!(correction.after.alphabet.is_none());
}

#[cfg(feature = "laya-native")]
#[test]
#[ignore = "Requires exact local tokenizer and pinned Python parity fixtures"]
fn native_tokenization_matches_pinned_python_build_sequence() {
    let root = PathBuf::from(
        std::env::var("VERSE_LAYA_PARITY_ASSETS")
            .expect("exact local parity asset directory required"),
    );
    let mut tokenizer =
        tokenizers::Tokenizer::from_file(root.join("tokenizer/tokenizer.json")).unwrap();
    tokenizer.with_truncation(None).unwrap();
    tokenizer.with_padding(None);
    let fixtures: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("verse-parity-fixtures.json")).unwrap())
            .unwrap();
    assert_eq!(fixtures["policy"], POLICY);
    for case in fixtures["cases"].as_array().unwrap() {
        let encoded = verse_lib::pronunciation::laya::native::encode(
            &tokenizer,
            case["state"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(encoded.input_ids).unwrap(),
            case["input_ids"]
        );
        assert_eq!(
            serde_json::to_value(encoded.marker_pos).unwrap(),
            case["marker_pos"]
        );
    }
    assert!(
        verse_lib::pronunciation::laya::native::encode(&tokenizer, &"word ".repeat(1024)).is_err()
    );
}

#[test]
fn concurrent_fresh_and_outdated_database_opens_serialize_migrations() {
    for outdated in [false, true] {
        let temp = Temp::new();
        if outdated {
            let conn = rusqlite::Connection::open(temp.0.join("pronunciation.sqlite3")).unwrap();
            conn.execute_batch("CREATE TABLE corrections(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL UNIQUE, payload TEXT NOT NULL, status TEXT NOT NULL); CREATE TABLE export_references(id TEXT PRIMARY KEY,export_hash TEXT NOT NULL,source_hash TEXT NOT NULL,payload TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let gate = barrier.clone();
                    let root = &temp.0;
                    scope.spawn(move || {
                        gate.wait();
                        Memory::open(root).unwrap().snapshot().unwrap()
                    })
                })
                .collect();
            for thread in threads {
                assert!(thread.join().unwrap().is_empty());
            }
        });
        let conn = rusqlite::Connection::open(temp.0.join("pronunciation.sqlite3")).unwrap();
        assert_eq!(
            conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            2
        );
    }
}

#[test]
fn exchange_serialized_budget_is_symmetric_at_the_boundary() {
    use verse_lib::pronunciation::exchange::{FORMAT, MAX_EXCHANGE_BYTES};
    let temp = Temp::new();
    let mut package = Package {
        format: FORMAT.into(),
        schema_version: 1,
        verse_version: "v".into(),
        pronunciation_policy: POLICY.into(),
        created_at_unix_seconds: 0,
        corrections: vec![],
    };
    let used = serde_json::to_vec_pretty(&package).unwrap().len();
    package
        .verse_version
        .push_str(&"x".repeat(MAX_EXCHANGE_BYTES as usize - used));
    let path = temp.0.join("boundary.json");
    package.write(&path).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_EXCHANGE_BYTES);
    assert_eq!(
        Package::read(&path).unwrap().verse_version,
        package.verse_version
    );
    package.verse_version.push('x');
    let over = temp.0.join("over.json");
    assert_eq!(
        package.write(&over).unwrap_err().code,
        "PRONUNCIATION_FILE_LIMIT"
    );
    assert!(!over.exists());
}

#[test]
fn geometry_lookalike_does_not_displace_joint_track_text_proof() {
    let (reference, _, _) = reference();
    let mut actual = reference.tracks[0].clone();
    actual.notes[2].tone += 1; // unrelated musical edit
    actual.notes[1].phonemizer = Some(
        verse_lib::pronunciation::Language::English
            .native_name()
            .into(),
    );
    let mut lookalike = reference.tracks[0].clone();
    lookalike.notes[0].lyric = "unrelated".into();
    lookalike.notes[2].lyric = "text".into();
    let review = compare(&reference, hash(b"lookalike"), &[lookalike, actual]).unwrap();
    assert_eq!(review.proposals.len(), 1);
    assert_eq!(review.proposals[0].word.key, "ami");
    assert_eq!(
        review.proposals[0].after.language,
        Some(verse_lib::pronunciation::Language::English)
    );
}

#[test]
fn unsorted_multilingual_reference_keeps_each_source_head_language() {
    use verse_lib::engine::projection::PronunciationLanguage as L;
    let (_, _, mut outcome) = reference();
    let project = outcome.svp.as_mut().unwrap();
    project.tracks[0].notes[0].pronunciation_language = Some(L::English);
    project.tracks[0].notes[2].pronunciation_language = Some(L::Portuguese);
    project.tracks[0].notes.reverse();
    let bytes = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let reference = Reference::build(
        hash(&source()),
        hash(b"unsorted"),
        project,
        &outcome.pronunciation_words,
        &bytes,
    )
    .unwrap();
    assert_eq!(
        reference
            .words
            .iter()
            .find(|w| w.word.key == "bonjour")
            .unwrap()
            .before
            .language,
        Some(verse_lib::pronunciation::Language::English)
    );
    assert_eq!(
        reference
            .words
            .iter()
            .find(|w| w.word.key == "ici")
            .unwrap()
            .before
            .language,
        Some(verse_lib::pronunciation::Language::Portuguese)
    );
}

#[test]
fn analysis_preflights_mandatory_original_reference_budget() {
    let temp = Temp::new();
    let (_, _, mut outcome) = reference();
    outcome.svp.as_mut().unwrap().tracks[0].name = "x".repeat(32 * 1024 * 1024);
    let mut snapshot = Snapshot::baseline(hash(&source()), vec![]).unwrap();
    snapshot.memory_root = Some(temp.0.clone());
    let error = verse_lib::pronunciation::with_snapshot(std::sync::Arc::new(snapshot), || {
        verse_lib::pronunciation::observe_projection(&outcome, ExportTarget::Ustx)
    })
    .unwrap_err();
    assert_eq!(error.code, "PRONUNCIATION_FILE_LIMIT");
    assert!(verse_lib::pronunciation::observed_projection().is_none());
}

#[test]
fn unchanged_lexical_key_attested_phone_correction_is_applied() {
    use verse_lib::pronunciation::{reading, Language};
    let xml = String::from_utf8(source())
        .unwrap()
        .replace("bonjour", "the")
        .replace("ami", "cat")
        .replace("ici", "here");
    let midi = musicxml::parse(xml.as_bytes()).unwrap();
    let base = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        None,
    );
    let project = base.svp.as_ref().unwrap();
    let bytes = target::serialize_to(ExportTarget::Ustx, project).unwrap();
    let mut before: serde_yaml::Value = serde_yaml::from_slice(&bytes).unwrap();
    before["voice_parts"][0]["notes"][1]["lyric"] = "cat[en/k en/ih en/t]".into();
    let original = serde_yaml::to_string(&before).unwrap();
    let reference = Reference::build(
        hash(xml.as_bytes()),
        hash(b"same-key"),
        project,
        &base.pronunciation_words,
        original.as_bytes(),
    )
    .unwrap();
    let hint = reading::known_reading("cat", "cat", Language::English).unwrap();
    let mut edited = reference.tracks.clone();
    edited[0].notes[1].lyric = format!("cat[{hint}]");
    let mut correction = compare(&reference, hash(b"phone edit"), &edited)
        .unwrap()
        .proposals
        .remove(0);
    assert_eq!(
        correction.before.lexical_reading,
        correction.after.lexical_reading
    );
    assert_eq!(correction.word.attacks, 1);
    correction.provenance.confirmed_after_listening = true;
    correction.seal().unwrap();
    let snapshot = Snapshot::baseline(hash(xml.as_bytes()), vec![correction]).unwrap();
    let result = convert_midi_with_snapshot(
        &midi,
        "english",
        None,
        ExportTarget::Ustx,
        PronunciationProfile::Automatic,
        Some(&snapshot),
    );
    assert!(result
        .tracks
        .iter()
        .flat_map(|t| &t.warnings)
        .any(|d| d.code == "PRONUNCIATION_READING_MEMORY_APPLIED"));
    assert_eq!(result.pronunciation_words[1].attacks, 1);
    assert_eq!(result.svp.unwrap(), *project);
}
