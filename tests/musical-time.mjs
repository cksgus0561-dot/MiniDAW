import ts from 'typescript';
import {readFile} from 'node:fs/promises';
import assert from 'node:assert/strict';
const code=ts.transpileModule(await readFile('src/musical-time.ts','utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText;
const {MusicalClock}=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const clock=new MusicalClock();
assert.equal(clock.seconds(960000),.5);assert.equal(clock.ticks(2),3840000);
assert.equal(clock.label(3840000),'2.1.0');assert.equal(clock.parse('2.1.0'),3840000);
assert.equal(clock.snap(.74,'beat'),.5);assert.equal(clock.snap(.76,'beat'),1);
assert.equal(clock.snap(.18,'8'),.25);
assert.equal(clock.preciseLabel(0),'1.1.1.000000');
assert.equal(clock.preciseLabel(240001),'1.1.2.000001');
assert.equal(clock.preciseLabel(959999),'1.1.4.239999');
assert.equal(clock.preciseLabel(960000),'1.2.1.000000');
assert.equal(clock.editTick(.12500052,null),240001);
assert.equal(clock.editTick(.017,'128'),30000);
clock.time.tempoMap[0].bpm=150;
clock.time.timeSignatures[0]={tick:'0',numerator:7,denominator:8};
assert.equal(clock.parse('2.1.0'),3360000);assert.equal(clock.label(3360000),'2.1.0');
assert.ok(Math.abs(clock.seconds(3360000)-1.4)<1e-9);
assert.equal(clock.parse('1.8.0'),null);
assert.equal(clock.preciseLabel(479999),'1.1.2.239999');
assert.equal(clock.preciseLabel(480000),'1.2.1.000000');
assert.equal(clock.preciseLabel(3360000),'2.1.1.000000');
for(const bpm of [1,87.3,137.5,1000]){
 clock.time.tempoMap[0].bpm=bpm;
 for(const tick of [0,123,960000,7000000000])assert.ok(Math.abs(clock.ticks(clock.seconds(tick))-tick)<.00001);
 for(const initial of [1,30001,1234567,7000000001]){
   let tick=initial;
   for(let i=0;i<10000;i++)tick=clock.editTick(Math.round(clock.seconds(tick)*1e9)/1e9,null);
   assert.equal(tick,initial,'Repeated tick/seconds serialization must not drift');
 }
}
clock.time.tempoMap=[{tick:'0',bpm:120},{tick:'3840000',bpm:60}];
assert.equal(clock.seconds(4800000),3);assert.equal(clock.ticks(3),4800000);
clock.time.timeSignatures=[{tick:'0',numerator:4,denominator:4},{tick:'3840000',numerator:3,denominator:4}];
assert.equal(clock.label(6720000),'3.1.0');assert.equal(clock.parse('3.1.0'),6720000);
const bars=clock.lines(0,8,800,'beat').filter(l=>l.bar);
assert.deepEqual(bars.map(l=>l.x),[0,200,500,800]);
assert.ok(clock.lines(0,600000,800,'32').length<2001);
console.log('Musical clock: PPQ, tempo map roundtrip, signatures, Bar/Beat labels, grid snapping and bounded grid density passed.');
