import {readFileSync,writeFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
const rows=JSON.parse(readFileSync('docs/validation/src-candidates.json'));
const unpack=b=>Array.from({length:b.length/4},(_,i)=>b.readFloatLE(i*4));
const amp=(s,r,hz)=>{let re=0,im=0,a=r/10,b=r*9/10;for(let i=a;i<b;i++){let p=2*Math.PI*hz*i/r;re+=s[i*2]*Math.cos(p);im+=s[i*2]*Math.sin(p);}return 2*Math.hypot(re,im)/(b-a);};
const rmsDb=(s,r)=>20*Math.log10(Math.sqrt(s.slice(r/10*2,r*9/10*2).reduce((sum,x)=>sum+x*x,0)/(r*.8*2))/(.5/Math.sqrt(2)));
const refs=[];
for(const row of rows.filter(r=>r.candidate==='linear')){
 const {inputRate:ir,outputRate:or,signal}=row,stem=`tests/generated/src-upgrade/${ir}-${or}-${signal}`;
 execFileSync('ffmpeg',['-v','error','-y','-f','f32le','-ar',String(ir),'-ac','2','-i',stem+'-input.f32','-af',`aresample=${or}:resampler=soxr:precision=28:cheby=0`,'-c:a','pcm_f32le','-f','f32le',stem+'-soxr.f32'],{windowsHide:true});
 const ref=unpack(readFileSync(stem+'-soxr.f32'));assert.equal(ref.length,or*2);
 const tones=row.tones.map(t=>({...t,amplitude:amp(ref,or,t.measuredHz),gainDb:20*Math.log10(amp(ref,or,t.measuredHz)/(.5/row.tones.length)),imageDb:t.imageDb===null?null:20*Math.log10(amp(ref,or,t.imageHz)/(.5/row.tones.length))}));
 refs.push({inputRate:ir,outputRate:or,signal,candidate:'soxr28',tones,interiorRmsDb:rmsDb(ref,or)});
 for(const r of rows.filter(r=>r.inputRate===ir&&r.signal===signal)){
  const actual=unpack(readFileSync(stem+`-${r.candidate}.f32`));let max=0,sq=0;
  for(let i=or/10*2;i<or*9/10*2;i++){let e=actual[i]-ref[i];max=Math.max(max,Math.abs(e));sq+=e*e;}
  r.maxVsSoxr=max;r.rmsVsSoxr=Math.sqrt(sq/(or*.8*2));
  r.interiorRmsDb=rmsDb(actual,or);
  if(signal==='1k') {
   let re=0,im=0;for(let i=or/10;i<or*9/10;i++){let p=2*Math.PI*1000*i/or;re+=actual[i*2]*Math.cos(p);im+=actual[i*2]*Math.sin(p);}
   r.residualDelaySamples=-Math.atan2(re,im)*or/(2*Math.PI*1000);
  }
  assert.equal(r.nonfinite,0);assert.equal(r.channelMismatch,0);
 }
}
writeFileSync('docs/validation/src-candidates-reference.json',JSON.stringify({reference:'FFmpeg libsoxr precision=28 cheby=0, coherent 0.1–0.9s interior; f32 input/output, f64 DSP',rows:[...rows,...refs]},null,2));
for(const ir of [44100,48000]){
 console.log('Rate',ir);
 console.table([...rows,...refs].filter(r=>r.inputRate===ir&&['15k','nearNyquist','alias'].includes(r.signal)).map(r=>({candidate:r.candidate,signal:r.signal,dB:r.tones[0].gainDb,imageDb:r.tones[0].imageDb,dspMs:r.dspTotalMs,delay:r.delayFrames,blockMaxUs:r.blockMaxUs})));
}
