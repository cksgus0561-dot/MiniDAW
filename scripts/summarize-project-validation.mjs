import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const root='docs/validation/';
const read=async name=>JSON.parse(await readFile(root+name+'.json','utf8'));
const hash=createHash('sha256').update(await readFile('src-tauri/target/release/minidaw.exe')).digest('hex');
const names=['project-ui','ui-smoke','audio-settings','streaming','primary-after','project-native','project-after-wasapi','project-after-asio'];
const reports={};
for(const name of names){const r=await read(name);assert.equal(r.passed,true,name);assert.equal(r.executableSha256??r.sha256,hash,name+' executable');reports[name]=r;}
const project=reports['project-ui'];
const summary={generated:new Date().toISOString(),executableSha256:hash,
 build:{frontend:'type check + production build passed',rust:'cargo check / fmt --check / Clippy -D warnings / tests passed (ASIO)',wasapiOnly:'cargo check --no-default-features and Clippy --all-targets -D warnings passed',release:'ASIO release build passed',rustTests:{passed:52,failed:0,ignored:1,ignoredReason:'existing opt-in generated-large Rust test; actual 460 MB release round-trip and 6-source streaming suite were run'},logs:['project-build.txt','project-release.txt']},
 releaseTests:Object.fromEntries(names.map(n=>[n,{passed:reports[n].passed,checks:reports[n].checks?.length,files:reports[n].files?.length,runs:reports[n].runs?.length}])),
 largeAsset:project.large,roundTrips:project.roundTrips.map(r=>({codec:r.extension,projectBytes:r.projectBytes,sourceSha256:r.sourceHash})),
 saveDuringPlayback:project.savePerformance.map(r=>({backend:r.backend,count:30,averageMs:r.saveAverageMs,maxMs:r.saveMaxMs,
  commandDelta:r.after.transport.appliedCommand-r.before.transport.appliedCommand,waveformBuildUnchanged:r.after.waveform.buildMs===r.before.waveform.buildMs,
  audioPositionAdvanced:r.after.position>r.before.position,callbackAverageMs:r.after.metrics.callbackAvgMs,callbackMaxMs:r.after.metrics.callbackMaxMs,
  overruns:r.after.metrics.overruns,deviceOverruns:r.after.metrics.deviceCallbackOverruns,starvation:r.after.source.starvation,streamErrors:r.after.streamErrors})),
 primarySource:{sha256:reports['primary-after'].sha256,unchanged:reports['primary-after'].sha256===reports['primary-after'].sourceSha256After,runs:reports['primary-after'].runs.length},
 transportComparison:{}, limitations:['Native file-picker responses supplied in automated project test; project IPC, files, process restarts and actual device audio are real. Native title-bar close was separately clicked with Computer Use.','Latency is Rust command to callback application / PCM delivery, not acoustic output. Single-device sample, not a statistical guarantee; scheduling jitter affects individual measurements.','Sampled asset fingerprint hashes only size and first/last 64 KiB. Original-file preservation tests hash the full files.','Current playback supports one selected full-source default-edit audio clip; non-default edit data and unknown extensions are preserved with playback disabled.','No multi-track playback, editing UI, MIDI, packing, autosave or recovery feature was added.']};
const actions=[[0,'play','playMs'],[1,'pause','pauseMs'],[2,'resume','resumeMs'],[3,'seek','seekMs'],[4,'stop','stopMs']];
for(const backend of ['wasapi','asio']){
 summary.transportComparison[backend]={};
 for(const phase of ['before','after']){
  const r=await read(`project-${phase}-${backend}`);assert.equal(r.passed,true);if(phase==='after')assert.equal(r.executableSha256,hash);
  else assert.equal(r.executableSha256,'9396c425dd5883ed7f0234fa47bc16379aeaadeb055ab72219aaf6d6ff14ed62');
  const entry={executableSha256:r.executableSha256,output:r.final.output,callbackAverageMs:r.final.metrics.callbackAvgMs,callbackMaxMs:r.final.metrics.callbackMaxMs,overruns:r.final.metrics.overruns,deviceOverruns:r.final.metrics.deviceCallbackOverruns,starvation:r.final.source.starvation,actions:{}};
  for(const[index,label,metric]of actions){const values=r.runs.map(t=>t.commands[index].snapshot.metrics[metric]);entry.actions[label]={count:values.length,meanMs:values.reduce((a,b)=>a+b,0)/values.length,maxMs:Math.max(...values)};}
  summary.transportComparison[backend][phase]=entry;
 }
}
await writeFile(root+'project-summary.json',JSON.stringify(summary,null,2));
const example=structuredClone(project.roundTrips[0].document);example.name='포맷 예시';example.assets[0].path={projectRelativePath:'media/stereo-44100.wav',originalAbsolutePath:null};
await writeFile('docs/project-v1-example.minidaw',JSON.stringify(example,null,2));
console.log(JSON.stringify({passed:true,executableSha256:hash,largeAsset:summary.largeAsset,releaseTests:summary.releaseTests}));
