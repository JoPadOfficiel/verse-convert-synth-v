using System.IO;
using System.Linq;
using OpenUtau.Core;
using OpenUtau.Core.ExpressionGraph;
using OpenUtau.Core.Format;
using OpenUtau.Core.Ustx;
using Xunit;

namespace OpenUtau.App {
    // Copied only into the exact ec7ba520... beta checkout. Never into .569.
    [Collection("Verse native compatibility")]
    public class BetaCompatibilityTest {
        private static string MaskedState(System.Collections.Generic.IEnumerable<UVoicePart> parts) =>
            Yaml.DefaultSerializer.Serialize(parts.Select(p => new { p.name, p.trackNo, p.position, p.maskedCurves }));
        [Fact]
        public void OldSourceWithoutGraphsKeepsNativeDefaultsWithoutInventedAutomation() {
            using var copy = new NativeCompatibilityTest.SourceCopy(NativeCompatibilityTest.SourceYaml);
            var project = Ustx.Load(copy.Input);
            Assert.Equal(new System.Version(0, 10), Ustx.kUstxVersion);
            CheckAbsentAutomation(project);
            var owned = NativeCompatibilityTest.OwnedState(project);
            NativeCompatibilityTest.SaveChecked(copy, project);
            var restored = Ustx.Load(copy.Output);
            CheckAbsentAutomation(restored);
            Assert.Equal(owned, NativeCompatibilityTest.OwnedState(restored));
        }

        private static void CheckAbsentAutomation(UProject project) {
            Assert.Null(project.expressionGraphs);
            Assert.Null(project.defaultExpressionGraphs);
            Assert.Equal(UExpressionType.MaskedCurve, project.expressions[Ustx.RPIT].type);
            Assert.Equal(UExpressionType.MaskedCurve, project.expressions[Ustx.PITO].type);
            foreach (var track in project.tracks) {
                Assert.Null(track.ExpressionGraph);
                Assert.Null(ExpressionGraphProgram.GetEffectiveGraph(project, track));
                Assert.Null(ExpressionGraphProgram.ForTrack(project, track));
                Assert.False(ExpressionGraphProgram.PrefersPitchOverride(project, track));
            }
            Assert.All(project.parts.OfType<UVoicePart>(), p => Assert.Empty(p.maskedCurves));
        }

        [Fact]
        public void SuppliedMaskedValuesAndHolesSurviveNativeLyricEditAndLoadSaveReload() {
            // Values are explicitly supplied by this input fixture. No graph/model is
            // constructed and no rendered-pitch data is computed from notes or lyrics.
            string yaml = NativeCompatibilityTest.SourceYaml.Replace("ustx_version: '0.6'", "ustx_version: '0.10'")
                .Replace("    curves:\n", @"    masked_curves:
      - {abbr: rpit, runs: [{x: 960, ys: [6000.25, 6010.5]}, {x: 980, ys: [5998.75]}]}
      - {abbr: pito, runs: [{x: 1440, ys: [6400.5, 6402.25]}]}
    curves:
");
            using var copy = new NativeCompatibilityTest.SourceCopy(yaml);
            var inputBytes = File.ReadAllBytes(copy.Input);
            var raw = Yaml.DefaultDeserializer.Deserialize<UProject>(yaml);
            var supplied = Yaml.DefaultSerializer.Serialize(raw.voiceParts![0].maskedCurves);
            var allSupplied = MaskedState(raw.voiceParts);
            var project = Ustx.Load(copy.Input);
            var part = (UVoicePart)project.parts[0];
            Assert.Equal(allSupplied, MaskedState(project.parts.OfType<UVoicePart>()));
            Assert.Equal(supplied, Yaml.DefaultSerializer.Serialize(part.maskedCurves));
            var rpit = part.maskedCurves.Single(c => c.abbr == Ustx.RPIT);
            Assert.True(rpit.TrySample(960, out float value));
            Assert.Equal(6000.25f, value);
            Assert.False(rpit.TrySample(970, out _)); // Preserve absence, not a filled default.
            var ordinary = NativeCompatibilityTest.OwnedState(project);
            var original = part.notes.First().lyric;
            var command = new ChangeNoteLyricCommand(part, part.notes.First(), "yeux[fr/y fr/ee]");
            command.Execute();
            Assert.NotEqual(original, part.notes.First().lyric);
            project.ValidateFull();
            NativeCompatibilityTest.AssertNativeBindings(project);
            Assert.Equal(allSupplied, MaskedState(project.parts.OfType<UVoicePart>()));
            Assert.Equal(supplied, Yaml.DefaultSerializer.Serialize(part.maskedCurves));
            NativeCompatibilityTest.SaveChecked(copy, project);
            var restored = Ustx.Load(copy.Output);
            var restoredPart = (UVoicePart)restored.parts[0];
            NativeCompatibilityTest.AssertNativeBindings(restored);
            Assert.Equal(allSupplied, MaskedState(restored.parts.OfType<UVoicePart>()));
            Assert.Equal(supplied, Yaml.DefaultSerializer.Serialize(restoredPart.maskedCurves));
            Assert.Equal("yeux[fr/y fr/ee]", restoredPart.notes.First().lyric);
            restoredPart.notes.First().lyric = original;
            Assert.Equal(ordinary, NativeCompatibilityTest.OwnedState(restored));
            Assert.Null(restored.expressionGraphs);
            Assert.Null(restored.defaultExpressionGraphs);
            Assert.Equal(inputBytes, File.ReadAllBytes(copy.Input));
            command.Unexecute();
            project.ValidateFull();
            NativeCompatibilityTest.AssertNativeBindings(project);
            Assert.Equal(allSupplied, MaskedState(project.parts.OfType<UVoicePart>()));
            Assert.Equal(original, part.notes.First().lyric);
            Assert.Equal(supplied, Yaml.DefaultSerializer.Serialize(part.maskedCurves));
        }
    }
}
