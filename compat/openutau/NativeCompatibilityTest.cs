using System;
using System.IO;
using System.Linq;
using System.Reflection;
using OpenUtau.Api;
using OpenUtau.Core;
using OpenUtau.Core.DiffSinger;
using OpenUtau.Core.Format;
using OpenUtau.Core.Ustx;
using OpenUtau.Core.Util;
using Xunit;

namespace OpenUtau.App {
    [CollectionDefinition("Verse native compatibility", DisableParallelization = true)]
    public class NativeCompatibilityCollection { }

    [Collection("Verse native compatibility")]
    public class NativeCompatibilityTest {
        // Explicit synthetic source input, not an acoustic or private-score oracle.
        // No voicebank, graph, model, renderer, or generated automation is supplied.
        internal const string SourceYaml = @"
name: Verse native compatibility source
ustx_version: '0.6'
expressions:
  src: {name: source expression, abbr: src, type: Curve, min: -100, max: 100, default_value: 0}
tempos:
  - {position: 0, bpm: 110}
  - {position: 1920, bpm: 55}
  - {position: 2400, bpm: 110}
time_signatures:
  - {bar_position: 0, beat_per_bar: 4, beat_unit: 4}
tracks:
  - track_name: Source Part A
    phonemizer: OpenUtau.Core.DiffSinger.DiffSingerEnglishPhonemizer
  - track_name: Source Part B
    phonemizer: OpenUtau.Core.DiffSinger.DiffSingerPortuguesePhonemizer
voice_parts:
  - name: Source Part A voice 1
    track_no: 0
    position: 480
    duration: 3840
    notes:
      - {position: 0, duration: 480, tone: 64, lyric: 'ciel[fr/s fr/y fr/ae fr/l]', phonemizer: 'DiffSinger French Millefeuille Phonemizer', pitch: {data: [], snap_first: false}, vibrato: {}, phoneme_expressions: [{abbr: vol, index: 0, value: 78}]}
      - {position: 480, duration: 480, tone: 65, lyric: '+', pitch: {data: [], snap_first: false}, vibrato: {}}
      - {position: 960, duration: 480, tone: 65, lyric: '+~', pitch: {data: [], snap_first: false}, vibrato: {}}
      - {position: 1440, duration: 480, tone: 67, lyric: 'read[en/r en/iy en/d]', phonemizer: 'DiffSinger English Phonemizer', pitch: {data: [], snap_first: false}, vibrato: {}}
      - {position: 1920, duration: 480, tone: 69, lyric: 'hola[o l a]', phonemizer: 'DiffSinger Spanish Phonemizer', pitch: {data: [], snap_first: false}, vibrato: {}}
    curves:
      - {abbr: src, xs: [0, 480, 960], ys: [-7, -3, -9]}
      - {abbr: dyn, xs: [0, 480, 960], ys: [-30, -15, -30]}
      - {abbr: pitd, xs: [1440, 1680, 1920], ys: [0, 25, 0]}
  - name: Source Part B voice 1
    track_no: 1
    position: 480
    duration: 3840
    notes:
      - {position: 0, duration: 480, tone: 60, lyric: 'acho[a S u]', phonemizer: 'DiffSinger Portuguese Phonemizer', pitch: {data: [], snap_first: false}, vibrato: {}}
wave_parts: []
";

        internal sealed class SourceCopy : IDisposable {
            public string Root { get; }
            public string Input => Path.Combine(Root, "source.ustx");
            public string Output => Path.Combine(Root, "saved.ustx");
            public SourceCopy(string yaml) {
                var testRoot = Environment.GetEnvironmentVariable("VERSE_OPENUTAU_TEST_ROOT");
                Assert.False(string.IsNullOrEmpty(testRoot));
                // PathManager's test-only seam runs before any user-data access.
                Assert.Equal(Path.Combine(Path.GetFullPath(testRoot!), "data"), PathManager.Inst.DataPath);
                Root = Path.Combine(testRoot!, "project-" + Guid.NewGuid());
                Directory.CreateDirectory(Root);
                File.WriteAllText(Input, yaml);
                foreach (var type in new[] {
                    typeof(DiffSingerFrenchMillfeuillePhonemizer), typeof(DiffSingerEnglishPhonemizer),
                    typeof(DiffSingerSpanishPhonemizer), typeof(DiffSingerPortuguesePhonemizer),
                }) {
                    Assert.NotNull(PhonemizerFactory.Get(type));
                }
                PhonemizerFactory.BuildList();
            }
            public void Dispose() => Directory.Delete(Root, true);
        }

        internal static string OwnedState(UProject project) {
            // Compare semantic source data, excluding native schema/default additions.
            return Yaml.DefaultSerializer.Serialize(new {
                tempos = project.tempos,
                meters = project.timeSignatures,
                sourceExpression = project.expressions["src"],
                tracks = project.tracks.Select(t => new { t.TrackName, phonemizer = t.Phonemizer.GetType().FullName }),
                parts = project.parts.OfType<UVoicePart>().Select(p => new {
                    p.name, p.trackNo, p.position, p.duration,
                    notes = p.notes.Select(n => new { n.position, n.duration, n.tone, n.lyric, n.PhonemizerOverride, n.phonemeExpressions, n.pitch, n.vibrato }),
                    p.curves,
                }),
            });
        }

        internal static void SaveChecked(SourceCopy copy, UProject project) {
            Assert.False(File.Exists(copy.Output));
            Ustx.Save(copy.Output, project); // Native Save catches errors: require an actual readable file.
            Assert.True(File.Exists(copy.Output));
            Assert.True(project.Saved);
            Assert.True(new FileInfo(copy.Output).Length > 0);
        }

        internal static void AssertNativeBindings(UProject project) {
            var first = (UVoicePart)project.parts[0];
            var notes = first.notes.ToArray();
            Assert.Same(notes[0], notes[1].Extends);
            Assert.Same(notes[0], notes[2].Extends);
            Assert.Equal(1440, notes[0].ExtendedDuration);
            foreach (var part in project.parts.OfType<UVoicePart>()) {
                foreach (var curve in part.curves) {
                    var expected = project.expressions[curve.abbr];
                    Assert.Same(expected, curve.descriptor);
                    Assert.Equal((int)expected.defaultValue, curve.Sample(curve.xs.First() - 5));
                    Assert.Equal((int)expected.defaultValue, curve.Sample(curve.xs.Last() + 5));
                }
            }
        }

        [Fact]
        public void Ustx06LoadSaveReloadPreservesSourceOwnershipHintsAndCurves() {
            using var copy = new SourceCopy(SourceYaml);
            var inputBytes = File.ReadAllBytes(copy.Input);
            var project = Ustx.Load(copy.Input);
            Assert.Equal(2, project.parts.Count);
            Assert.Equal(6, project.parts.OfType<UVoicePart>().Sum(p => p.notes.Count));
            var raw = Yaml.DefaultDeserializer.Deserialize<UProject>(SourceYaml);
            Assert.Equal(Yaml.DefaultSerializer.Serialize(raw.expressions["src"]),
                Yaml.DefaultSerializer.Serialize(project.expressions["src"]));
            Assert.Equal(raw.tempos.Select(t => (t.position, t.bpm)), project.tempos.Select(t => (t.position, t.bpm)));
            Assert.Equal(raw.timeSignatures.Select(t => (t.barPosition, t.beatPerBar, t.beatUnit)),
                project.timeSignatures.Select(t => (t.barPosition, t.beatPerBar, t.beatUnit)));
            Assert.Equal(raw.tracks.Select(t => (t.TrackName, t.phonemizer)),
                project.tracks.Select(t => (t.TrackName, t.Phonemizer.GetType().FullName)));
            var rawParts = raw.voiceParts!.ToArray();
            for (int p = 0; p < rawParts.Length; p++) {
                var part = (UVoicePart)project.parts[p];
                Assert.Equal((rawParts[p].name, rawParts[p].trackNo, rawParts[p].position, rawParts[p].duration),
                    (part.name, part.trackNo, part.position, part.duration));
                Assert.Equal(Yaml.DefaultSerializer.Serialize(rawParts[p].curves), Yaml.DefaultSerializer.Serialize(part.curves));
                Assert.Equal(rawParts[p].notes.Select(n => (n.position, n.duration, n.tone, n.lyric, n.PhonemizerOverride)),
                    part.notes.Select(n => (n.position, n.duration, n.tone, n.lyric, n.PhonemizerOverride)));
                Assert.Equal(rawParts[p].notes.Select(n => Yaml.DefaultSerializer.Serialize(n.phonemeExpressions)),
                    part.notes.Select(n => Yaml.DefaultSerializer.Serialize(n.phonemeExpressions)));
                foreach (var note in part.notes.Where(n => n.PhonemizerOverride != null)) {
                    var factory = Assert.Single(PhonemizerFactory.GetAll(), f => f.name == note.PhonemizerOverride);
                    var native = note.ToPhonemizerNote(project.tracks[part.trackNo], part);
                    var start = note.lyric.IndexOf('[');
                    Assert.Equal(note.lyric.Substring(0, start), native.lyric);
                    Assert.Equal(note.lyric.Substring(start + 1).TrimEnd(']'), native.phoneticHint);
                    Assert.Equal(part.position + note.position, native.position);
                    Assert.Equal(note.duration, native.duration);
                    Assert.Equal(note.tone, native.tone);
                    // Inspect the actual UPart resolver cache, not a duplicate resolver.
                    var cache = typeof(UVoicePart).GetField("overridePhonemizers", BindingFlags.Instance | BindingFlags.NonPublic)!;
                    var resolved = (System.Collections.Generic.Dictionary<string, Phonemizer>)cache.GetValue(part)!;
                    if (factory.type != project.tracks[part.trackNo].Phonemizer.GetType()) {
                        Assert.Equal(factory.type, resolved[note.PhonemizerOverride!].GetType());
                    }
                }
            }
            AssertNativeBindings(project);
            var owned = OwnedState(project);
            SaveChecked(copy, project);
            var restored = Ustx.Load(copy.Output);
            Assert.Equal(Ustx.kUstxVersion, restored.ustxVersion);
            Assert.Equal(owned, OwnedState(restored));
            AssertNativeBindings(restored);
            Assert.Equal(inputBytes, File.ReadAllBytes(copy.Input));
        }

        [Theory]
        [InlineData(false)]
        [InlineData(true)]
        public void ActualNativeLyricEditUndoAndSaveLeaveExplicitClockInvariant(bool hasExplicitHold) {
            string yaml = hasExplicitHold ? SourceYaml : SourceYaml.Replace(
                "  - {position: 1920, bpm: 55}\n  - {position: 2400, bpm: 110}\n", "");
            using var copy = new SourceCopy(yaml);
            var inputBytes = File.ReadAllBytes(copy.Input);
            var project = Ustx.Load(copy.Input);
            Assert.Equal(hasExplicitHold ? 3 : 1, project.tempos.Count);
            AssertNativeBindings(project);
            var part = (UVoicePart)project.parts[0];
            var note = part.notes.First();
            var original = note.lyric;
            var owned = OwnedState(project);
            var tempos = Yaml.DefaultSerializer.Serialize(project.tempos);
            int[] ticks = { 0, 480, 1919, 1920, 2160, 2399, 2400, 3840 };
            var milliseconds = ticks.Select(t => project.timeAxis.TickPosToMsPos(t)).ToArray();
            // Independent piecewise expectation at every boundary, including each side.
            for (int i = 0; i < ticks.Length; i++) {
                int tick = ticks[i];
                double expected = hasExplicitHold
                    ? Math.Min(tick, 1920) / 480.0 * 60000 / 110
                      + Math.Clamp(tick - 1920, 0, 480) / 480.0 * 60000 / 55
                      + Math.Max(tick - 2400, 0) / 480.0 * 60000 / 110
                    : tick / 480.0 * 60000 / 110;
                Assert.Equal(expected, milliseconds[i], 8);
            }
            var command = new ChangeNoteLyricCommand(part, note, "yeux[fr/y fr/ee]");
            command.Execute();
            Assert.NotEqual(original, note.lyric);
            Assert.Equal("yeux[fr/y fr/ee]", note.lyric);
            project.ValidateFull();
            AssertNativeBindings(project);
            Assert.Equal(tempos, Yaml.DefaultSerializer.Serialize(project.tempos));
            Assert.Equal(milliseconds, ticks.Select(t => project.timeAxis.TickPosToMsPos(t)).ToArray());
            SaveChecked(copy, project);
            var reread = Ustx.Load(copy.Output);
            AssertNativeBindings(reread);
            Assert.Equal("yeux[fr/y fr/ee]", ((UVoicePart)reread.parts[0]).notes.First().lyric);
            Assert.Equal(tempos, Yaml.DefaultSerializer.Serialize(reread.tempos));
            Assert.Equal(milliseconds, ticks.Select(t => reread.timeAxis.TickPosToMsPos(t)).ToArray());
            // Compare every source-owned field after allowing only the intentional lyric edit.
            ((UVoicePart)reread.parts[0]).notes.First().lyric = original;
            Assert.Equal(owned, OwnedState(reread));
            command.Unexecute();
            project.ValidateFull();
            AssertNativeBindings(project);
            Assert.Equal(owned, OwnedState(project));
            Assert.Equal(milliseconds, ticks.Select(t => project.timeAxis.TickPosToMsPos(t)).ToArray());
            Assert.Equal(inputBytes, File.ReadAllBytes(copy.Input));
        }
    }
}
