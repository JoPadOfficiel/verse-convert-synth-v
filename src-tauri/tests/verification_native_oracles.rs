//! Independent written fade geometry for the pinned native consumer, including
//! short floor-free fades and a niente endpoint at the final sounding tick.
use verse_lib::engine::{
    convert::convert_midi_with_target,
    musicxml,
    target::{ustx, ExportTarget},
};

fn sampled(curve: &ustx::UstxCurve, tick: i32) -> i32 {
    match curve.xs.binary_search(&tick) {
        Ok(i) => curve.ys[i],
        Err(i) if i > 0 && i < curve.xs.len() => {
            let left = i - 1;
            let phase = f64::from(tick - curve.xs[left]) / f64::from(curve.xs[i] - curve.xs[left]);
            (f64::from(curve.ys[left]) + phase * f64::from(curve.ys[i] - curve.ys[left]))
                .round_ties_even() as i32
        }
        _ => panic!("oracle tick is outside the emitted curve"),
    }
}

#[test]
fn emit_native_verification_fade_oracles() {
    for duration in [10, 480] {
        for direction in ["in", "out"] {
            let (opening, closing) = if direction == "out" {
                ("<direction><direction-type><dynamics><mf/></dynamics><wedge type=\"diminuendo\" number=\"1\"/></direction-type></direction>",
                 "<direction><direction-type><wedge type=\"stop\" number=\"1\" niente=\"yes\"/></direction-type></direction>")
            } else {
                ("<direction><direction-type><wedge type=\"crescendo\" number=\"1\" niente=\"yes\"/></direction-type></direction>",
                 "<direction><direction-type><wedge type=\"stop\" number=\"1\"/><dynamics><mf/></dynamics></direction-type></direction>")
            };
            let xml = format!(
                r#"<score-partwise version="4.0"><part-list><score-part id="P1"><part-name>Voice</part-name></score-part></part-list><part id="P1"><measure><attributes><divisions>480</divisions></attributes>{opening}<note><pitch><step>C</step><octave>4</octave></pitch><duration>{duration}</duration><lyric><text>la</text></lyric></note>{closing}</measure></part></score-partwise>"#
            );
            let source = musicxml::parse(xml.as_bytes()).unwrap();
            let out = convert_midi_with_target(&source, "english", None, ExportTarget::Ustx);
            assert!(out.ok, "{duration}/{direction}: {:?}", out.msg);
            let model = ustx::serialize(out.svp.as_ref().unwrap()).unwrap();
            assert_eq!(model.voice_parts.len(), 1);
            assert_eq!(model.voice_parts[0].notes.len(), 1);
            assert_eq!(model.voice_parts[0].notes[0].position, 0);
            assert_eq!(model.voice_parts[0].notes[0].duration, duration);
            let curve = model.voice_parts[0]
                .curves
                .iter()
                .find(|c| c.abbr == "dyn")
                .unwrap();
            // Written mf is 0 dB (gain 1). Niente uses a linear gain ramp;
            // expected values never consult the converter's intensity evaluator.
            let gains: Vec<_> = (0..=duration)
                .map(|tick| {
                    f64::from(if direction == "out" {
                        duration - tick
                    } else {
                        tick
                    }) / f64::from(duration)
                })
                .collect();
            let mut floor_samples = 0;
            for (tick, intended) in gains.iter().copied().enumerate() {
                let actual = sampled(curve, tick as i32);
                if intended == 0.0 {
                    assert_eq!(actual, -240, "exact authored zero endpoint");
                } else {
                    assert!(actual > -240, "positive gain must never become mute");
                    let expected_db = 20.0 * intended.log10();
                    let needs_floor = (expected_db * 10.0).round() <= -240.0;
                    let supported_db = if needs_floor {
                        assert!(tick > 0 && tick < duration as usize);
                        floor_samples += 1;
                        assert_eq!(actual, -239, "authorized floor is exactly DYN -239");
                        -23.9
                    } else {
                        expected_db
                    };
                    assert!((f64::from(actual) * 0.1 - supported_db).abs() <= 0.10001);
                }
            }
            assert_eq!(floor_samples > 0, duration == 480);
            if let Ok(directory) = std::env::var("VERSE_SCORE_PERFORMANCE_PROBE_DIR") {
                let directory = std::path::Path::new(&directory);
                std::fs::create_dir_all(directory).unwrap();
                let name = format!("score-verification-fade-{direction}-{duration}");
                std::fs::write(
                    directory.join(format!("{name}.ustx")),
                    ustx::to_yaml(&model),
                )
                .unwrap();
                let oracle = serde_json::json!({
                    "schema":1, "noteCount":1, "ticksPerQuarter":480,
                    "startTick":0, "endTick":duration, "tickStep":1, "gains":gains,
                    "nienteFadeIntervals":[{"startTick":0,"endTick":duration,"direction":direction}],
                });
                std::fs::write(
                    directory.join(format!("{name}.expected.json")),
                    serde_json::to_vec(&oracle).unwrap(),
                )
                .unwrap();
            }
        }
    }
}
