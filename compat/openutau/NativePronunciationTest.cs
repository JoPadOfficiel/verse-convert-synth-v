using System;
using System.IO;
using System.Linq;
using OpenUtau.Api;
using OpenUtau.Core;
using OpenUtau.Core.DiffSinger;
using OpenUtau.Core.Ustx;
using Xunit;

namespace OpenUtau.App {
    public class NativePronunciationTest {
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
