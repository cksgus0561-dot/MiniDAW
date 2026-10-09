import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const read=p=>JSON.parse(readFileSync(`docs/validation/${p}.json`));
let frames=0,seeks=0;
for(const file of [...read('compare'),...read('primary-pcm')]){
 const all=[...file.sequentialVsMemory,...file.memoryVsStreaming,...file.seeks.flatMap(s=>s.channels)];
 for(const c of all){assert.equal(c.exactMismatch,0,file.path);assert.equal(c.firstMismatch,null);assert.equal(c.maxAbs,0);assert.equal(c.rms,0);assert.equal(c.nan,0);assert.equal(c.inf,0);}
 assert.equal(file.streamAfterFullPass.error,null);assert.equal(file.waveform.error,null);assert.equal(file.waveform.progress,1);
 frames+=file.file.frames;seeks+=file.seeks.length;
}
for(const c of read('conversion')){assert.equal(c.unsupported,undefined);assert.equal(c.correctlyRoundedSamples,c.channels.reduce((s,ch)=>s+ch.samples,0));if(c.flacVsFfmpeg)assert.equal(c.flacVsFfmpeg.exactMismatch,0);if(['pcm16','pcm24','mono16','float32'].includes(c.name))for(const ch of c.channels)assert.equal(ch.exactMismatch,0);if(c.name==='float32'){assert.equal(c.firstSamples[3],1.25);assert.equal(c.firstSamples[4],-1.5);}}
console.log(JSON.stringify({files:read('compare').length+1,frames,seeks,exactComparisonsPassed:true}));
