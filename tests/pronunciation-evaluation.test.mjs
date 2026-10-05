import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

test("editorial eligible-record accounting separates wrong covered predictions from noncoverage", () => {
  const result = spawnSync("python3", ["-c", `
import importlib.util
s=importlib.util.spec_from_file_location('evaluation','scripts/evaluate-pronunciation.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
sha='66ad238c35ed838e95b9eebb7bc421168539a600eaab055b69b234663d867fff'
owner={'track':'t','part':None,'staff':None,'voice':None,'occurrence':1,'segment':None,'lane':'lyrics','verse':1}
notes=[{'owner':owner,'id':str(i),'raw':'word','continues':False,'verse':1,'baseline_owner':'en','hybrid_owner':'en'} for i in range(3)]
words=[{'source_word':{'owner':owner,'members':['0']},'raw_laya_choice':'en'},{'source_word':{'owner':owner,'members':['1']},'raw_laya_choice':'pt'}]
r=m.eligible_development_oracle([{'source_sha256':sha,'notes':notes,'words':words}])[0]
assert r['counts']['eligible_lyric_records']==3
assert r['counts']['raw_covered_predictions']==2
assert r['counts']['raw_wrong_covered_predictions']==1
assert r['counts']['raw_uncovered_records']==1
assert r['counts']['raw_full_denominator_failures_including_noncoverage']==2
assert r['raw_breaks_covered_predictions_only']==['1']
assert r['raw_uncovered_records']==['2']
`], { cwd: new URL("..", import.meta.url), env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" }, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
});

test("independent word gold retains missing membership and scores actual hybrid owners", () => {
  const result = spawnSync("python3", ["-c", `
import importlib.util
s=importlib.util.spec_from_file_location('evaluation','scripts/evaluate-pronunciation.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
owner={'track':'t','part':None,'staff':None,'voice':None,'occurrence':1,'segment':None,'lane':'lyrics','verse':1}
records=[{'source_sha256':'source','words':[{'source_word':{'owner':owner,'members':['0'],'key':'word'},'baseline_owner':'en','hybrid_owner':'fr','raw_laya_choice':'pt'}]}]
gold={'format':'verse.pronunciation-gold','schema_version':1,'records':[{'id':str(i),'source_sha256':'source','owner':owner,'members':[str(i)],'accepted_keys':['word'],'accepted_languages':['en'],'song_family_id':'family','split':'test'} for i in range(2)]}
r=m.metrics(records,gold)['counts']
assert r['denominator']==2 and r['baseline_correct']==1 and r['hybrid_correct']==0 and r['missing_membership']==1
`], { cwd: new URL("..", import.meta.url), env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" }, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
});

test("owner-qualified maps keep two verses sharing source note IDs separate", () => {
  const result = spawnSync("python3", ["-c", `
import importlib.util
s=importlib.util.spec_from_file_location('evaluation','scripts/evaluate-pronunciation.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
sha='66ad238c35ed838e95b9eebb7bc421168539a600eaab055b69b234663d867fff'
owners=[{'track':'t','part':'P1','staff':'1','voice':'1','occurrence':1,'segment':0,'lane':'lyrics','verse':v} for v in [1,2]]
notes=[{'owner':o,'id':'shared','raw':'word','continues':False,'verse':o['verse'],'staff':'1','baseline_owner':lang,'hybrid_owner':lang} for o,lang in zip(owners,['en','fr'])]
words=[{'source_word':{'owner':o,'members':['shared'],'key':'word'},'baseline_owner':lang,'hybrid_owner':lang,'raw_laya_choice':lang} for o,lang in zip(owners,['en','fr'])]
records=[{'source_sha256':sha,'notes':notes,'words':words}]
assert m.eligible_development_oracle(records)[0]['counts']['raw_laya_correct']==2
gold={'format':'verse.pronunciation-gold','schema_version':1,'records':[{'id':str(o['verse']),'source_sha256':sha,'owner':o,'members':['shared'],'accepted_keys':['word'],'accepted_languages':[lang],'song_family_id':'family','split':'development'} for o,lang in zip(owners,['en','fr'])]}
assert m.metrics(records,gold)['counts']['baseline_correct']==2
`], {cwd:new URL("..",import.meta.url),env:{...process.env,PYTHONDONTWRITEBYTECODE:"1"},encoding:"utf8"});
  assert.equal(result.status,0,result.stderr);
});
