import ts from 'typescript';
import {readFile} from 'node:fs/promises';
import assert from 'node:assert/strict';
async function module(path){const code=ts.transpileModule(await readFile(path,'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText;return import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));}
const {MusicalClock}=await module('src/musical-time.ts'),{parseNoteTime:parse,formatNoteTime:format}=await module('src/note-time.ts');
const clock=new MusicalClock();
assert.equal(parse(clock,'2.3.4.239999','start',0,false),6719999);
assert.equal(parse(clock,'0.1.2.000123','length',0,false),1440123);
for(const denominator of [2,4,8,16,32,64,128]) {
 clock.time.timeSignatures=[{tick:'0',numerator:7,denominator}];
 for(const tick of [0,1,123,239999,240000,959999,960000,123456789]){
  assert.equal(parse(clock,format(clock,tick,'start',0,false),'start',0,false),tick);
  if(tick)assert.equal(parse(clock,format(clock,tick,'length',123,false),'length',123,false),tick);
 }
}
clock.time.timeSignatures=[{tick:'0',numerator:4,denominator:4},{tick:'3840000',numerator:7,denominator:8}];
for(const tick of [3839999,3840000,3840001,7000001])assert.equal(parse(clock,format(clock,tick,'end',0,false),'end',0,false),tick);
for(const text of ['1.0.1.000000','0.1.1.000000','1.1.0.000000','1.1.1.240000','1.5.1.000000','1.1.5.000000','1.1.1.-1','1.1.1.1e3','1.1.1.9007199254740993'])assert.equal(parse(clock,text,'start',0,false),null,text);
for(const text of ['-1','1.5','1e3','9007199254740992'])assert.equal(parse(clock,text,'start',0,true),null);
assert.equal(parse(clock,'0','length',0,true),null);assert.equal(parse(clock,'1','length',0,true),1);assert.equal(parse(clock,'9007199254740991','start',0,true),Number.MAX_SAFE_INTEGER);
console.log('Note Start/End/Length: exact musical/tick roundtrip, signature changes, 2–128 denominators, invalid inputs passed.');
