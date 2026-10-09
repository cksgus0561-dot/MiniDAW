import {readFileSync,writeFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const checks=[];
for(const [ir,or] of [[44100,48000],[48000,44100]]){
 for(const [production,benchmark] of [['1k','1k'],['mid','5k'],['high','15k'],['nearNyquist',ir===44100?'nearNyquist':'alias'],['silence','silence'],['dc','dc']]){
  const a=`tests/generated/fidelity/src/${ir}-${or}-${production}`;
  const b=`tests/generated/src-upgrade/${ir}-${or}-${benchmark}`;
  assert.ok(readFileSync(a+'-input.f32').equals(readFileSync(b+'-input.f32')),'input differs');
  const actual=readFileSync(a+'-output.f32'),expected=readFileSync(b+'-fft512.f32');
  assert.ok(actual.equals(expected),`production Renderer != chosen FFT benchmark ${ir} ${production}`);
  checks.push({inputRate:ir,outputRate:or,signal:production,samples:actual.length/4,exact:true});
 }
}
writeFileSync('docs/validation/src-production-exact.json',JSON.stringify({checks,passed:true},null,2));
console.log('12 signals: actual Renderer output bit-identical to selected candidate, including EOS.');
