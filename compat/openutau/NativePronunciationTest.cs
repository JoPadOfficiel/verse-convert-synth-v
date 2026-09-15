using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using OpenUtau.Api;
using OpenUtau.Classic;
using OpenUtau.Core;
using OpenUtau.Core.DiffSinger;
using OpenUtau.Core.Ustx;
using OpenUtau.Core.Util;
using Xunit;

namespace OpenUtau.App {
    [CollectionDefinition("Verse native pronunciation", DisableParallelization = true)]
    public class NativePronunciationCollection { }

    [Collection("Verse native pronunciation")]
    public class NativePronunciationTest {
        // This is the stock consumer pinned by test-openutau-clean-lyrics.sh,
        // not nobodyP's Spanish+ plugin. Verse supplies MFA allophones; the stock
        // consumer validates hints against dictionary types AND duration tokens.
        // It does not infer contextual B/D/G or promise an exact b/d/g fallback.
        // The acoustic vocabulary is independent and checked later by the singer.
        public static IEnumerable<object[]> ExactHintCases() {
            var path = Environment.GetEnvironmentVariable("VERSE_OPENUTAU_EXACT_HINT_FIXTURE");
            Assert.False(string.IsNullOrEmpty(path));
            var rows = File.ReadAllLines(path!).Select(line => line.Split('\t')).ToArray();
            Assert.Equal(8, rows.Length);
            foreach (var row in rows) {
                Assert.Equal(3, row.Length);
                foreach (bool prefixed in new[] { false, true }) {
                    yield return new object[] { row[0], row[1], row[2], prefixed };
                }
            }
        }

        public static IEnumerable<object[]> MissingPhoneCases() {
            foreach (var row in ExactHintCases()) {
                var phone = ((string)row[2]).Split(' ').FirstOrDefault(
                    symbol => symbol is "B" or "D" or "G" or "S");
                // dedo also contains D; use cada once for the missing-D case.
                if (phone != null && (string)row[1] != "dedo") {
                    yield return row.Concat(new object[] { phone }).ToArray();
                }
            }
        }

        [Theory]
        [MemberData(nameof(ExactHintCases))]
        public void ExactMfaHintReachesNativeProcess(
            string language, string word, string hint, bool prefixed) {
            using var bank = new SyntheticSinger(language, word, prefixed);
            var phonemizer = CreateConsumer(language);
            var expected = hint.Split(' ').Select(bank.Alias).ToArray();
            var result = RunNative(phonemizer, bank, word + "[" + hint + "]");
            Assert.Equal(expected, result.phonemes.Select(p => p.phoneme));
            // Native acoustic tokenization must preserve each distinct symbol too.
            Assert.Equal(expected.Select(bank.Token),
                result.phonemes.Select(p => bank.Singer.PhonemeTokenize(p.phoneme)));
            // A deliberately nonlinguistic bank entry resolves to SP. This control
            // proves the result above consumed the hint, not the bank dictionary.
            Assert.Equal(new[] { "SP" }, RunNative(phonemizer, bank, word)
                .phonemes.Select(p => p.phoneme));
        }

        [Theory]
        [MemberData(nameof(MissingPhoneCases))]
        public void MissingDurationPhoneRaisesNativeErrorWithoutApproximateFallback(
            string language, string word, string hint, bool prefixed, string missing) {
            using var bank = new SyntheticSinger(language, word, prefixed,
                missingDuration: missing);
            // Acoustic support alone cannot make a duration hint valid.
            Assert.Equal(bank.Token(bank.Alias(missing)),
                bank.Singer.PhonemeTokenize(bank.Alias(missing)));
            var phonemizer = CreateConsumer(language);
            var error = Assert.Throws<Exception>(() =>
                RunNative(phonemizer, bank, word + "[" + hint + "]"));
            Assert.Equal($"Unrecognized phoneme \"{missing}\"", error.Message);
            // The same bank CAN resolve unhinted text. An invalid explicit hint
            // nevertheless raises an error rather than retrying that fallback.
            Assert.Equal(new[] { "SP" }, RunNative(phonemizer, bank, word)
                .phonemes.Select(p => p.phoneme));
        }

        [Theory]
        [MemberData(nameof(MissingPhoneCases))]
        public void MissingAcousticPhoneFailsAfterSuccessfulNativePhonemization(
            string language, string word, string hint, bool prefixed, string missing) {
            using var bank = new SyntheticSinger(language, word, prefixed,
                missingAcoustic: missing);
            var result = RunNative(CreateConsumer(language), bank, word + "[" + hint + "]");
            Assert.Equal(hint.Split(' ').Select(bank.Alias),
                result.phonemes.Select(p => p.phoneme));
            var rejected = Assert.Single(result.phonemes,
                p => p.phoneme == bank.Alias(missing));
            var error = Assert.Throws<Exception>(() => bank.Singer.PhonemeTokenize(rejected.phoneme));
            Assert.Equal($"Phoneme \"{bank.Alias(missing)}\" isn't supported by acoustic model. " +
                $"Please check {Path.Combine(bank.Root, "phonemes.txt")}", error.Message);
        }

        private static Phonemizer CreateConsumer(string language) => language switch {
            "es" => new DiffSingerSpanishPhonemizer(),
            "pt" => new DiffSingerPortuguesePhonemizer(),
            _ => throw new ArgumentException("Unexpected fixture language", nameof(language)),
        };

        private static Phonemizer.Result RunNative(
            Phonemizer phonemizer, SyntheticSinger bank, string lyric) {
            var project = new UProject();
            var track = new UTrack { Singer = bank.Singer };
            var source = new UNote { lyric = lyric, position = 480, duration = 480, tone = 60 };
            // Exercise the public native lifecycle, not a reimplementation or
            // reflection call into private hint parsing methods.
            var saved = Yaml.DefaultSerializer.Serialize(source);
            var reread = Yaml.DefaultDeserializer.Deserialize<UNote>(saved);
            var notes = new[] { reread.ToPhonemizerNote(track, new UVoicePart()) };
            bank.Consumers.Add(phonemizer);
            phonemizer.SetSinger(bank.Singer);
            phonemizer.SetTiming(project.timeAxis);
            // Always execute the synthetic graphs, even on a developer machine
            // with cached results. Do not require or write the user's cache.
            // The nonparallel collection isolates this in-memory preference.
            var tensorCache = Preferences.Default.DiffSingerTensorCache;
            Preferences.Default.DiffSingerTensorCache = false;
            try {
                phonemizer.SetUp(new[] { notes }, project, track);
                return phonemizer.Process(notes, null, null, null, null, Array.Empty<Phonemizer.Note>());
            } finally {
                Preferences.Default.DiffSingerTensorCache = tensorCache;
                phonemizer.CleanUp();
                Assert.Equal(lyric, source.lyric);
                Assert.Equal(lyric, reread.lyric);
            }
        }

        private sealed class SyntheticSinger : IDisposable {
            public string Root { get; }
            public DiffSingerSinger Singer { get; }
            public HashSet<Phonemizer> Consumers { get; } = new HashSet<Phonemizer>();
            private readonly string prefix;
            private readonly string[] inventory;

            public string Alias(string symbol) => symbol is "SP" or "AP" ? symbol : prefix + symbol;
            public int Token(string symbol) => Array.IndexOf(inventory, symbol);

            public SyntheticSinger(string language, string word, bool prefixed,
                string missingDuration = null, string missingAcoustic = null) {
                Root = Path.Combine(Path.GetTempPath(), "verse-synthetic-singer-" + Guid.NewGuid());
                prefix = prefixed ? language + "/" : "";
                inventory = "SP AP a e i o u b B d D g G s k l m t S".Split(' ').Select(Alias).ToArray();
                Directory.CreateDirectory(Root);
                try {
                    var durationRoot = Path.Combine(Root, "dsdur");
                    Directory.CreateDirectory(durationRoot);
                    File.WriteAllText(Path.Combine(Root, "dsconfig.yaml"), "phonemes: phonemes.txt\n");
                    File.WriteAllLines(Path.Combine(Root, "phonemes.txt"),
                        inventory.Where(p => missingAcoustic == null || p != Alias(missingAcoustic)));
                    File.WriteAllText(Path.Combine(durationRoot, "dsconfig.yaml"),
                        "phonemes: phonemes.txt\nlinguistic: linguistic.onnx\ndur: duration.onnx\n");
                    File.WriteAllLines(Path.Combine(durationRoot, "phonemes.txt"),
                        inventory.Where(p => missingDuration == null || p != Alias(missingDuration)));
                    var symbols = "SP AP a e i o u b B d D g G s k l m t S".Split(' ');
                    var dictionary = "symbols:\n" + string.Concat(symbols.Select(symbol =>
                        $"  - {{symbol: '{Alias(symbol)}', type: {((symbol is "SP" or "AP" or "a" or "e" or "i" or "o" or "u") ? "vowel" : "consonant")}}}\n"));
                    dictionary += $"entries:\n  - grapheme: '{word}'\n    phonemes: [SP]\n";
                    File.WriteAllText(Path.Combine(durationRoot, "dsdict-" + language + ".yaml"), dictionary);
                    File.WriteAllBytes(Path.Combine(durationRoot, "linguistic.onnx"), Convert.FromBase64String(LinguisticModel));
                    File.WriteAllBytes(Path.Combine(durationRoot, "duration.onnx"), Convert.FromBase64String(DurationModel));
                    Singer = new DiffSingerSinger(new Voicebank {
                        File = Path.Combine(Root, "character.txt"), BasePath = Root,
                        Name = "Verse synthetic inventory (no voice)", SingerType = USingerType.DiffSinger,
                    });
                    Assert.Empty(Singer.Errors);
                } catch {
                    Directory.Delete(Root, true);
                    throw;
                }
            }

            public void Dispose() {
                // The pinned base has no public session-disposal API. Reflection
                // is confined to cleanup; all behavior above uses public methods.
                foreach (var consumer in Consumers) {
                    foreach (var name in new[] { "linguisticModel", "durationModel" }) {
                        var field = typeof(DiffSingerBasePhonemizer).GetField(name,
                            BindingFlags.Instance | BindingFlags.NonPublic);
                        Assert.NotNull(field);
                        (field!.GetValue(consumer) as IDisposable)?.Dispose();
                    }
                }
                Singer.FreeMemory();
                Directory.Delete(Root, true);
            }

            // Self-authored ONNX graphs, IR 8 / opset 13, checked with onnx 1.20.1.
            // No trained weights, speech, speaker embeddings, acoustic model or
            // vocoder. Native SetSinger requires these two inference sessions:
            // linguistic: Cast(tokens -> FLOAT encoder_out), Shape(tokens) ->
            // ConstantOfShape(BOOL false x_masks); inputs also word_div, word_dur.
            // duration: Shape(ph_midi) -> ConstantOfShape(FLOAT 1 durations);
            // inputs also encoder_out, x_masks. All tensors have [1, phones]
            // shape except word_div/word_dur [1, words]. Durations are arbitrary
            // test scaffolding; passing this gate says nothing about singing.
            private const string LinguisticModel =
                "CAg6wwIKJgoGdG9rZW5zEgtlbmNvZGVyX291dCIEQ2FzdCoJCgJ0bxgBoAECChYKBnRva2VucxIFc2hhcGUiBVNoYXBlCj0KBXNoYXBlEgd4X21hc2tzIg9Db25zdGFudE9mU2hhcGUqGgoFdmFsdWUqDggBEAkqAQBCBXZhbHVloAEEEhp2ZXJzZS1zeW50aGV0aWMtbGluZ3Vpc3RpY1oeCgZ0b2tlbnMSFAoSCAcSDgoCCAEKCBIGcGhvbmVzWh8KCHdvcmRfZGl2EhMKEQgHEg0KAggBCgcSBXdvcmRzWh8KCHdvcmRfZHVyEhMKEQgHEg0KAggBCgcSBXdvcmRzYiMKC2VuY29kZXJfb3V0EhQKEggBEg4KAggBCggSBnBob25lc2IfCgd4X21hc2tzEhQKEggJEg4KAggBCggSBnBob25lc0IECgAQDQ==";
            private const string DurationModel =
                "CAg6gQIKFwoHcGhfbWlkaRIFc2hhcGUiBVNoYXBlCkIKBXNoYXBlEglkdXJhdGlvbnMiD0NvbnN0YW50T2ZTaGFwZSodCgV2YWx1ZSoRCAEQASIEAACAP0IFdmFsdWWgAQQSGHZlcnNlLXN5bnRoZXRpYy1kdXJhdGlvblojCgtlbmNvZGVyX291dBIUChIIARIOCgIIAQoIEgZwaG9uZXNaHwoHeF9tYXNrcxIUChIICRIOCgIIAQoIEgZwaG9uZXNaHwoHcGhfbWlkaRIUChIIBxIOCgIIAQoIEgZwaG9uZXNiIQoJZHVyYXRpb25zEhQKEggBEg4KAggBCggSBnBob25lc0IECgAQDQ==";
        }

        [Fact]
        public void PortugueseExactHintSurvivesNativeParsingAndRoundTrip() {
            // portuguese-pronunciation.tsv: acho, brazil+portugal, a ʃ u.
            // Verse maps MFA ʃ exactly to the stock Portuguese DiffSinger S.
            var note = new UNote {
                lyric = "acho[a S u]",
                PhonemizerOverride = "DiffSinger Portuguese Phonemizer",
                position = 480,
                duration = 480,
                tone = 60,
            };
            var saved = Yaml.DefaultSerializer.Serialize(note);
            var reread = Yaml.DefaultDeserializer.Deserialize<UNote>(saved);
            Assert.Equal(note.lyric, reread.lyric);
            Assert.Equal(note.PhonemizerOverride, reread.PhonemizerOverride);

            var native = reread.ToPhonemizerNote(new UTrack(), new UVoicePart());
            Assert.Equal("acho", native.lyric);
            Assert.Equal("a S u", native.phoneticHint);
            Assert.Equal(note.position, native.position);
            Assert.Equal(note.duration, native.duration);
            Assert.Equal(note.tone, native.tone);

            Assert.NotNull(PhonemizerFactory.Get(typeof(DiffSingerPortuguesePhonemizer)));
            PhonemizerFactory.BuildList();
            var factory = Assert.Single(PhonemizerFactory.GetAll(),
                f => f.name == reread.PhonemizerOverride);
            Assert.IsType<DiffSingerPortuguesePhonemizer>(factory.Create());
        }

        [Fact]
        public void VerseExportResolvesEveryNativeWordOverride() {
            string path = Environment.GetEnvironmentVariable("VERSE_OPENUTAU_PRONUNCIATION_FIXTURE")!;
            Assert.False(string.IsNullOrEmpty(path));
            var project = Yaml.DefaultDeserializer.Deserialize<UProject>(File.ReadAllText(path));
            var part = Assert.Single(project.voiceParts!);
            var notes = part.notes.ToArray();
            Type[] expected = {
                typeof(DiffSingerFrenchMillfeuillePhonemizer),
                typeof(DiffSingerEnglishPhonemizer),
                typeof(DiffSingerSpanishPhonemizer),
                typeof(DiffSingerPortuguesePhonemizer),
                typeof(DiffSingerFrenchMillfeuillePhonemizer),
            };
            foreach (var type in expected.Distinct()) {
                Assert.NotNull(PhonemizerFactory.Get(type));
            }
            PhonemizerFactory.BuildList();
            Assert.Equal(expected.Length, notes.Length);
            for (int i = 0; i < notes.Length; i++) {
                // This is the pinned UPart resolver's actual name contract.
                var factory = PhonemizerFactory.GetAll().SingleOrDefault(
                    f => f.name == notes[i].PhonemizerOverride);
                Assert.NotNull(factory);
                Assert.Equal(expected[i], factory!.Create().GetType());
                Assert.NotEqual(expected[i].FullName, notes[i].PhonemizerOverride);
                Assert.Equal(i * 480, notes[i].position);
                Assert.Equal(480, notes[i].duration);
                var saved = Yaml.DefaultSerializer.Serialize(notes[i]);
                var reread = Yaml.DefaultDeserializer.Deserialize<UNote>(saved);
                Assert.Equal(notes[i].PhonemizerOverride, reread.PhonemizerOverride);
                Assert.Equal(notes[i].lyric, reread.lyric);
            }
            // Verse owns the exact MFA-derived Spanish hint. OpenUtau still
            // owns the DiffSinger consumer/type resolution for that note.
            Assert.Equal("hola[o l a]", notes[2].lyric);
            Assert.Equal("obrigado", notes[3].lyric);
        }
    }
}
