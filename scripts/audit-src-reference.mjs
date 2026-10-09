// Installed FFmpeg/libsoxr is an offline independent reference, not a runtime dependency.
import {readFileSync,writeFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
const report=JSON.parse(readFileSync('docs/validation/src-after.json'));
const unpack=b=>Array.from({length:b.length/4},(_,i)=>b.readFloatLE(i*4));
const amplitude=(s,rate,hz)=>{let re=0,im=0,n=s.length/2;for(let i=0;i<n;i++){let p=2*Math.PI*hz*i/rate;re+=s[i*2]*Math.cos(p);im+=s[i*2]*Math.sin(p);}return 2*Math.hypot(re,im)/n;};
const results=[];
for(const m of report.measurements){
 const stem=`tests/generated/fidelity/src/${m.inputRate}-${m.outputRate}-${m.signal}`;
 const ref=`${stem}-soxr.f32`;
 execFileSync('ffmpeg',['-v','error','-y','-f','f32le','-ar',String(m.inputRate),'-ac','2','-i',`${stem}-input.f32`,'-af',`aresample=${m.outputRate}:resampler=soxr:precision=28:cheby=0`,'-c:a','pcm_f32le','-f','f32le',ref]);
 const actual=unpack(readFileSync(`${stem}-output.f32`)),reference=unpack(readFileSync(ref));assert.equal(actual.length,reference.length);
 // A coherent 0.8 s interior avoids endpoint transients in spectral measurements.
 const a=Math.round(m.outputRate*.1)*2,b=Math.round(m.outputRate*.9)*2;
 let max=0,sq=0;for(let i=a;i<b;i++){let e=actual[i]-reference[i];max=Math.max(max,Math.abs(e));sq+=e*e;}
 const tones=m.tones.map(t=>({hz:t.measuredHz,miniAmplitude:amplitude(actual.slice(a,b),m.outputRate,t.measuredHz),soxrAmplitude:amplitude(reference.slice(a,b),m.outputRate,t.measuredHz),imageHz:t.imageHz,miniImage:t.imageAmplitude===null?null:amplitude(actual.slice(a,b),m.outputRate,t.imageHz),soxrImage:t.imageAmplitude===null?null:amplitude(reference.slice(a,b),m.outputRate,t.imageHz)}));
 results.push({inputRate:m.inputRate,outputRate:m.outputRate,signal:m.signal,interiorMaxAbsVsSoxr:max,interiorRmsVsSoxr:Math.sqrt(sq/(b-a)),tones});
 assert.equal(m.nonfinite,0);assert.equal(m.channelMismatch,0);
}
writeFileSync('docs/validation/src-reference.json',JSON.stringify({reference:'FFmpeg 8.1.1 libsoxr, precision=28, cheby=0; 0.1–0.9 s coherent window',results},null,2));
console.log('Independent SRC reference measurements saved; see SRC_UPGRADE.txt for measured quality and transition-band limits.');
