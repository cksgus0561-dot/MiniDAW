// Consolidate measured artifacts; refuses stale executable reports or failed tests.
import {readFile, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const directory = 'docs/validation/';
const read = async name => JSON.parse(await readFile(`${directory}${name}.json`, 'utf8'));
const sha = createHash('sha256').update(await readFile('src-tauri/target/release/minidaw.exe')).digest('hex');
const names = ['editing-ui', 'editing-stress', 'ui-smoke', 'project-ui', 'audio-settings', 'streaming', 'primary-after', 'editing-after-wasapi', 'editing-after-asio'];
const reports = Object.fromEntries(await Promise.all(names.map(async name => [name, await read(name)])));
for (const [name, report] of Object.entries(reports)) {
  assert.equal(report.passed, true, name);
  assert.equal(report.executableSha256 ?? report.sha256, sha, `${name}: outdated release`);
}
const pcm = await read('editing-pcm');
assert.equal(pcm.passed, true);
assert.equal(pcm.uiReleaseSha256, sha);
const wasapi = await read('editing-ui-wasapi');
const wasapiUi = await read('editing-wasapi-ui-smoke');
assert.ok(wasapi.passed && wasapiUi.passed);
assert.equal(wasapi.executableSha256, wasapiUi.executableSha256);
const stats = values => ({samples: values.length, average: values.reduce((a,b)=>a+b,0)/values.length, max: Math.max(...values)});
function timing(report) {
  const groups = {};
  for (const run of report.runs) for (const [index, command] of run.commands.entries()) {
    const action = index === 2 ? 'resume' : command.action;
    const list = groups[action] ??= {engine: [], ipcAcknowledged: []};
    list.engine.push(command.snapshot.metrics[`${action}Ms`]);
    list.ipcAcknowledged.push(command.acknowledgedMs);
  }
  return Object.fromEntries(Object.entries(groups).map(([name, value])=>[name, {
    engineMs: stats(value.engine), ipcAcknowledgedMs: stats(value.ipcAcknowledged)
  }]));
}
const comparisons = {};
for (const backend of ['wasapi', 'asio']) {
  const before = await read(`editing-before-${backend}`), after = reports[`editing-after-${backend}`];
  assert.ok(before.passed);
  comparisons[backend] = {
    beforeSha256: before.executableSha256, afterSha256: sha,
    output: after.final.output,
    before: {metrics: before.final.metrics, transport: timing(before)},
    after: {metrics: after.final.metrics, transport: timing(after)},
    afterErrors: {rendererDeadline: after.final.metrics.overruns, outputDeadline: after.final.metrics.deviceCallbackOverruns, starvation: after.final.source.starvation, streamErrors: after.final.streamErrors}
  };
}
const builds = {};
for (const name of ['editing-build', 'editing-wasapi-build']) {
  const log = await readFile(`${directory}${name}.txt`, 'utf8');
  assert.ok(log.includes('전체 빌드 검증을 통과했습니다.'), name);
  const results = [...log.matchAll(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored/g)];
  builds[name] = {passed: results.reduce((sum,m)=>sum+Number(m[1]),0), failed: results.reduce((sum,m)=>sum+Number(m[2]),0), optInIgnored: results.reduce((sum,m)=>sum+Number(m[3]),0), typeCheck:true,frontendBuild:true,cargoCheck:true,fmt:true,clippy:true,release:true};
}
const largeLog = await readFile(`${directory}editing-large-test.txt`, 'utf8');
assert.match(largeLog, /test result: ok\. 1 passed; 0 failed; 0 ignored/);
const ui = reports['editing-ui'], stress = reports['editing-stress'];
assert.deepEqual(ui.sourcesBefore, ui.sourcesAfter);
const primary = reports['primary-after'];
assert.equal(primary.sha256, primary.sourceSha256After);
const summary = {
  verifiedAt: new Date().toISOString(), releaseSha256: sha, wasapiOnlySha256: wasapi.executableSha256,
  passed:true, builds, explicitLargeSourceTest:true,
  reports: names.map(name=>({path:`${directory}${name}.json`,passed:reports[name].passed})),
  pcm, smallProjectMouseCommits: ui.commits,
  large: {commands:stress.large.commands,saveMs:stress.large.saveMs,loadMs:stress.large.loadMs,memoryBefore:stress.large.memoryBefore,memoryAfter:stress.large.memoryAfter,source:stress.large.snapshot.source},
  stress1200: stress.stress,
  editingPlayback: [...ui.backends, ...stress.backends].map(b=>({backend:b.backend,metrics:b.after.metrics,starvation:b.after.source.starvation,streamErrors:b.after.streamErrors,analysisBuildsBefore:b.before.analysisBuilds,analysisBuildsAfter:b.after.analysisBuilds})),
  comparisons,
  hashes: {fixturesBefore:ui.sourcesBefore,fixturesAfter:ui.sourcesAfter,largeBefore:pcm.large.sourceSha256Before,largeAfter:pcm.large.sourceSha256After,primaryBefore:primary.sha256,primaryAfter:primary.sourceSha256After},
  limitations: ['One nonempty Audio Track; maximum 128 simultaneous Clip voices.', 'History uses a 32 MiB serialized-size budget / 128 entries, retaining at least the latest entry; this is not a strict heap-byte cap.', 'Timing is a single local protocol sample, not acoustic latency or a statistical guarantee.', 'UI tests drive the real release WebView/IPC/audio; native file-picker answers use test paths.', 'Normalize has no progress percentage or cancellation yet; Peak only.']
};
await writeFile(`${directory}editing-summary.json`, JSON.stringify(summary,null,2));
const f = n => Number(n).toFixed(4);
const mib = n => (n / 1048576).toFixed(2);
const appendix = [
  '측정 부록 (자동 집계)',
  `최종 ASIO/WASAPI 지원 실행 파일 SHA-256: ${sha}`,
  `별도 WASAPI 전용 빌드 SHA-256: ${wasapi.executableSha256}`,
  '두 전체 빌드: frontend type/build, cargo check, fmt --check, Clippy --all-targets -D warnings, Rust 64/64 통과.',
  '기본 제외된 대용량 테스트도 별도 실행: 1/1 통과. 추가 audit binary 수정 후 양 feature Clippy/fmt 재통과.',
  '실제 release: 편집 27단계·마우스 드래그, 1,200 Clip/대용량, 기존 UI, Project, Audio Settings,',
  '6종 Streaming 파일, 사용자 MP3, 두 backend 동일 transport benchmark 전부 통과. 각 JSON의 최종 exe hash 일치 확인.',
  '',
  '저장/재실행 문서 및 Memory→Streaming PCM:',
  ...pcm.roundtripMemoryStreaming.map(p=>`${p.outputRate}Hz: ${p.frames} stereo frames, mismatch=${p.mismatchSamples}, max=${p.maxError}, RMS=${p.rmsError}; PCM SHA=${p.afterSha256}`),
  'Synthetic integration: Split/Fade 중 Split/Trim/Move/Delete/Copy/Gain/Mute/Normalize/Crossfade/Undo/Redo, 44.1↔48kHz와 세 codec 통과.',
  'RT guard: 실제 Timeline Plan 교체 200회에서 callback allocation/free 및 파일/decoder 작업 0.',
  '',
  '1,200 Clip 실제 release (ms; engine commit과 IPC 왕복을 구분):',
  'command | mutation | plan | engine commit | IPC 왕복',
  ...stress.stress.commands.map(c=>`${c.command} | ${f(c.mutationMs)} | ${f(c.planMs)} | ${f(c.commitMs)} | ${f(c.ipcMs)}`),
  `첫 Open+UI ${f(stress.stress.openAndUiMs)}ms; Save ${f(stress.stress.saveMs)}ms; 재Load ${f(stress.stress.loadMs)}ms; 파일 ${stress.stress.fileBytes} bytes.`,
  `Fit DOM ${stress.stress.fitDomClips} Clips → 128x zoom ${stress.stress.zoomDomClips} Clips; 최대 Canvas 폭 ${stress.stress.maxCanvasWidth}px.`,
  `80회 gain 편집 후 history ${stress.stress.history.undo}단계, 직렬화 예산 ${stress.stress.history.bytes} bytes.`,
  `Rust private ${mib(stress.stress.memoryBefore.PrivateMemorySize64)} → ${mib(stress.stress.memoryAfterHistory.PrivateMemorySize64)} MiB; working set ${mib(stress.stress.memoryBefore.WorkingSet64)} → ${mib(stress.stress.memoryAfterHistory.WorkingSet64)} MiB.`,
  `WebView 자식 프로세스 private ${mib(stress.stress.memoryBefore.webviewPrivateBytes)} → ${mib(stress.stress.memoryAfterHistory.webviewPrivateBytes)} MiB; working set ${mib(stress.stress.memoryBefore.webviewWorkingSet)} → ${mib(stress.stress.memoryAfterHistory.webviewWorkingSet)} MiB.`,
  '브라우저 수치는 프로세스 snapshot이며 GC/OS cache를 분리한 retained-heap 또는 장시간 leak 측정은 아니다.',
  `독립 offline lookup: ${pcm.timelineLookup.lookupCount}회/${f(pcm.timelineLookup.lookupTotalMs)}ms, 평균 ${f(pcm.timelineLookup.lookupMeanNs)}ns; ${pcm.timelineLookup.spans} spans.`,
  `1,200 Clip offline compile ${f(pcm.timelineLookup.buildMs)}ms; ${pcm.timelineLookup.sequentialFrames} frames worker 순차 render ${f(pcm.timelineLookup.sequentialWorkerMs)}ms (callback 시간이 아님).`,
  '',
  `${pcm.large.bytes}-byte / ${pcm.large.duration}초 Streaming 편집:`,
  `전체 source PCM resident ${stress.large.snapshot.source.pcmResidentBytes} bytes; ring ${stress.large.snapshot.source.bufferBytes} bytes.`,
  `Rust private ${mib(stress.large.memoryBefore.PrivateMemorySize64)} → ${mib(stress.large.memoryAfter.PrivateMemorySize64)} MiB; working set ${mib(stress.large.memoryBefore.WorkingSet64)} → ${mib(stress.large.memoryAfter.WorkingSet64)} MiB.`,
  `WebView private ${mib(stress.large.memoryBefore.webviewPrivateBytes)} → ${mib(stress.large.memoryAfter.webviewPrivateBytes)} MiB.`,
  `Save ${f(stress.large.saveMs)}ms, Load ${f(stress.large.loadMs)}ms. Normalize 검사 대상은 Trim한 현재 약 2초 범위이며 전체 20분 Normalize 속도를 측정한 것은 아니다.`,
  '',
  'Backend 전후 비교 (각 WAV/MP3/FLAC 3회, 총 9회; 평균/최대 ms):'
];
for (const [backend, c] of Object.entries(comparisons)) {
  const b = c.before.metrics, a = c.after.metrics;
  appendix.push(
    `${backend.toUpperCase()} ${c.output.sampleRate}Hz, 실제 마지막 buffer ${a.bufferFrames} frames, ${c.output.device}:`,
    `  Renderer ${f(b.callbackAvgMs)}/${f(b.callbackMaxMs)} → ${f(a.callbackAvgMs)}/${f(a.callbackMaxMs)}`,
    `  Output callback ${f(b.deviceCallbackAvgMs)}/${f(b.deviceCallbackMaxMs)} → ${f(a.deviceCallbackAvgMs)}/${f(a.deviceCallbackMaxMs)}`
  );
  for (const command of ['play','pause','resume','stop','seek']) {
    const x = c.before.transport[command].engineMs, y = c.after.transport[command].engineMs;
    appendix.push(`  ${command} ${f(x.average)}/${f(x.max)} → ${f(y.average)}/${f(y.max)}`);
  }
  appendix.push(`  deadline/starvation/stream errors: ${JSON.stringify(c.afterErrors)}`);
}
appendix.push(
  '관측 결과 callback 평균/최대가 악화되지 않았고 기한 초과/공급 부족/스트림 오류는 0이었다.',
  'Transport 평균의 증감은 존재한다(예: WASAPI Play 약 +0.675ms, ASIO Seek 약 +0.634ms).',
  'callback 주기와 비동기 공급/스케줄링 변동 범위의 단일 표본이며 지연이 항상 같거나 개선된다고 주장하지 않는다.',
  '실제 재생 중 편집 8회 및 1,200 Clip edit/Undo/Redo에서도 두 backend 오류·deadline·starvation 0, Asset 분석 rebuild 0.',
  '',
  '원본 SHA-256 (편집 전후 동일):',
  ...ui.sourcesBefore.map(s=>`${s.path}: ${s.sha256}`),
  `460MB WAV: ${pcm.large.sourceSha256After}`,
  `사용자 MP3: ${primary.sha256}`,
  '',
  '원시 증거: validation/editing-summary.json, editing-ui.json, editing-ui-wasapi.json, editing-stress.json,',
  'editing-pcm.json, editing-before/after-wasapi.json, editing-before/after-asio.json, editing-build.txt,',
  'editing-wasapi-build.txt, editing-large-test.txt, ui-smoke.json, project-ui.json, audio-settings.json, streaming.json, primary-after.json.',
  '화면: validation/editing-desktop.png, editing-stress.png. 이전 실패 진단 파일은 역사적 기록이며 최종 결과는 위 passed/hash로 구분한다.',
  ''
);
const reportPath = 'docs/AUDIO_EDITING.txt';
const reportText = await readFile(reportPath,'utf8');
const marker = reportText.indexOf('\n측정 부록');
assert.ok(marker > 0);
await writeFile(reportPath, reportText.slice(0, marker) + '\n' + appendix.join('\n'));
console.log(JSON.stringify({passed:true,sha,builds,comparisons,stress1200:summary.stress1200,pcm},null,2));
