//! Engine parity tests on real song files, plus pure score-structure tests.
//!
//! The song fixtures are copyrighted karaoke/score files and are therefore NOT
//! committed to the public repository (see .gitignore). File-based tests are
//! explicitly ignored unless requested with `cargo test --test parity -- --ignored`;
//! once requested, every missing fixture is a hard failure. The unroll tests
//! below always run.
use std::collections::HashMap;
use verse_lib::engine::convert::{
    convert_auto, convert_bytes, convert_midi_with, convert_midi_with_target, ConvertOutcome,
};
use verse_lib::engine::midi;
use verse_lib::engine::target;

fn read_fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!("required private fixture is unavailable at {path}: {error}")
    })
}

fn conv(name: &str) -> ConvertOutcome {
    convert_bytes(&read_fixture(name), "english")
}

fn conv_auto(name: &str) -> ConvertOutcome {
    convert_auto(&read_fixture(name), "english")
}

/// A Standard MIDI File assembled from raw track chunks, so the golden test
/// below needs no committed fixture. Same construction as
/// `source_fidelity.rs`'s `smf`, widened to format 1 for several tracks.
fn smf(tracks: &[&[u8]]) -> Vec<u8> {
    let mut data = b"MThd\0\0\0\x06\0\x01".to_vec();
    data.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    data.extend_from_slice(&480u16.to_be_bytes());
    for track in tracks {
        data.extend_from_slice(b"MTrk");
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(track);
    }
    data
}

/// Two sung tracks, a tempo change, a meter change at a bar boundary, an
/// untexted note, and a third track with notes but no lyric evidence anywhere.
fn golden_source() -> Vec<u8> {
    let sung: Vec<u8> = vec![
        0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // 120 bpm at tick 0
        0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, // 3/4 at tick 0
        0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // lyric "let"
        0x00, 0x90, 60, 100, // C4 on at 0
        0x83, 0x60, 0x80, 60, 0, // off at 480
        0x00, 0xff, 0x05, 0x02, b'i', b't', // lyric "it" at 480
        0x00, 0x90, 62, 100, // D4 on at 480
        0x81, 0x70, 0x80, 62, 0, // off at 720
        0x81, 0x70, 0x90, 64, 100, // E4 on at 960, no lyric of its own
        0x83, 0x60, 0x80, 64, 0, // off at 1440
        0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, // 4/4 at tick 1440, bar 1
        0x00, 0xff, 0x51, 0x03, 0x06, 0x1a, 0x80, // 150 bpm at tick 1440
        0x00, 0xff, 0x2f, 0x00,
    ];
    let second_voice: Vec<u8> = vec![
        0x00, 0xff, 0x05, 0x04, b's', b'i', b'n', b'g', // lyric "sing"
        0x00, 0x90, 67, 80, // G4 on at 0
        0x87, 0x40, 0x80, 67, 0, // off at 960
        0x00, 0xff, 0x2f, 0x00,
    ];
    // No lyric evidence anywhere on this track, so it is never projected as a
    // vocal lane. The colours and display order below must therefore follow the
    // projected sequence, not the source track index.
    let instrumental: Vec<u8> = vec![
        0x00, 0x90, 48, 90, //
        0x83, 0x60, 0x80, 48, 0, //
        0x00, 0xff, 0x2f, 0x00,
    ];
    smf(&[&sung, &second_voice, &instrumental])
}

/// [`golden_source`] with the untexted E4 given a word of its own.
///
/// Nothing is then left for the untexted split to move, so this source projects
/// exactly as release 0.4.9 projected it — which is what lets
/// [`a_fully_texted_source_still_writes_release_0_4_9_bytes`] keep pinning
/// 0.4.9's bytes after the split changed what [`golden_source`] produces.
fn fully_texted_source() -> Vec<u8> {
    let sung: Vec<u8> = vec![
        0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // 120 bpm at tick 0
        0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, // 3/4 at tick 0
        0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', // lyric "let"
        0x00, 0x90, 60, 100, // C4 on at 0
        0x83, 0x60, 0x80, 60, 0, // off at 480
        0x00, 0xff, 0x05, 0x02, b'i', b't', // lyric "it" at 480
        0x00, 0x90, 62, 100, // D4 on at 480
        0x81, 0x70, 0x80, 62, 0, // off at 720
        // The one difference from `golden_source`: this E4 is texted.
        0x81, 0x70, 0xff, 0x05, 0x05, b's', b'h', b'i', b'n', b'e', // lyric at 960
        0x00, 0x90, 64, 100, // E4 on at 960
        0x83, 0x60, 0x80, 64, 0, // off at 1440
        0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, // 4/4 at tick 1440, bar 1
        0x00, 0xff, 0x51, 0x03, 0x06, 0x1a, 0x80, // 150 bpm at tick 1440
        0x00, 0xff, 0x2f, 0x00,
    ];
    let second_voice: Vec<u8> = vec![
        0x00, 0xff, 0x05, 0x04, b's', b'i', b'n', b'g', // lyric "sing"
        0x00, 0x90, 67, 80, // G4 on at 0
        0x87, 0x40, 0x80, 67, 0, // off at 960
        0x00, 0xff, 0x2f, 0x00,
    ];
    let instrumental: Vec<u8> = vec![
        0x00, 0x90, 48, 90, //
        0x83, 0x60, 0x80, 48, 0, //
        0x00, 0xff, 0x2f, 0x00,
    ];
    smf(&[&sung, &second_voice, &instrumental])
}

/// Release 0.4.9's `.svp` for [`golden_source`], byte for byte.
const GOLDEN_SVP: &str = r#"{"version":113,"time":{"meter":[{"denominator":4,"index":0,"numerator":3},{"denominator":4,"index":1,"numerator":4}],"tempo":[{"bpm":120.0,"position":0},{"bpm":150.0,"position":2116800000}]},"renderConfig":{"aspirationFormat":"noAspiration","bitDepth":16,"destination":"./","exportMixDown":true,"filename":"untitled","numChannels":1,"sampleRate":44100},"tracks":[{"name":"Track 0","dispColor":"ff7db235","dispOrder":0,"renderEnabled":true,"mixer":{"gainDecibel":0.0,"pan":0.0,"mute":false,"solo":false,"display":true},"mainRef":{"audio":{"filename":"","duration":0.0},"database":{"name":"","language":"","phoneset":""},"dictionary":"","voice":{},"groupID":"00000000-0000-4000-8000-000000000000","isInstrumental":false,"blickOffset":0},"mainGroup":{"name":"main","uuid":"00000000-0000-4000-8000-000000000000","parameters":{"breathiness":{"mode":"cubic","points":[]},"gender":{"mode":"cubic","points":[]},"loudness":{"mode":"cubic","points":[]},"pitchDelta":{"mode":"cubic","points":[]},"tension":{"mode":"cubic","points":[]},"vibratoEnv":{"mode":"cubic","points":[]},"voicing":{"mode":"cubic","points":[]}},"notes":[{"attributes":{},"duration":705600000,"lyrics":"let","onset":0,"phonemes":"","pitch":60},{"attributes":{},"duration":352800000,"lyrics":"it","onset":705600000,"phonemes":"","pitch":62},{"attributes":{},"duration":705600000,"lyrics":"","onset":1411200000,"phonemes":"","pitch":64}]},"groups":[]},{"name":"Track 1","dispColor":"ff4a90d9","dispOrder":1,"renderEnabled":true,"mixer":{"gainDecibel":0.0,"pan":0.0,"mute":false,"solo":false,"display":true},"mainRef":{"audio":{"filename":"","duration":0.0},"database":{"name":"","language":"","phoneset":""},"dictionary":"","voice":{},"groupID":"00000001-0000-4000-8000-000000000000","isInstrumental":false,"blickOffset":0},"mainGroup":{"name":"main","uuid":"00000001-0000-4000-8000-000000000000","parameters":{"breathiness":{"mode":"cubic","points":[]},"gender":{"mode":"cubic","points":[]},"loudness":{"mode":"cubic","points":[]},"pitchDelta":{"mode":"cubic","points":[]},"tension":{"mode":"cubic","points":[]},"vibratoEnv":{"mode":"cubic","points":[]},"voicing":{"mode":"cubic","points":[]}},"notes":[{"attributes":{},"duration":1411200000,"lyrics":"sing","onset":0,"phonemes":"","pitch":67}]},"groups":[]}]}"#;

/// Release 0.4.9's bytes for [`golden_source`] with its one untexted note
/// removed — which is exactly what the current rule writes for that source, and
/// nothing else. Derived rather than re-recorded so it is still 0.4.9 that is
/// pinned: the note count of `Track 0` falls by one and no other byte may move.
fn golden_svp_without_the_untexted_note() -> String {
    let expected = GOLDEN_SVP.replace(
        r#",{"attributes":{},"duration":705600000,"lyrics":"","onset":1411200000,"phonemes":"","pitch":64}"#,
        "",
    );
    assert_ne!(expected, GOLDEN_SVP, "the deletion must actually apply");
    expected
}

/// Pins the whole Synthesizer V output, not just its shape. `GOLDEN_SVP` was
/// taken from release 0.4.9 before the target seam existed, so this test is the
/// standing proof that no later work drifts a single `.svp` byte: a blick, a
/// colour, a display order, a group UUID, a marker, `version: 113`, the render
/// config, or the field order they are written in.
///
/// It runs on [`fully_texted_source`] rather than [`golden_source`] because the
/// untexted split now moves `golden_source`'s E4 to a companion lane. The
/// expectation is still 0.4.9's own value, reached by the one substitution
/// texting that note causes and nothing else: the note keeps its onset, its
/// duration, its pitch, its track and its position in the track, so no other
/// byte of the release output can move. Deriving it instead of re-recording it
/// is the point — a value copied out of the code under test would pin that code
/// to itself.
#[test]
fn a_fully_texted_source_still_writes_release_0_4_9_bytes() {
    let expected = GOLDEN_SVP.replace(
        r#"{"attributes":{},"duration":705600000,"lyrics":"","onset":1411200000,"phonemes":"","pitch":64}"#,
        r#"{"attributes":{},"duration":705600000,"lyrics":"shine","onset":1411200000,"phonemes":"","pitch":64}"#,
    );
    assert_ne!(expected, GOLDEN_SVP, "the substitution must actually apply");

    let outcome = convert_bytes(&fully_texted_source(), "japanese");
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(outcome.placed, 4);
    let projected = outcome.svp.expect("a projection");
    // Nothing to move, so no companion: still only the two tracks with source
    // lyric evidence, and neither of them muted.
    assert_eq!(projected.tracks.len(), 2);
    assert!(projected.tracks.iter().all(|track| !track.muted));
    let svp = target::svp::serialize(&projected).expect("480 PPQ is exactly representable");
    let json = String::from_utf8(serde_json::to_vec(&svp).expect("serializes"))
        .expect("SVP JSON is UTF-8");
    assert_eq!(json, expected);
}

/// The same source with the E4 left untexted: that note is not written into the
/// project at all, and nothing else about the file changes.
///
/// Asserted against `GOLDEN_SVP` by deleting exactly one note object from it, so
/// the diff this rule is allowed to make is stated here in full — one note gone,
/// every colour, display order, group UUID and blick untouched. A wordless note
/// is not vocal material: OpenUtau's phonemizer marks an empty lyric `error`, so
/// writing them produced a project that reads as a failed conversion. The note
/// is preserved in the bundle's source and audible in its rendered stem.
#[test]
fn an_untexted_note_is_left_out_of_the_project() {
    let expected = golden_svp_without_the_untexted_note();
    let outcome = convert_bytes(&golden_source(), "japanese");
    assert!(outcome.ok, "{:?}", outcome.msg);
    // The projected lyric count is about words, not notes, so leaving a wordless
    // note out must not change it.
    assert_eq!(outcome.placed, 3);
    let projected = outcome.svp.expect("a projection");
    assert_eq!(
        projected
            .tracks
            .iter()
            .map(|track| (track.name.as_str(), track.muted, track.notes.len()))
            .collect::<Vec<_>>(),
        vec![("Track 0", false, 2), ("Track 1", false, 1)],
    );

    let svp = target::svp::serialize(&projected).expect("480 PPQ is exactly representable");
    let json = String::from_utf8(serde_json::to_vec(&svp).expect("serializes"))
        .expect("SVP JSON is UTF-8");
    assert_eq!(json, expected);
    assert!(
        !json.contains(r#""lyrics":"""#),
        "no note reaches the project without a word"
    );
}

/// The OpenUtau project for the very same [`golden_source`], byte for byte.
///
/// The untexted E4 at tick 960 is not here: a note the source never texted is
/// not written into a vocal project, because OpenUtau has no way to sing one —
/// its phonemizer marks an empty lyric `error`. It is preserved in the source
/// and in the stem rendered from it.
const GOLDEN_USTX: &str = concat!(
    "ustx_version: \"0.6\"\n",
    "resolution: 480\n",
    "bpm: 120\n",
    "beat_per_bar: 3\n",
    "beat_unit: 4\n",
    "time_signatures: [{bar_position: 0, beat_per_bar: 3, beat_unit: 4}, {bar_position: 1, beat_per_bar: 4, beat_unit: 4}]\n",
    "tempos: [{position: 0, bpm: 120}, {position: 1440, bpm: 150}]\n",
    "expressions: {}\n",
    "tracks:\n",
    "  - phonemizer: \"OpenUtau.Core.DefaultPhonemizer\"\n",
    "    track_name: \"Track 0\"\n",
    "    mute: false\n",
    "    solo: false\n",
    "    volume: 0\n",
    "  - phonemizer: \"OpenUtau.Core.DefaultPhonemizer\"\n",
    "    track_name: \"Track 1\"\n",
    "    mute: false\n",
    "    solo: false\n",
    "    volume: 0\n",
    "voice_parts:\n",
    "  - name: \"Track 0\"\n",
    "    track_no: 0\n",
    "    position: 0\n",
    "    notes:\n",
    "      - position: 0\n",
    "        duration: 480\n",
    "        tone: 60\n",
    "        lyric: \"let\"\n",
    "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
    "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
    "        phoneme_expressions: []\n",
    "        phoneme_overrides: []\n",
    "      - position: 480\n",
    "        duration: 240\n",
    "        tone: 62\n",
    "        lyric: \"it\"\n",
    "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
    "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
    "        phoneme_expressions: []\n",
    "        phoneme_overrides: []\n",
    "    curves: []\n",
    "  - name: \"Track 1\"\n",
    "    track_no: 1\n",
    "    position: 0\n",
    "    notes:\n",
    "      - position: 0\n",
    "        duration: 960\n",
    "        tone: 67\n",
    "        lyric: \"sing\"\n",
    "        pitch: {data: [{x: -1, y: 0, shape: io}, {x: 1, y: 0, shape: io}], snap_first: true}\n",
    "        vibrato: {length: 0, period: 175, depth: 25, in: 10, out: 10, shift: 0, drift: 0}\n",
    "        phoneme_expressions: []\n",
    "        phoneme_overrides: []\n",
    "    curves: []\n",
    "wave_parts: []\n",
);

/// Pins the whole OpenUtau output for the same programmatic source the `.svp`
/// golden above uses, so one projection pins both targets and neither can drift
/// the other. Any change to a tick, a marker, a structural default, the
/// `ustx_version`, the obsolete downgrade scalars or the field order fails here.
#[test]
fn the_openutau_bytes_are_pinned_for_the_same_source() {
    let outcome = convert_midi_with_target(
        &midi::parse(&golden_source()).expect("valid MIDI"),
        "japanese",
        None,
        target::ExportTarget::Ustx,
    );
    assert!(outcome.ok, "{:?}", outcome.msg);
    assert_eq!(outcome.placed, 3);
    let projected = outcome.svp.expect("a projection");
    // Only the two lanes with lyric evidence. The untexted E4 is not a lane and
    // not a note here: OpenUtau cannot sing an empty lyric.
    assert_eq!(projected.tracks.len(), 2);
    let bytes = target::serialize_to(target::ExportTarget::Ustx, &projected)
        .expect("480 PPQ is exactly representable");
    let yaml = String::from_utf8(bytes).expect("USTX is UTF-8");
    assert_eq!(yaml, GOLDEN_USTX);
    // The tempo and the meter change both survive, which is what the 0.6 floor
    // buys: below it, `Ustx.Load` replaces both lists with one entry each.
    assert!(yaml.contains("{position: 1440, bpm: 150}"));
    assert!(yaml.contains("{bar_position: 1, beat_per_bar: 4, beat_unit: 4}"));
    // No lyric was invented for the untexted note, and nothing was written for
    // the third source track, which carries notes but no lyric evidence.
    assert_eq!(
        yaml.matches("lyric: \"\"").count(),
        0,
        "an empty lyric is what OpenUtau marks `error`, so none is written"
    );
    assert!(!yaml.contains("lyric: \"a\""));
    assert_eq!(yaml.matches("track_name:").count(), 2);
    // No projected lane opens silent: every one of them sings.
    assert_eq!(yaml.matches("mute: true").count(), 0);
    assert_eq!(yaml.matches("mute: false").count(), 2);
}

/// One projection, two targets, one file each — and the same bytes as the
/// per-target goldens above. This is the seam: nothing above `serialize_to`
/// decides anything format-specific.
#[test]
fn one_projection_writes_both_targets_from_the_same_analysis() {
    let outcome = convert_bytes(&golden_source(), "japanese");
    let projected = outcome.svp.expect("a projection");
    let svp = target::serialize_to(target::ExportTarget::Svp, &projected).expect("representable");
    let ustx = target::serialize_to(target::ExportTarget::Ustx, &projected).expect("representable");
    assert_eq!(
        String::from_utf8(svp).expect("SVP JSON is UTF-8"),
        golden_svp_without_the_untexted_note()
    );
    assert_eq!(String::from_utf8(ustx).expect("USTX is UTF-8"), GOLDEN_USTX);
}

/// The reason the analysis gate has to take the caller's target: `480 = 2^5*3*5`
/// cannot express a septuplet, so OpenUtau refuses a strict subset of what
/// Synthesizer V blicks accept. Gating on one target for the other would clear a
/// source the other must refuse, and the refusal would resurface at export.
#[test]
fn analysis_refuses_for_openutau_exactly_what_openutau_cannot_write() {
    // PPQ 448 = 64 * 7. A note onset at tick 64 is a whole number of blicks
    // (64 * 705_600_000 / 448 = 100_800_000) but not of 480ths of a quarter.
    let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x01\xc0".to_vec();
    let track: Vec<u8> = vec![
        0x40, 0xff, 0x05, 0x03, b'l', b'e', b't', // lyric "let" at tick 64
        0x00, 0x90, 60, 100, // C4 on at 64
        0x83, 0x40, 0x80, 60, 0, // off at 512, so a 448-tick quarter
        0x00, 0xff, 0x2f, 0x00,
    ];
    data.extend_from_slice(b"MTrk");
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(&track);
    let parsed = midi::parse(&data).expect("valid MIDI");

    let synthesizer_v =
        convert_midi_with_target(&parsed, "english", None, target::ExportTarget::Svp);
    assert!(
        synthesizer_v.ok,
        "blicks represent this source exactly: {:?}",
        synthesizer_v.msg
    );

    let openutau = convert_midi_with_target(&parsed, "english", None, target::ExportTarget::Ustx);
    assert!(!openutau.ok, "480 ticks per quarter cannot place tick 64");
    assert!(openutau.svp.is_none(), "nothing may be written");
    let message = openutau.msg.expect("a refusal message");
    // Named for the target, not for timing: OpenUtau also refuses a syllable
    // split, a chord in one monophonic lane and a held syllable across a gap, so
    // "fix the timing" would send the user after the wrong thing. Synthesizer V
    // keeps 0.4.9's timing wording, which
    // `the_synthesizer_v_bytes_stay_identical_to_release_0_4_9` and the analysis
    // test in `lib.rs` both still pin.
    assert_eq!(
        message,
        "the source cannot be projected safely to OpenUtau: note onset on source track \
         midi-track-0 at MIDI tick 64 cannot be represented exactly in OpenUtau's 480 ticks per \
         quarter with PPQ 448"
    );
    // Parsing succeeded, so the refusal must not erase the source evidence.
    assert_eq!(
        openutau.topology.parts.len(),
        synthesizer_v.topology.parts.len()
    );
}

/// A caller that names no target keeps release 0.4.9's verdict and bytes, which
/// is what lets the webview stay untouched.
#[test]
fn naming_no_target_is_synthesizer_v() {
    let parsed = midi::parse(&golden_source()).expect("valid MIDI");
    let unnamed = convert_midi_with(&parsed, "japanese", None);
    let named =
        convert_midi_with_target(&parsed, "japanese", None, target::ExportTarget::default());
    assert!(unnamed.ok && named.ok);
    assert_eq!(unnamed.svp, named.svp);
    assert_eq!(target::ExportTarget::default(), target::ExportTarget::Svp);
    let bytes = target::serialize_to(
        target::ExportTarget::default(),
        &unnamed.svp.expect("a projection"),
    )
    .expect("representable");
    assert_eq!(
        String::from_utf8(bytes).expect("SVP JSON is UTF-8"),
        golden_svp_without_the_untexted_note()
    );
}

/// The converter validates the meter before it walks the tracks and the timing
/// grid after, so a source that fails both must report the meter. That order is
/// observable message text: moving the timing gate ahead of the meter check
/// would silently change which reason the user is given.
#[test]
fn a_source_failing_both_meter_and_timing_still_reports_the_meter() {
    // PPQ 1024, so a one-tick duration misses the blick grid, and a 3/4 -> 4/4
    // change at tick 1, which is inside the first bar.
    let mut data = b"MThd\0\0\0\x06\0\0\0\x01\x04\x00".to_vec();
    let track: Vec<u8> = vec![
        0x00, 0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, // 3/4 at tick 0
        0x00, 0xff, 0x05, 0x03, b'l', b'e', b't', //
        0x00, 0x90, 60, 100, //
        0x01, 0x80, 60, 0, // duration 1 tick, not blick-exact
        0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, // 4/4 at tick 1, mid-bar
        0x00, 0xff, 0x2f, 0x00,
    ];
    data.extend_from_slice(b"MTrk");
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(&track);

    let outcome = convert_bytes(&data, "english");
    assert!(!outcome.ok);
    assert!(outcome.svp.is_none());
    assert_eq!(
        outcome.msg.as_deref(),
        Some(
            "MIDI meter cannot be projected safely: time signature change at MIDI tick 1 \
             (event:midi-track-0:4) falls inside a 3/4 measure; Synthesizer V meter changes \
             require a measure boundary"
        )
    );
}

#[test]
fn unroll_repeat_with_voltas() {
    use verse_lib::engine::midi::{unroll, MeasureMarks};
    // M0 | M1(||: ) M2 | M3(volta 1, :||) | M4(volta 2) | M5
    let mut m = vec![MeasureMarks::default(); 6];
    m[1].start_repeat = true;
    m[3].volta = Some(vec![1]);
    m[3].end_repeat = 2;
    m[4].volta = Some(vec![2]);
    let order = unroll(&m).expect("repeat structure is representable");
    // pass 1: 0 1 2 3 -> back; pass 2: 1 2 (3 skipped) 4 5
    assert_eq!(
        order,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (1, 1),
            (2, 1),
            (4, 0),
            (5, 0)
        ]
    );
}

#[test]
fn unroll_ds_al_coda() {
    use verse_lib::engine::midi::{unroll, Jump, MeasureMarks};
    // M0(segno) M1 M2(To Coda) M3(D.S. al Coda) M4(Coda) M5
    let mut m = vec![MeasureMarks::default(); 6];
    m[0].segno = true;
    m[2].to_coda = true;
    m[3].jump = Some(Jump::DsAlCoda);
    m[4].coda = true;
    let order = unroll(&m).expect("D.S. al Coda structure is representable");
    // 0 1 2 3 -> D.S. -> 0 1 2 -> To Coda -> 4 5
    assert_eq!(
        order,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (1, 1),
            (2, 1),
            (4, 0),
            (5, 0)
        ]
    );
}

#[test]
fn unroll_dc_al_fine() {
    use verse_lib::engine::midi::{unroll, Jump, MeasureMarks};
    // M0 M1(Fine) M2 M3(D.C. al Fine)
    let mut m = vec![MeasureMarks::default(); 4];
    m[1].fine = true;
    m[3].jump = Some(Jump::DcAlFine);
    let order = unroll(&m).expect("D.C. al Fine structure is representable");
    // 0 1 2 3 -> D.C. -> 0 1(Fine, stop)
    assert_eq!(order, vec![(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (1, 1)]);
}

#[test]
#[ignore = "requires the private hound_dog.kar fixture"]
fn hound_dog_multitrack() {
    let data = read_fixture("hound_dog.kar");
    let parsed = midi::parse(&data).expect("valid KAR/SMF");
    let automatic = convert_midi_with(&parsed, "english", None);
    assert!(automatic.ok, "Hound Dog must convert");
    // One report entry per projection lane. A source track that sounds two
    // notes at once becomes two monophonic lanes, so lanes outnumber the source
    // voices `n_tracks` counts; every lane must still be inventoried.
    assert_eq!(
        automatic.tracks.len(),
        parsed.tracks.len(),
        "every projection lane is inventoried"
    );
    assert_eq!(automatic.n_tracks, parsed.topology.voice_count());
    assert_eq!(
        automatic.placed, 244,
        "the complete lyrics stream must bind to its unique timing-compatible melody"
    );
    let automatic_vocal: Vec<_> = automatic
        .tracks
        .iter()
        .filter(|track| track.role == "vocal")
        .collect();
    assert_eq!(automatic_vocal.len(), 1);
    assert_eq!(automatic_vocal[0].placed, 244);

    // The override targets a track by report index. Splitting a source track
    // into monophonic voices moves those indices, so the lead vocal is found by
    // name instead of by a number that no longer means the same lane.
    let lead = automatic
        .tracks
        .iter()
        .find(|track| track.track.starts_with("Lead Vox"))
        .expect("the lead vocal track is inventoried")
        .id;
    let r = convert_midi_with(&parsed, "english", Some(&HashMap::from([(lead, true)])));
    assert_eq!(
        r.placed, 244,
        "the proven external binding remains source-backed under an explicit override; report: {:?}",
        r.tracks
    );
    let vox = r
        .tracks
        .iter()
        .find(|t| t.role == "vocal")
        .expect("one singing track");
    assert_eq!(vox.placed, 244);
    let svp = target::svp::serialize(&r.svp.unwrap()).unwrap();
    assert_eq!(
        svp.tracks
            .iter()
            .flat_map(|track| &track.main_group.notes)
            .filter(|note| !note.lyrics.is_empty())
            .count(),
        244
    );
}

#[test]
#[ignore = "requires the private help.kar fixture"]
fn help_kar_binds_words_only_to_the_unique_lead_track() {
    let data = read_fixture("help.kar");
    let parsed = midi::parse(&data).expect("valid KAR/SMF");
    let automatic = convert_midi_with(&parsed, "english", None);
    assert_eq!(
        automatic
            .tracks
            .iter()
            .filter(|track| track.role == "vocal")
            .count(),
        1,
        "only the complete timing-compatible melody may receive the Words stream"
    );
    assert_eq!(automatic.placed, 314);
    // The user may explicitly export Lead + Harm 1 + Harm 2 as vocal notes,
    // but that never transfers the separate Words track into them.
    // Overrides address report indices, which move once a source track is
    // split into monophonic voices. Select the three sung parts by name so the
    // test states its intent rather than a numbering.
    let chosen: HashMap<usize, bool> = automatic
        .tracks
        .iter()
        .filter(|track| {
            ["Lead", "Harm 1", "Harm 2"]
                .iter()
                .any(|name| track.track.contains(name))
        })
        .map(|track| (track.id, true))
        .collect();
    let r = convert_midi_with(&parsed, "english", Some(&chosen));
    assert!(r.ok);
    // Every source voice owns at least one projection lane; a voice that
    // sounds two notes at once owns several.
    assert!(
        r.tracks.len() >= r.n_tracks,
        "every source voice keeps at least one inventoried lane"
    );
    let vocal: Vec<_> = r.tracks.iter().filter(|t| t.role == "vocal").collect();
    assert_eq!(
        vocal.len(),
        chosen.len(),
        "every selected part sings, one lane at a time; report: {:?}",
        r.tracks
    );
    let lead = vocal
        .iter()
        .find(|t| t.track.contains("Lead"))
        .expect("Lead track");
    assert_eq!(lead.placed, 314);
    assert!(vocal
        .iter()
        .any(|t| t.track.contains("Harm 1") && t.placed == 0));
    assert!(vocal
        .iter()
        .any(|t| t.track.contains("Harm 2") && t.placed == 0));
    // the 3 explicitly selected singing tracks are the only audible SVP tracks;
    // any track beyond them is a muted companion one of them shed, and a lane
    // that binds no word at all (both Harm tracks) is never split.
    let svp = target::svp::serialize(&r.svp.unwrap()).unwrap();
    assert_eq!(
        svp.tracks.iter().filter(|track| !track.mixer.mute).count(),
        3
    );
    assert!(svp
        .tracks
        .iter()
        .filter(|track| track.mixer.mute)
        .all(|track| track.name.ends_with(" — untexted notes")));
    assert!(svp.tracks[0].name.contains("Lead") || svp.tracks[0].name.contains("Harm"));
    assert_eq!(
        svp.tracks
            .iter()
            .flat_map(|track| &track.main_group.notes)
            .filter(|note| !note.lyrics.is_empty())
            .count(),
        314
    );
    assert!(svp
        .tracks
        .iter()
        .filter(|track| track.name.contains("Harm"))
        .flat_map(|track| &track.main_group.notes)
        .all(|note| note.lyrics.is_empty()));
}

#[test]
#[ignore = "requires the private help.mxl fixture"]
fn musicxml_help_lyrics() {
    let r = conv_auto("help.mxl");
    assert!(r.ok, "the MusicXML must convert: {:?}", r.msg);
    assert!(
        r.tracks.iter().any(|t| t.role == "vocal"),
        "at least one singing track"
    );
    assert!(r.placed > 100, "many syllables placed, got {}", r.placed);
    let svp = target::svp::serialize(&r.svp.unwrap()).unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group
                .notes
                .iter()
                .any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the MusicXML unrolling
    assert!(
        has("changed"),
        "verse 2 must be sung (.mxl repeats unrolled)"
    );
}

#[test]
#[ignore = "requires the private help.mscz fixture"]
fn musescore_mscz_native() {
    // The user's primary format: the .mscz must convert natively.
    let r = conv_auto("help.mscz");
    assert!(r.ok, "the .mscz must convert: {:?}", r.msg);
    let vocal = r.tracks.iter().filter(|t| t.role == "vocal").count();
    assert!(vocal >= 3, "the 3 voices must sing, got {}", vocal);
    assert!(r.placed > 300, "many syllables placed, got {}", r.placed);
    let svp = target::svp::serialize(&r.svp.unwrap()).unwrap();
    let has = |w: &str| {
        svp.tracks.iter().any(|tr| {
            tr.main_group
                .notes
                .iter()
                .any(|n| n.lyrics.to_lowercase().contains(w))
        })
    };
    assert!(has("help"), "the real lyrics (Help) must appear");
    // "changed" only exists in verse 2 -> proves the repeat unrolling
    assert!(
        has("changed"),
        "verse 2 must be sung on the 2nd pass of the repeat"
    );
    // part names must come from longName (not "Track N")
    assert!(
        svp.tracks.iter().any(|tr| {
            tr.name.starts_with("Mi-que") || tr.name.starts_with("Do") || tr.name.starts_with('T')
        }),
        "real part names expected"
    );
}

#[test]
#[ignore = "requires the private help.mscz and help.mxl fixtures"]
fn the_same_score_projects_identically_from_mscz_and_mxl() {
    // The two fixtures are the same piece exported twice, so the two paths must
    // agree note for note. Before ties were merged and repeats were computed
    // score-wide, the .mscz gave 250/162/173 notes against the .mxl's
    // 214/210/231 — and both dropped the syllable sitting on a tied note.
    let from_mscz = conv_auto("help.mscz");
    let from_mxl = conv_auto("help.mxl");
    let voices = |outcome: &ConvertOutcome| {
        target::svp::serialize(outcome.svp.as_ref().expect("conversion succeeds"))
            .expect("exactly representable")
            .tracks
            .iter()
            .map(|track| {
                track
                    .main_group
                    .notes
                    .iter()
                    .map(|note| (note.onset, note.duration, note.pitch, note.lyrics.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let mscz = voices(&from_mscz);
    let mxl = voices(&from_mxl);
    assert_eq!(
        mscz.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![214, 210, 235],
        "each staff must sustain its ties and unroll the repeat; the counts \
         predate the untexted rule, so a change here means notes the source \
         texts stopped being projected"
    );
    assert_eq!(
        mscz, mxl,
        "the MuseScore projection must match the MusicXML one note for note"
    );

    // No word may be lost on either path. "ate" sits on a tied note and used to
    // vanish with the merge.
    let words = |projection: &Vec<Vec<(i64, i64, u8, String)>>| {
        projection
            .iter()
            .flatten()
            .map(|note| note.3.trim().to_lowercase())
            .filter(|text| !text.is_empty())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(
        words(&mscz).contains("ate"),
        "a syllable on a tied note is sung"
    );
    assert_eq!(words(&mscz), words(&mxl));

    // The verses are stacked two deep only where their words differ; the
    // refrain that follows them is written on a single lyric line for both
    // passes. Reading that single line as "verse 2 is silent here" deleted the
    // whole refrain from the second pass — ninety words across the three
    // voices, twenty seconds of the piece with notes but nothing to sing.
    let sung = |projection: &Vec<Vec<(i64, i64, u8, String)>>| {
        projection
            .iter()
            .map(|track| {
                track
                    .iter()
                    .filter(|note| !note.3.trim().is_empty())
                    .count()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sung(&mscz),
        vec![197, 194, 212],
        "each pass sings its own verse and the refrain both passes share"
    );
    assert_eq!(sung(&mscz), sung(&mxl));
    assert_eq!(
        mscz[0].iter().filter(|note| note.3 == "bam").count(),
        18,
        "the refrain is sung on the repeat, not only on the first pass"
    );
}

#[test]
#[ignore = "requires the private queen.kar fixture"]
fn queen_lyrics_without_source_melody_do_not_create_c4() {
    let r = conv("queen.kar");
    assert!(r.ok, "a lyric-only karaoke file must still be accepted");
    // Every source voice owns at least one projection lane; a voice that
    // sounds two notes at once owns several.
    assert!(
        r.tracks.len() >= r.n_tracks,
        "every source voice keeps at least one inventoried lane"
    );
    assert!(
        r.tracks.iter().all(|t| t.role != "vocal_synth"),
        "the converter must not invent a synthetic melody track"
    );
    let svp = target::svp::serialize(&r.svp.expect("a valid, possibly empty SVP project"))
        .expect("exactly representable");
    assert!(
        svp.tracks
            .iter()
            .flat_map(|track| track.main_group.notes.iter())
            .all(|note| note.pitch != 60 || !note.lyrics.is_empty()),
        "no lyric-only token may become an invented C4"
    );
}

#[test]
#[ignore = "requires the private help.mscz fixture"]
fn help_mscz_styled_names_are_not_fused() {
    let r = conv_auto("help.mscz");
    assert!(r.ok, "help.mscz must convert: {:?}", r.msg);
    // <longName>Batterie ou<br/>persussions<br/>corporelles</longName>:
    // <br/> must become a space, never fuse the words.
    assert!(
        r.tracks
            .iter()
            .any(|t| t.track.contains("ou persussions corporelles")),
        "multi-line longName must be collapsed with spaces, got: {:?}",
        r.tracks.iter().map(|t| t.track.clone()).collect::<Vec<_>>()
    );
    assert!(
        r.tracks.iter().all(|t| !t.track.contains("oupersussions")),
        "words fused across <br/>"
    );
}

#[test]
#[ignore = "requires the private help.mxl fixture"]
fn help_mxl_converts() {
    let r = conv_auto("help.mxl");
    assert!(r.ok, "help.mxl must convert: {:?}", r.msg);
    assert!(r.placed > 0, "lyrics must be placed");
}

/// Syllables a Soft Karaoke file actually asks to be sung, one list per text
/// track. Control records (`@…`), line/paragraph markers and punctuation-only
/// tokens are not words.
fn kar_syllable_streams(data: &[u8]) -> Vec<Vec<String>> {
    let parsed = midi::parse_with_karaoke_profile(data).expect("valid KAR");
    parsed
        .tracks
        .iter()
        .map(|track| {
            track
                .events
                .iter()
                .filter_map(|event| match &event.kind {
                    // Generic Text is only karaoke lyrics when this very track
                    // proves the Soft Karaoke profile, exactly as production
                    // qualifies it. Counting unqualified metadata as required
                    // syllables would fail the test for words nobody sings.
                    midi::Kind::Text(text)
                        if track.text_profile == midi::MidiTextProfile::KaraokeLyrics =>
                    {
                        Some(text.text.clone())
                    }
                    midi::Kind::Lyrics(lyric) => match &lyric.state {
                        midi::LyricState::Text(text) => Some(text.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .filter(|text| !text.starts_with('@'))
                .map(|text| {
                    text.replace(['\r', '\n'], "")
                        .trim_start_matches(['\\', '/'])
                        .trim()
                        .to_string()
                })
                .filter(|text| !text.is_empty() && text.chars().any(|c| c != '.'))
                .collect()
        })
        .collect()
}

/// Splitting a lane must move notes, never lose them.
///
/// The sibling of [`assert_no_syllable_is_lost`], one level down: that one proves
/// no *word* is lost on the way to the project, this one proves no *note* is. It
/// runs on sources built here rather than on a fixture, so it gates CI instead of
/// waiting for a private file, and it compares against the projection with the
/// split disabled — reconstructed by folding every companion back into the lane
/// it came from, which is the exact inverse of what the split does.
/// Leaving a note out must be a decision about that note, never a shortcut that
/// also loses a sung one.
///
/// Runs on sources built here rather than on a private fixture, so it gates CI,
/// and checks the two properties that matter: every note still in the project
/// carries a word, and every note that left was one the source never texted.
#[test]
fn only_notes_the_source_never_texted_are_left_out() {
    let sources: Vec<(&str, Vec<u8>)> = vec![
        ("golden", golden_source()),
        ("fully texted", fully_texted_source()),
        ("melisma and repeat", MELISMA_AND_REPEAT.as_bytes().to_vec()),
    ];
    for (name, data) in sources {
        // `convert_auto`, not `convert_bytes`: two MIDIs and one MuseScore
        // score, and the rule must behave the same on both parsers.
        let outcome = convert_auto(&data, "english");
        assert!(outcome.ok, "{name}: {:?}", outcome.msg);
        let projected = outcome.svp.expect("a projection");
        for lane in &projected.tracks {
            assert!(!lane.muted, "{name}: no projected lane opens silent");
            assert!(
                lane.notes.iter().all(|note| note.lyric.is_sung()),
                "{name}: {} holds a note with nothing to sing",
                lane.name
            );
        }
        assert!(
            !projected
                .tracks
                .iter()
                .any(|lane| lane.name.contains("untexted")),
            "{name}: no lane is created for notes that are not sung"
        );
    }

    // The melisma specifically: `A` is held across the note that follows it, so
    // that note carries no text of its own and yet must not be treated as
    // untexted. Leaving it out would shorten a word the score sustains.
    let outcome = convert_auto(MELISMA_AND_REPEAT.as_bytes(), "english");
    let projected = outcome.svp.expect("a projection");
    assert_eq!(
        projected.tracks[0]
            .notes
            .iter()
            .map(|note| (note.onset_ticks, note.pitch))
            .collect::<Vec<_>>(),
        vec![
            (0, 60),
            (960, 62),
            (1920, 64),
            (3840, 60),
            (4800, 62),
            (5760, 64),
        ],
        "the held note of a melisma stays, on both repeat passes"
    );
}

/// A bare note after a syllable is not a melisma unless the source says so.
///
/// Standard MIDI cannot state a continuation, so a syllable sustained across
/// several notes is written as one lyric event and several bare notes — exactly
/// how a syllable followed by unsung notes is written. Verse has never guessed
/// between the two readings: it wrote those notes with no lyric before the
/// companion lane existed, and it moves them to the companion now. Same reading,
/// different track.
///
/// Pinned because the opposite reading is tempting and would invent a hold the
/// source never stated. [`MELISMA_AND_REPEAT`] is the contrasting case: a source
/// that *does* state the extension keeps every held note on the sung lane.
#[test]
fn a_bare_midi_note_after_a_syllable_is_untexted_and_not_a_melisma() {
    let data = smf(&[&[
        0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // 120 bpm
        0x00, 0xff, 0x05, 0x01, b'A', // lyric "A"
        0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, // C4 0..480
        0x00, 0x90, 62, 100, 0x83, 0x60, 0x80, 62, 0, // D4 480..960, bare
        0x00, 0x90, 64, 100, 0x83, 0x60, 0x80, 64, 0, // E4 960..1440, bare
        0x00, 0xff, 0x05, 0x03, b'm', b'e', b'n', // lyric "men" at 1440
        0x00, 0x90, 65, 100, 0x83, 0x60, 0x80, 65, 0, // F4 1440..1920
        0x00, 0xff, 0x2f, 0x00,
    ]]);
    let outcome = convert_auto(&data, "english");
    assert!(outcome.ok, "{:?}", outcome.msg);
    let svp = target::svp::serialize(&outcome.svp.expect("a projection"))
        .expect("480 PPQ is exactly representable");
    assert_eq!(svp.tracks.len(), 1, "one lane, and nothing muted beside it");
    assert_eq!(
        svp.tracks[0]
            .main_group
            .notes
            .iter()
            .map(|note| (note.pitch, note.lyrics.as_str()))
            .collect::<Vec<_>>(),
        vec![(60, "A"), (65, "men")],
        "the two bare notes are not sung, so they are not written"
    );
    // No hold was invented on the way out, in either target's vocabulary, and no
    // note reached the project without a word.
    let json = serde_json::to_string(&svp).expect("serializes");
    assert!(!json.contains(r#""lyrics":"-""#));
    assert!(!json.contains(r#""lyrics":"+""#));
    assert!(!json.contains(r#""lyrics":"""#));
}

/// One voice holding a syllable across several notes, under a repeat, beside a
/// note the score leaves wordless. The melisma must survive the split whole.
const MELISMA_AND_REPEAT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<museScore version="3.02">
  <Score>
    <Division>480</Division>
    <Part><trackName>Voice</trackName><Staff id="1"/></Part>
    <Staff id="1">
      <Measure><startRepeat/><voice>
        <Chord><durationType>half</durationType>
          <Lyrics><text>A</text><ticks>1920</ticks></Lyrics>
          <Note><pitch>60</pitch></Note>
        </Chord>
        <Chord><durationType>half</durationType>
          <Note><pitch>62</pitch></Note>
        </Chord>
      </voice></Measure>
      <Measure><voice>
        <Chord><durationType>half</durationType>
          <Lyrics><text>men</text></Lyrics>
          <Note><pitch>64</pitch></Note>
        </Chord>
        <Chord><durationType>half</durationType>
          <Note><pitch>65</pitch></Note>
        </Chord>
      </voice><endRepeat>2</endRepeat></Measure>
    </Staff>
  </Score>
</museScore>"#;

/// Every syllable the source writes must reach the project. A KAR may transcribe
/// the same passage in two competing text tracks, so the fullest stream is the
/// reference: adding them together counts a duplicate as a loss.
fn assert_no_syllable_is_lost(fixture: &str) {
    let data = read_fixture(fixture);
    let reference = kar_syllable_streams(&data)
        .into_iter()
        .max_by_key(Vec::len)
        .unwrap_or_default();
    assert!(
        !reference.is_empty(),
        "{fixture} must carry singable syllables"
    );

    let outcome = convert_bytes(&data, "english");
    assert!(outcome.ok, "{fixture}: {:?}", outcome.msg);
    let mut projected: HashMap<&str, usize> = HashMap::new();
    let project = target::svp::serialize(outcome.svp.as_ref().expect("valid SVP"))
        .expect("exactly representable");
    for note in project
        .tracks
        .iter()
        .flat_map(|track| track.main_group.notes.iter())
    {
        let lyric = note.lyrics.trim();
        if !lyric.is_empty() {
            *projected.entry(lyric).or_default() += 1;
        }
    }
    let mut missing: Vec<&str> = Vec::new();
    let mut needed: HashMap<&str, usize> = HashMap::new();
    for syllable in &reference {
        *needed.entry(syllable.as_str()).or_default() += 1;
    }
    for (syllable, count) in needed {
        if projected.get(syllable).copied().unwrap_or(0) < count {
            missing.push(syllable);
        }
    }
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "{fixture} drops syllables the source writes: {missing:?}"
    );
}

#[test]
#[ignore = "requires the private KAR fixtures"]
fn no_karaoke_syllable_is_lost_on_the_way_to_the_project() {
    // The counters said these files were complete while a whole refrain line
    // was missing from one of them, so the oracle is the words themselves.
    for fixture in ["help.kar", "hound_dog.kar"] {
        assert_no_syllable_is_lost(fixture);
    }
}
