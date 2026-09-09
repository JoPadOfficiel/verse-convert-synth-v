using System.Reflection;
using System.Runtime.Loader;
using System.Runtime.CompilerServices;
using System.Security.Cryptography;
using System.Text.Json;
using OpenUtau.Core;
using OpenUtau.Core.Ustx;
using OpenUtau.Core.Util;
using OpenUtau.Core.Render;
class Program {
static void Main(string[] args) {
    var app=Environment.GetEnvironmentVariable("VERSE_OPENUTAU_APP_DIR") ?? "/Applications/OpenUtau.app/Contents/MacOS";
    AssemblyLoadContext.Default.Resolving+=(ctx,name)=>{var p=Path.Combine(app,name.Name+".dll");return File.Exists(p)?ctx.LoadFromAssemblyPath(p):null;};
    var hash=Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(app+"/OpenUtau.Core.dll"))).ToLowerInvariant();
    if(hash!="0674a9f691a23fedc8aad2debbef9e6104b6296c89c356af64fc799ca5dce1c2")throw new Exception("Installed consumer differs from pinned audited binary");
    Console.WriteLine("CORE_SHA256 "+hash);
    var ui=AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(app,"OpenUtau.dll"));
    Console.WriteLine("APP_ASSEMBLY "+ui.GetName().Version+" "+ui.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion);
    Run(args);
}
[MethodImpl(MethodImplOptions.NoInlining)]
static void Run(string[] args) {
    var version=typeof(UCurve).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
    Console.WriteLine("CONSUMER "+version);
    if(version==null || !version.Contains("3f213e8993ca792c3e6f8958c92ab27eae78eac5"))throw new Exception("Wrong consumer revision");
    var expectedNames=new[]{"pitch","gain","pulse","tempo-rest","default"};
    if(!args.Select(Path.GetFileNameWithoutExtension).Order().SequenceEqual(expectedNames.Order()))throw new Exception("Expected exactly five named fixtures");
    foreach(var path in args) {
        var fixtureName=Path.GetFileNameWithoutExtension(path);
        var project=Yaml.DefaultDeserializer.Deserialize<UProject>(File.ReadAllText(path));
        var expectedNotes=fixtureName=="pitch" || fixtureName=="tempo-rest"?2:1;
        var expectedCurves=fixtureName=="default"?Array.Empty<string>():new[]{fixtureName=="gain"?"dyn":"pitd"};
        if(project.tracks.Count!=1 || project.voiceParts.Count!=1 || project.voiceParts[0].notes.Count!=expectedNotes)throw new Exception("Fixture part/note counts changed: "+fixtureName);
        if(!project.voiceParts[0].curves.Select(c=>c.abbr).Order().SequenceEqual(expectedCurves.Order()))throw new Exception("Missing/unexpected curves: "+fixtureName);
        if(project.voiceParts[0].curves.Any(c=>c.xs.Count==0 || c.xs.Count!=c.ys.Count))throw new Exception("Empty or unpaired required curve: "+fixtureName);
        // Exercise the real load, migration and full synchronous validation
        // path on an isolated fixture. DocManager.Initialize is never called;
        // there is no phonemizer runner, singer lookup or acoustic model.
        var loaded=OpenUtau.Core.Format.Ustx.Load(path);
        if(loaded.parts.OfType<UVoicePart>().Count()!=1 || loaded.parts.OfType<UVoicePart>().Single().notes.Count!=expectedNotes)throw new Exception("Native load lost fixture notes");
        var loadedPart=loaded.parts.OfType<UVoicePart>().Single();
        if(!loadedPart.curves.Select(c=>c.abbr).Order().SequenceEqual(expectedCurves.Order()))throw new Exception("Native load lost curves");
        foreach(var originalCurve in project.voiceParts[0].curves) {
            var loadedCurve=loadedPart.curves.Single(c=>c.abbr==originalCurve.abbr);
            if(!loadedCurve.xs.SequenceEqual(originalCurve.xs)||!loadedCurve.ys.SequenceEqual(originalCurve.ys))throw new Exception("Native load changed curve values/times");
        }
        if(!loadedPart.notes.Select(n=>(n.position,n.duration,n.tone,n.lyric)).SequenceEqual(project.voiceParts[0].notes.Select(n=>(n.position,n.duration,n.tone,n.lyric))))throw new Exception("Native load changed nominal note/lyric geometry");
        if(loaded.ustxVersion!=OpenUtau.Core.Format.Ustx.kUstxVersion)throw new Exception("Native migration version mismatch");
        Console.WriteLine("NATIVE_LOAD "+fixtureName+" synchronous AfterLoad/ValidateFull and 0.6 migration passed");
        OpenUtau.Core.Format.Ustx.AddDefaultExpressions(project);
        project.timeAxis.BuildSegments(project);
        foreach(var part in project.voiceParts) {
            var notes=part.notes.ToArray();
            for(int i=0;i<notes.Length;i++) {
                notes[i].Prev=i>0?notes[i-1]:null;
                notes[i].Next=i+1<notes.Length?notes[i+1]:null;
                var before=notes[i].pitch.data.Select(p=>p.Y).ToArray();
                notes[i].Validate(new ValidateOptions {SkipPhonemizer=true,SkipPhoneme=true},project,project.tracks[part.trackNo],part);
                if(!notes[i].pitch.snapFirst && !before.SequenceEqual(notes[i].pitch.data.Select(p=>p.Y)))throw new Exception("Snap changed an imported base");
            }
            foreach(var curve in part.curves) {
                curve.descriptor=project.expressions[curve.abbr];
                if(curve.xs.Count!=curve.ys.Count)throw new Exception("Unpaired curve arrays");
                for(int i=1;i<curve.xs.Count;i++)if(curve.xs[i]<=curve.xs[i-1])throw new Exception("Curve x order");
            }
            var name=Path.GetFileNameWithoutExtension(path);
            var pitch=part.curves.FirstOrDefault(c=>c.abbr=="pitd");
            var gain=part.curves.FirstOrDefault(c=>c.abbr=="dyn");
            if(name=="pitch") {
                for(int tick=0;tick<=960;tick++) {
                    var expected=tick<240?0:tick<480?100:tick<720?-200:0;
                    if(pitch.Sample(tick)!=expected)throw new Exception("PITD tick "+tick);
                }
                for(int leading=0;leading<5;leading++) {
                    var baseline=PitchConsumer.Sample(project,part,notes[0].position,leading,notes.Last().End);
                    for(int i=0;i<baseline.Length;i++) {
                        int x=notes[0].position-leading+i*5;
                        // RenderPhrase floors the next note's first sample
                        // index. A flat base therefore steps up to four ticks
                        // early, never through an extra interpolated portamento.
                        int boundary=480-((480+leading)%5);
                        float expected=x<boundary?6000:6400;
                        if(baseline[i]!=expected)throw new Exception($"Synthetic transition at {x}: {baseline[i]} != {expected}");
                        // This extracted body verifies the flat base only.
                        // PITD composition in the full RenderPhrase constructor
                        // requires RenderPhone/singer/renderer state and is
                        // source-inspected, not claimed as executed here.
                    }
                }
            }
            if(name=="gain") {
                var method=typeof(RenderPhrase).GetMethod("SampleCurve",BindingFlags.NonPublic|BindingFlags.Static,null,new[]{typeof(UCurve),typeof(int),typeof(int),typeof(Func<float,UCurve,float>)},null);
                // RenderPhrase.cs:471 exact DYN conversion, invoked through the
                // consumer's actual private 5-tick sampling method.
                Func<float,UCurve,float> convert=(x,c)=>x==c.descriptor.min?0:(float)MusicMath.DecibelToLinear(x*0.1);
                for(int origin=0;origin<5;origin++) {
                    var samples=(float[])method.Invoke(null,new object[]{gain,origin,192,convert});
                    for(int i=0;i<samples.Length;i++) {
                        var tick=origin+5*i; var expected=tick<240?1:tick<480?Math.Pow(10,-0.3):tick<720?0:1;
                        if(Math.Abs(samples[i]-expected)>1e-6)throw new Exception("Gain/mute/restore mismatch");
                    }
                }
            }
            if(name=="pulse")for(int origin=0;origin<5;origin++)if(!Enumerable.Range(0,96).Any(i=>pitch.Sample(origin+5*i)==100))throw new Exception("Pulse disappeared");
            if(name=="pulse") {
                foreach(var width in new[]{1,4,5}) foreach(var abbr in new[]{"pitd","dyn"}) {
                    var value=abbr=="pitd"?100:-240;
                    var terminal=new UCurve(project.expressions[abbr]) {
                        xs=new(){0,479-width,480-width,480}, ys=new(){0,0,value,value}
                    };
                    var disappears=Enumerable.Range(0,5).Any(origin=>!Enumerable.Range(0,96).Any(i=>terminal.Sample(origin+5*i)==value));
                    if(disappears!=(width<5))throw new Exception("Terminal pulse sampling bound");
                }
                Console.WriteLine("TERMINAL_PULSE pitd/dyn widths=1,4,5 native UCurve.Sample verified");
            }
            if(name=="tempo-rest") {
                if(Math.Abs(project.timeAxis.TickPosToMsPos(720)-project.timeAxis.TickPosToMsPos(240)-750)>1e-7)throw new Exception("Tempo-relative timing mismatch");
                if(pitch.Sample(1439)!=625 || pitch.Sample(1920)!=625)throw new Exception("Held state lost across rest/end");
            }
            if(name=="default") {
                if(!notes[0].pitch.snapFirst || notes[0].pitch.data[0].X!=-40 || notes[0].pitch.data[1].X!=40 || part.curves.Count!=0)throw new Exception("Default changed");
            }
            Console.WriteLine(JsonSerializer.Serialize(new{file=name,notes=notes.Length,curves=part.curves.Select(c=>c.abbr).ToArray(),validated=true}));
        }
    }
}
}
