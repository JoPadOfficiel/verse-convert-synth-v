// FID-002 model-free native consumer for the synthetic source_continuity exports.
// Native implementation: OpenUtau 3f213e8993ca792c3e6f8958c92ab27eae78eac5.
// UPart.cs:99-119 owns predecessor linkage and UNote validation;
// UNote.cs:94-114 owns duration/overlap checks; :120-160 reads lyric/hints.
// Do not call Ustx.Load/ValidateFull: this probe intentionally exercises only
// native YAML deserialization, part AfterLoad and the explicit skipped-work
// Validate path, without track/singer/phonemizer initialization or rendering.
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text.Json;
using OpenUtau.Core;
using OpenUtau.Core.Ustx;
using OpenUtau.Core.Util;

internal static class Program {
    internal const string Revision = "3f213e8993ca792c3e6f8958c92ab27eae78eac5";
    internal const string CoreHash = "0674a9f691a23fedc8aad2debbef9e6104b6296c89c356af64fc799ca5dce1c2";

    static int Main(string[] args) {
        var output = Console.Out;
        try {
            string app = Path.GetFullPath(Environment.GetEnvironmentVariable("VERSE_OPENUTAU_APP_DIR")
                ?? throw new InvalidOperationException("APP_DIRECTORY_REQUIRED"));
            string core = Path.Combine(app, "OpenUtau.Core.dll");
            string hash = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(core))).ToLowerInvariant();
            if (hash != CoreHash) throw new InvalidOperationException("CORE_PIN_MISMATCH");
            AssemblyLoadContext.Default.Resolving += (context, name) => {
                string path = Path.Combine(app, name.Name + ".dll");
                return File.Exists(path) ? context.LoadFromAssemblyPath(path) : null;
            };
            // A native library's incidental console log is not evidence. Only
            // the final structured result is written to stdout, once complete.
            Console.SetOut(TextWriter.Null);
            object evidence = NativeProbe.Run(args);
            Console.SetOut(output);
            string json = JsonSerializer.Serialize(evidence);
            if (System.Text.Encoding.UTF8.GetByteCount(json) > 131072)
                throw new InvalidOperationException("EVIDENCE_SIZE_LIMIT");
            output.WriteLine(json);
            return 0;
        } catch (Exception error) {
            Console.SetOut(output);
            while (error is TargetInvocationException && error.InnerException != null)
                error = error.InnerException;
            string message = error.Message;
            Console.Error.WriteLine(JsonSerializer.Serialize(new {
                schemaVersion = 1, status = "failed", error = message[..Math.Min(message.Length, 2048)]
            }));
            return 1;
        }
    }
}

internal static class NativeProbe {
    const string High = "Alti — polyphonic member 2";
    const string Low = "Alti — polyphonic member 1";
    static readonly string[] Names = {
        "pb-default", "pb-french", "pb-english", "chant-default", "chant-french", "chant-english"
    };
    static readonly MethodInfo ValidatePart = typeof(UVoicePart).GetMethod("Validate",
        BindingFlags.Instance | BindingFlags.Public, null,
        new[] { typeof(ValidateOptions), typeof(UProject), typeof(UTrack) }, null)
        ?? throw new InvalidOperationException("NATIVE_VALIDATE_METHOD_MISSING");
    static readonly MethodInfo ReadNativeNote = typeof(UNote).GetMethod("ToPhonemizerNote",
        BindingFlags.Instance | BindingFlags.NonPublic, null,
        new[] { typeof(UTrack), typeof(UPart) }, null)
        ?? throw new InvalidOperationException("NATIVE_LYRIC_READER_MISSING");
    static readonly FieldInfo NotesTimestamp = PrivateTimestamp("notesTimestamp");
    static readonly FieldInfo PhonemesTimestamp = PrivateTimestamp("phonemesTimestamp");

    sealed record Geometry(string Part, int Track, int PartPosition, int Position, int Duration, int Tone, string Lyric);
    sealed record Reading(string Lyric, string? Hint, int Position, int Duration, int Tone);
    sealed record Address(string Part, int Tick, int Duration, int Tone);

    static void Require(bool condition, string code) {
        if (!condition) throw new InvalidOperationException(code);
    }

    static FieldInfo PrivateTimestamp(string name) {
        var field = typeof(UVoicePart).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic);
        Require(field != null && field.FieldType == typeof(long), "NATIVE_TIMESTAMP_CONTRACT_CHANGED:" + name);
        return field!;
    }

    // Delayed until Program installs the resolver for the pinned local app.
    [MethodImpl(MethodImplOptions.NoInlining)]
    internal static object Run(string[] paths) {
        string version = typeof(UVoicePart).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion ?? "";
        Require(version.Contains(Program.Revision, StringComparison.Ordinal), "CONSUMER_REVISION_MISMATCH");
        Require(paths.Length == Names.Length && paths.Select(Path.GetFileNameWithoutExtension)
            .OrderBy(x => x, StringComparer.Ordinal).SequenceEqual(Names.OrderBy(x => x, StringComparer.Ordinal)),
            "EXPECTED_SIX_NAMED_FIXTURES");
        var results = new List<object>();
        string? negativeSource = null;
        foreach (string path in paths.OrderBy(x => Path.GetFileName(x), StringComparer.Ordinal)) {
            string name = Path.GetFileNameWithoutExtension(path);
            byte[] bytes = File.ReadAllBytes(path);
            Require(bytes.Length > 0 && bytes.Length <= 1048576, "FIXTURE_SIZE_LIMIT:" + name);
            string text = new System.Text.UTF8Encoding(false, true).GetString(bytes);
            var project = Read(text);
            CheckGeometry(project);
            var before = Snapshot(project);
            var trackSettings = project.tracks.Select(t => (t.TrackName, t.phonemizer, t.Mute, t.Solo, t.Volume)).ToArray();
            Validate(project);
            Require(before.SequenceEqual(Snapshot(project)), "NATIVE_CHANGED_NOTE_OR_PART_GEOMETRY:" + name);
            Require(trackSettings.SequenceEqual(project.tracks.Select(t => (t.TrackName, t.phonemizer, t.Mute, t.Solo, t.Volume))),
                "NATIVE_CHANGED_TRACK_SETTINGS:" + name);
            Require(project.voiceParts.SelectMany(p => p.notes).All(n => !n.OverlapError), "NATIVE_OVERLAP:" + name);
            object holds = CheckHolds(project, name.StartsWith("chant-", StringComparison.Ordinal));
            object passTwo = CheckPassTwo(project, name);
            var nativeReadings = project.voiceParts.SelectMany(part => part.notes.Select(note => {
                var reading = ReadingOf(project, part, note);
                Require(reading.Position == checked(part.position + note.position) && reading.Duration == note.duration && reading.Tone == note.tone,
                    "NATIVE_LYRIC_READER_CHANGED_GEOMETRY");
                return new { part = part.name, tick = reading.Position, duration = reading.Duration,
                    pitch = reading.Tone, serializedLyric = note.lyric, nativeLyric = reading.Lyric,
                    phoneticHint = reading.Hint,
                    extendsTick = note.Extends == null ? (int?)null : checked(part.position + note.Extends.position),
                    extendedDuration = note.ExtendedDuration, overlap = note.OverlapError };
            })).ToArray();
            int singerErrors = project.voiceParts.SelectMany(p => p.notes).Count(n => n.Error);
            Require(singerErrors == 21, "EXPECTED_MISSING_SINGER_FLAGS");
            results.Add(new {
                fixture = name, sha256 = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant(),
                tracks = project.tracks.Count, parts = project.voiceParts.Count, notes = nativeReadings.Length,
                geometryUnchanged = true, missingSingerErrors = singerErrors, holds, passTwo,
                partTiming = project.voiceParts.Select(p => new { part = p.name, position = p.position, duration = p.Duration }).ToArray(),
                nativeReadings
            });
            Require(File.ReadAllBytes(path).SequenceEqual(bytes), "INPUT_BYTES_CHANGED:" + name);
            if (name == "pb-default") negativeSource = text;
        }
        Require(negativeSource != null, "NEGATIVE_CONTROL_SOURCE_MISSING");
        object negatives = NegativeControls(negativeSource!);
        return new {
            schemaVersion = 1, status = "passed", revision = Program.Revision, coreSha256 = Program.CoreHash,
            consumerVersion = version, fixtureCount = results.Count,
            methods = new[] { "Yaml.DefaultDeserializer.Deserialize<UProject>", "UVoicePart.AfterLoad",
                "UVoicePart.Validate(SkipPhonemizer=true, SkipPhoneme=true)", "UNote.ToPhonemizerNote" },
            fullUstxLoadExecuted = false, singerLoaded = false, phonemizerExecuted = false, rendered = false,
            noteErrorPolicy = "Missing-singer Error flags are expected; OverlapError must be false.",
            fixtures = results, negativeControls = negatives
        };
    }

    static UProject Read(string text) {
        var project = Yaml.DefaultDeserializer.Deserialize<UProject>(text);
        CheckStructure(project);
        return project;
    }

    static void CheckStructure(UProject project) {
        Require(project != null && project.resolution == 480, "FIXTURE_RESOLUTION");
        Require(project.tracks.Count == 5 && project.voiceParts.Count == 5
            && project.voiceParts.Sum(p => p.notes.Count) == 21, "FIXTURE_COUNTS");
        Require(project.waveParts.Count == 0, "FIXTURE_HAS_AUDIO");
        var expected = new[] { "Sop", "Alti", Low, High, "Bass" }.OrderBy(x => x, StringComparer.Ordinal);
        Require(project.voiceParts.Select(p => p.name).OrderBy(x => x, StringComparer.Ordinal).SequenceEqual(expected), "FIXTURE_PART_NAMES");
        Require(project.voiceParts.Select(p => p.trackNo).OrderBy(x => x).SequenceEqual(Enumerable.Range(0, 5)), "FIXTURE_TRACK_OWNERSHIP");
        foreach (var part in project.voiceParts) {
            Require(part.position == 0 && part.trackNo >= 0 && part.trackNo < project.tracks.Count, "FIXTURE_PART_POSITION");
            Require(part.name == project.tracks[part.trackNo].TrackName, "FIXTURE_PART_TRACK_MISMATCH");
            Require(part.curves.Count == 0 && part.phonemes.Count == 0, "FIXTURE_HAS_PERFORMANCE_OR_PHONEMES");
            foreach (var note in part.notes) {
                Require(note.position >= 0 && note.duration >= 10 && note.duration <= 960 && note.tone is >= 0 and <= 127,
                    "FIXTURE_NOTE_RANGE");
                Require(!string.IsNullOrWhiteSpace(note.lyric) && note.lyric.Length <= 512, "FIXTURE_LYRIC_RANGE");
                Require(note.pitch != null && note.pitch.data.Count > 0 && note.vibrato != null, "FIXTURE_NOTE_DEFAULTS");
            }
        }
        foreach (var track in project.tracks) {
            Require(track.Singer == null && string.IsNullOrEmpty(track.singer), "FIXTURE_MUST_NOT_ASSIGN_A_SINGER");
            Require(!track.Mute && !track.Solo && track.Volume == 0, "FIXTURE_MIXER_CHANGED");
            Require(track.RendererSettings == null || (string.IsNullOrEmpty(track.RendererSettings.renderer)
                && string.IsNullOrEmpty(track.RendererSettings.resampler) && string.IsNullOrEmpty(track.RendererSettings.wavtool)),
                "FIXTURE_MUST_NOT_ASSIGN_A_RENDERER");
        }
    }

    static Geometry[] Snapshot(UProject project) => project.voiceParts.OrderBy(p => p.trackNo)
        .SelectMany(p => p.notes.Select(n => new Geometry(p.name, p.trackNo, p.position, n.position, n.duration, n.tone, n.lyric))).ToArray();

    static void CheckGeometry(UProject project) {
        var expected = new List<Address>();
        foreach (int pass in new[] { 0, 55680 }) {
            expected.AddRange(new[] {
                new Address("Sop", 38160 + pass, 240, 64), new Address("Sop", 38400 + pass, 960, 64),
                new Address("Sop", 39360 + pass, 480, 65), new Address("Alti", 17760 + pass, 480, 59),
                new Address("Alti", 19920 + pass, 480, 64), new Address(Low, 19440 + pass, 240, 59),
                new Address(High, 19440 + pass, 240, 68)
            });
        }
        expected.AddRange(new[] {
            new Address(High, 19680, 240, 68), new Address("Alti", 75360, 240, 68),
            new Address("Alti", 205920, 240, 66), new Address("Alti", 214560, 240, 64),
            new Address("Bass", 180240, 240, 61), new Address("Bass", 205920, 240, 57),
            new Address("Bass", 214560, 240, 59)
        });
        var actual = project.voiceParts.SelectMany(p => p.notes.Select(n => new Address(p.name, checked(p.position + n.position), n.duration, n.tone)));
        string Key(Address n) => $"{n.Part}:{n.Tick:D9}:{n.Duration:D9}:{n.Tone:D3}";
        Require(actual.OrderBy(Key, StringComparer.Ordinal).SequenceEqual(expected.OrderBy(Key, StringComparer.Ordinal)), "FIXTURE_GEOMETRY");
    }

    static void Validate(UProject project) {
        OpenUtau.Core.Format.Ustx.AddDefaultExpressions(project);
        project.timeAxis.BuildSegments(project);
        var options = new ValidateOptions { SkipPhonemizer = true, SkipPhoneme = true };
        foreach (var part in project.voiceParts) {
            var track = project.tracks[part.trackNo];
            Require(track.Singer == null && string.IsNullOrEmpty(track.singer), "SINGER_MUST_REMAIN_UNASSIGNED");
            part.AfterLoad(project, track);
            // Pinned UPart.cs:58-60 and :285-287: unequal timestamps prevent
            // even RenderPhrase.FromPart from being entered. No note/lyric,
            // predecessor link, geometry or validation result is fabricated.
            NotesTimestamp.SetValue(part, 1L);
            PhonemesTimestamp.SetValue(part, 0L);
            Require(!part.PhonemesUpToDate, "RENDER_PREPARATION_NOT_DISABLED");
            ValidatePart.Invoke(part, new object[] { options, project, track });
            Require(part.phonemes.Count == 0 && part.renderPhrases.Count == 0 && !part.PhonemesUpToDate,
                "UNEXPECTED_PHONEMIZER_OR_RENDER_WORK");
            Require(part.Duration >= part.notes.Max(n => n.End), "NATIVE_PART_TRUNCATES_NOTES");
        }
    }

    static UVoicePart Part(UProject project, string name) => project.voiceParts.Single(p => p.name == name);
    static UNote At(UVoicePart part, int absoluteTick, int pitch) {
        var found = part.notes.Where(n => checked(part.position + n.position) == absoluteTick && n.tone == pitch).ToArray();
        Require(found.Length == 1, "NOTE_ADDRESS:" + part.name + ":" + absoluteTick);
        return found[0];
    }

    static Reading ReadingOf(UProject project, UVoicePart part, UNote note) {
        object native = ReadNativeNote.Invoke(note, new object[] { project.tracks[part.trackNo], part })
            ?? throw new InvalidOperationException("NATIVE_LYRIC_READER_RETURNED_NULL");
        object? Field(string name) => native.GetType().GetField(name, BindingFlags.Public | BindingFlags.Instance)?.GetValue(native);
        Require(Field("lyric") is string && Field("position") is int && Field("duration") is int && Field("tone") is int,
            "NATIVE_LYRIC_READER_SHAPE_CHANGED");
        return new Reading((string)Field("lyric")!, (string?)Field("phoneticHint"),
            (int)Field("position")!, (int)Field("duration")!, (int)Field("tone")!);
    }

    static void RequireLink(UNote head, UNote tail, int total) {
        Require(ReferenceEquals(tail.Prev, head) && ReferenceEquals(head.Next, tail)
            && ReferenceEquals(tail.Extends, head) && head.Extends == null
            && head.End == tail.position && head.ExtendedDuration == total
            && tail.ExtendedDuration == tail.duration && !head.OverlapError && !tail.OverlapError,
            "HOLD_LINK");
    }

    static object CheckHolds(UProject project, bool chant) {
        var alti = Part(project, High);
        var head = At(alti, 19440, 68);
        var tail = At(alti, 19680, 68);
        var sop = Part(project, "Sop");
        var sopHead = At(sop, 38160, 64);
        var sopTail = At(sop, 38400, 64);
        Require(head.duration == 240 && tail.duration == 240 && sopHead.duration == 240 && sopTail.duration == 960, "HOLD_DURATIONS");
        Require(tail.lyric == "+~" && sopTail.lyric == "+~", "HOLD_MARKERS");
        Require(ReadingOf(project, alti, head).Lyric.TrimEnd('\'', '’').ToLowerInvariant() == "même", "ALTI_OWNER_WORD");
        Require(ReadingOf(project, sop, sopHead).Lyric.ToLowerInvariant() == (chant ? "mur" : "murs"), "SOP_OWNER_WORD");
        RequireLink(head, tail, 480);
        RequireLink(sopHead, sopTail, 1200);
        Require(At(Part(project, Low), 19440, 59).ExtendedDuration == 240, "LOW_MEMBER_MUST_NOT_OWN_ENDPOINT");
        var before = At(Part(project, "Alti"), 17760, 59);
        Require(before.duration == 480 && before.End == 18240 && before.ExtendedDuration == 480, "MAIN_LANE_REST_CHANGED");
        return new[] {
            new { part = High, head = 19440, tail = 19680, pitch = 68, headDuration = 240, tailDuration = 240, extendedDuration = 480, endpointCopies = 1 },
            new { part = "Sop", head = 38160, tail = 38400, pitch = 64, headDuration = 240, tailDuration = 960, extendedDuration = 1200, endpointCopies = 1 }
        };
    }

    static object CheckPassTwo(UProject project, string fixture) {
        var sop = Part(project, "Sop");
        var head = At(sop, 93840, 64);
        var tail = At(sop, 94080, 64);
        var headReading = ReadingOf(project, sop, head);
        var tailReading = ReadingOf(project, sop, tail);
        Require(head.duration == 240 && tail.duration == 960 && head.Extends == null, "PASS_TWO_TIMING");
        Require(tail.lyric != "+~", "PASS_TWO_HOLD");
        bool split = tail.lyric == "+";
        if (split) {
            bool defaultProfile = fixture.EndsWith("-default", StringComparison.Ordinal);
            Require(headReading.Lyric == "courants" && tailReading.Lyric == "+" && tailReading.Hint == null,
                "PASS_TWO_SPLIT_READING");
            Require(defaultProfile ? headReading.Hint == null : !string.IsNullOrWhiteSpace(headReading.Hint),
                "PASS_TWO_SPLIT_HINT_READING");
            RequireLink(head, tail, 1200);
        } else {
            Require(!fixture.EndsWith("-default", StringComparison.Ordinal), "DEFAULT_SOURCE_WORD_SPLIT_LOST");
            Require(headReading.Lyric == "cou" && tailReading.Lyric == "rants" && tail.Extends == null
                && head.ExtendedDuration == 240 && tail.ExtendedDuration == 960, "PASS_TWO_OWN_SYLLABLE_READING");
        }
        var alti = Part(project, "Alti");
        var ger = At(alti, 75360, 68);
        Require(ReadingOf(project, alti, ger).Lyric == "ger" && ger.Extends == null && ger.duration == 240, "PASS_TWO_GER_READING");
        foreach (var (part, tone) in new[] { (Low, 59), (High, 68) }) {
            var lane = Part(project, part);
            var chan = At(lane, 75120, tone);
            Require(ReadingOf(project, lane, chan).Lyric == "chan" && chan.Extends == null && chan.duration == 240,
                "PASS_TWO_CHAN_READING");
        }
        return new { head = headReading, tail = tailReading, reading = split ? "profile-syllable-split" : "separate-source-syllables",
            tailExtendsTick = tail.Extends == null ? (int?)null : checked(sop.position + tail.Extends.position), ger = ReadingOf(project, alti, ger) };
    }

    static object NegativeControls(string text) {
        var results = new List<object>();
        void Rejected(string name, string expectedCode, Action action) {
            try { action(); }
            catch (InvalidOperationException error) when (error.Message == expectedCode) {
                results.Add(new { control = name, rejected = true, code = expectedCode });
                return;
            }
            throw new InvalidOperationException("NEGATIVE_CONTROL_DID_NOT_REJECT:" + name);
        }
        var empty = Read(text);
        empty.voiceParts.Clear();
        Rejected("empty-project", "FIXTURE_COUNTS", () => CheckStructure(empty));

        var missing = Read(text);
        var missingPart = Part(missing, High);
        missingPart.notes.Remove(At(missingPart, 19680, 68));
        Validate(missing);
        Rejected("missing-endpoint", "FIXTURE_GEOMETRY", () => CheckGeometry(missing));

        var duplicate = Read(text);
        var duplicatePart = Part(duplicate, High);
        var separate = Read(text);
        Require(duplicatePart.notes.Add(At(Part(separate, High), 19680, 68)), "DUPLICATE_CONTROL_SETUP_FAILED");
        Validate(duplicate);
        Require(duplicatePart.notes.Any(n => n.OverlapError), "NATIVE_DUPLICATE_OVERLAP_NOT_DETECTED");
        Rejected("duplicate-endpoint-native-overlap", "FIXTURE_GEOMETRY", () => CheckGeometry(duplicate));

        var gap = Read(text);
        var gapPart = Part(gap, High);
        var gapTail = At(gapPart, 19680, 68);
        gapPart.notes.Remove(gapTail);
        gapTail.position += 1;
        gapPart.notes.Add(gapTail);
        Validate(gap);
        Require(gapTail.Extends == null && !gapTail.OverlapError, "NATIVE_GAP_WAS_BOUND");
        Rejected("one-tick-gap-native-ungrouped", "HOLD_LINK", () => RequireLink(At(gapPart, 19440, 68), gapTail, 480));

        var wrong = Read(text);
        var wrongHigh = Part(wrong, High);
        var wrongTail = At(wrongHigh, 19680, 68);
        wrongHigh.notes.Remove(wrongTail);
        var wrongMain = Part(wrong, "Alti");
        wrongMain.notes.Add(wrongTail);
        Validate(wrong);
        Require(wrongTail.Extends == null && !wrongTail.OverlapError && wrongTail.Prev != null
            && wrongTail.Prev.position == 17760 && wrongTail.Prev.End == 18240, "NATIVE_WRONG_LANE_GAP_WAS_BOUND");
        Rejected("wrong-lane-1440-tick-gap-native-ungrouped", "HOLD_LINK", () => RequireLink(At(wrongHigh, 19440, 68), wrongTail, 480));

        var stolen = Read(text);
        var stolenSop = Part(stolen, "Sop");
        At(stolenSop, 94080, 64).lyric = "+~";
        Validate(stolen);
        Require(ReferenceEquals(At(stolenSop, 94080, 64).Extends, At(stolenSop, 93840, 64)), "ELIGIBLE_HOLD_CONTROL_SETUP_FAILED");
        Rejected("eligible-syllable-replaced-by-hold", "PASS_TWO_HOLD", () => CheckPassTwo(stolen, "pb-default"));
        Require(results.Count == 6, "NEGATIVE_CONTROL_COUNT");
        return results;
    }
}
