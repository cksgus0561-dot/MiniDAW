import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const dir='docs/validation/';
const read=name=>JSON.parse(readFileSync(dir+name+'.json','utf8').replace(/^\uFEFF/,''));
const hash=createHash('sha256').update(readFileSync('src-tauri/target/release/minidaw.exe')).digest('hex');
const cases={baseline:read('asio-baseline/transport')};
for(const backend of ['wasapi','asio'])for(const mode of ['off','on'])cases[`${backend}-${mode}`]=read(`transport-${backend}-${mode}`);
const gui=['audio-settings','ui-smoke','streaming','primary-after','asio-panel','streaming-wasapi','streaming-asio','primary-wasapi','primary-asio'].map(read);
for(const [key,r] of Object.entries(cases)){assert.equal(r.passed,true,key);if(key!=='baseline')assert.equal(r.executableSha256,hash,key);}
for(const r of gui){assert.equal(r.passed,true);assert.equal(r.executableSha256??r.sha256,hash);}
const range=xs=>{const a=xs.filter(x=>typeof x==='number');return a.length?{min:Math.min(...a),max:Math.max(...a),mean:a.reduce((s,x)=>s+x,0)/a.length}:null;};
const summary={executableSha256:hash,cases:{},tools:read('asio-tools'),declick:read('declick-audit').production};
for(const [key,r] of Object.entries(cases)) {
 const values={};for(const [index,action] of ['Play','Pause','Resume','Seek','Stop'].entries()) {
  const commands=r.runs.map(run=>run.commands[index]);const metric={Play:'playMs',Pause:'pauseMs',Resume:'resumeMs',Seek:'seekMs',Stop:'stopMs'}[action];
  values[action]={callbackMs:range(commands.map(c=>c.snapshot.metrics[metric])),ipcAndPollAckMs:range(commands.map(c=>c.acknowledgedMs))};
 }
 summary.cases[key]={output:r.final.output,metrics:r.final.metrics,actions:values,starvation:r.final.source.starvation};
}
const fmt=n=>typeof n==='number'?n.toFixed(4):'미측정';
const bounds=r=>r?`${fmt(r.min)}–${fmt(r.max)} (평균 ${fmt(r.mean)})`:'미측정';
const lines=[];const line=(...s)=>lines.push(...s);
line('MiniDAW Windows Audio Output / Transport De-click 검증','2026-10-04',`최종 release SHA256: ${hash}`,'');
line('1. ASIO 구현 방식','CPAL 0.16 HostId::Asio로 driver/device를 초기화한다. 기본 CPAL/asio-sys ASIO dispatcher는 callback mutex가 있어 쓰지 않고, output-only SDK adapter에서 미리 할당한 stereo/interleaved 버퍼와 driver native 버퍼를 연결한다. SDK 헤더는 외부 빌드 경로에서만 참조한다. 녹음/input stream은 생성하지 않는다.','');
line('2. CPAL/backend 설정','Cargo optional feature asio = cpal/asio + asio-sys 0.2.6. WASAPI는 HostId::Wasapi shared mode를 유지한다. ASIO feature 없는 빌드도 SDK 없이 cargo check/Clippy를 통과했다. scripts/with-asio.ps1의 SdkPath/LibClangPath 인자를 사용하며 PATH/CARGO_HOME/RUSTUP_HOME/npm/global environment는 변경하지 않았다. 도구 출처와 SHA256은 asio-tools.json. SDK/LLVM은 저장소/번들/installer에 들어가지 않는다.','');
line('3. 실제 ASIO 장치','실제 열거: Topping USB Audio Device. 설치 DLL 버전 5.68.0.0. 실제 native Control Panel에서 DX3 Pro+ 확인. ASIO device open + WAV/MP3/FLAC, 44.1k SRC 및 48k bypass 출력 성공.','');
line('4. WASAPI↔ASIO 전환','제어 스레드가 이전 callback을 중지하고 소유자를 해제한 뒤 선택한 backend/device를 연다. transport는 정지/0으로 재설정된다. 장치 rate가 달라지면 AudioSource::for_output로 worker SRC를 준비하며 waveform Analysis는 유지한다.','');
line('5. ASIO 실패 동작','고의로 없는 ASIO 이름을 저장하고 앱 재시작: 출력 없음 + 한국어 오류를 확인했다. 자동 WASAPI fallback 없음. 사용자가 UI에서 WASAPI 선택/적용하여 복구했다. 드라이버 reset/rate/latency 변경은 atomic 오류로 표시하고 명시적 출력 재적용을 요구한다.','');
line('6. preference 저장','%APPDATA%/local.minidaw.desktop/audio-preferences.json. driverType, outputDevice, asioDriver, transportDeclick을 temp+rename으로 저장. 음악 프로젝트/원본 파일에 저장하지 않는다. 실제 앱 프로세스 재시작으로 복원 검증.','');
line('7. 실제 출력 상태','WASAPI: TOPPING USB DAC / 48000 Hz / F32 / stereo / steady 480 frames (10ms), 시작 callback 1056 관측.','ASIO: Topping USB Audio Device / 48000 Hz / I32 / stereo / 64 frames (1.3333ms). 드라이버 preferred buffer 사용, 임의 buffer 강제 없음.','실제 Control Panel: preferred 64, Safe Mode 체크, output latency 168 frames (3.50ms), input latency 112 (2.33ms). 드라이버 설정 변경 없이 읽고 닫았다. TOPPING 패널은 별도 process의 non-modal 창이므로 설정 변경 뒤 사용자가 출력 적용을 누른다.','');
line('8–10. 실제 latency / callback / deadline / stream error','단위 ms. callbackAvg/Max = 기존 Renderer DSP 구간; deviceCallbackAvg/Max = MiniDAW 출력 adapter까지 포함(ASIO native format 변환/OutputReady 포함). OS/드라이버 내부 처리와 실제 DAC/청취 지연은 측정하지 않았다.');
for(const [key,c] of Object.entries(summary.cases)){
 line(`${key}: frames ${c.metrics.bufferFrames}, DSP avg/max ${fmt(c.metrics.callbackAvgMs)}/${fmt(c.metrics.callbackMaxMs)}, adapter avg/max ${fmt(c.metrics.deviceCallbackAvgMs)}/${fmt(c.metrics.deviceCallbackMaxMs)}, interval avg/max ${fmt(c.metrics.callbackIntervalAvgMs)}/${fmt(c.metrics.callbackIntervalMaxMs)}, deadline ${c.metrics.overruns}/${c.metrics.deviceCallbackOverruns??'미측정'}, stream ${cases[key].final.streamErrors}, starvation ${c.starvation}`);
 for(const action of ['Play','Pause','Resume','Seek','Stop'])line(`  ${action}: Rust→callback ${bounds(c.actions[action].callbackMs)}; IPC+poll acknowledgement ${bounds(c.actions[action].ipcAndPollAckMs)}`);
}
line('','11. click/pop 원인','기존 transport는 현재 비영점 sample을 바로 0 또는 다른 위상의 새 위치 sample로 바꿨다. 이 출력 경계 불연속이 광대역 transient를 만든다. 사용자 청감 보고를 실제 결함으로 취급했다. 원본 파일/디코더/SRC 상시 음질 문제로 처리하지 않았다.','');
line('12–14. 방식 / 길이 / 선택 근거','고정 2ms raised cosine. 마지막 출력 anchor에서 새 PCM 또는 0으로 convex blend. seek의 worker 대기에서는 먼저 0으로, 준비 완료 시 새 PCM으로 연결한다. 각 rate의 weight table은 callback 시작 전에 생성한다. 48kHz=96 frames, 44.1kHz=88, 96kHz=192. 추가 lookahead/queued buffer 없음.','1/2/3/5ms × 44.1/48/96kHz × DC/DC+sine/0.98-amplitude sine/15kHz sine × zero/nonzero phase = 96 cases. 48kHz DC 0.5 step: 1ms의 peak adjacent jump 0.0167075, 2ms 0.0082670, 3ms 0.0054922, 5ms 0.0032862 (OFF 0.5). 2ms의 second-difference energy는 OFF 대비 약 -51.53dB. 3/5ms는 수정되는 attack 구간/에너지를 더 늘리므로 2ms를 기본으로 선택했다. 정상 고주파 신호의 자연스러운 sample slope까지 제거하려는 필터는 아니다.','');
line('15. transport 반응성 비용','위 8–10항에 명령별 ON/OFF/기존 baseline 범위를 보존했다. 명령 적용과 source cursor에는 대기 블록을 추가하지 않는다. Pause/Stop 위치는 즉시 고정/0이 되고 출력 tail만 최대 2ms 이어진다. Play/Resume/Seek는 목표 source frame부터 진행하며 2ms 안에 정상 레벨이 된다. callback 공급 지연과 full-amplitude 시간은 다르다.','');
line('16. 실제 Renderer sample discontinuity 전/후 (absolute f32 jump)');
const off=summary.declick.find(c=>!c.enabled),on=summary.declick.find(c=>c.enabled);
off.transitions.forEach((t,i)=>line(`${t.action}: OFF ${t.boundaryJump} → ON ${on.transitions[i].boundaryJump}`));
line('전체 transition window의 peak slope, transient difference energy, second-difference energy와 모든 candidate 수치는 declick-audit.json. OFF는 모든 생산 Renderer frame이 기존 raw sample과 exact, ON은 ramp 이후 exact임을 assert했다.','');
line('17–19. 기본값 / 저장 / OFF bypass','Transport De-click 기본 ON. UI 체크박스와 저장/적용으로 선택한다. 같은 장치에서 De-click만 변경하면 callback bounded command로 적용하며 재생 위치/stream을 재설정하지 않는다. OFF preference의 앱 재시작 복원 검증. OFF는 ramp/crossfade 계산을 건너뛰고 raw sample transition을 그대로 출력한다.','');
line('20. 데이터 분리','원본 읽기 전용. production audit에서 source PCM 벡터 unchanged assert. GUI ON/OFF에서 동일 waveform buildMs 유지. 기존 fidelity 17 files / 86,992,535 frames / 289 seeks exact; 12 SRC production signals bit-identical. 편집 fade/clip/project/offline export는 아직 미구현이며 이번 DSP를 연결하지 않았다. offline audit/worker source 경로에는 de-click이 없다.',`주 음원 SHA256 before/after: ${gui[3].sha256} / ${gui[3].sourceSha256After}`,'');
line('21. 실제 release 검증과 청감 범위','실제 release WebView/IPC/CPAL 및 TOPPING ASIO로 조작했다. Native Computer Use로 실제 TOPPING panel을 열고 확인/닫았다. 100 move 중 seek0/up1, wheel/zoom/scroll, rapid toggle, 90/110/125 font, settings restart, missing-driver recovery를 검사했다.','자동 검증은 실제 driver callback에 PCM이 공급됨을 확인하며, loopback/마이크/스피커 녹음이나 자동 청감 판정은 하지 않았다. 사용자의 실제 청취 피드백은 아래 별도 청취 메모에 기록한다.','');
line('22. 회귀 검사','Frontend type check/build, cargo check, Rust 37 tests + 대용량 opt-in 1 test, fmt check, Clippy -D warnings, ASIO release build 통과. WASAPI-only check/Clippy 통과. Callback allocator/free 검사 + Streaming I/O/decode 부재 검사를 De-click ON으로 실행했다. 기존 고품질 SRC, equal-rate bypass, Streaming, 대용량 waveform, transport/Canvas/theme/font를 유지했다.','성능은 위 실측치를 기준으로 판단한다. ON의 DSP 비용은 짧은 경계 계산/분기이며 deadline miss와 stream error는 검증 실행에서 0이었다. 평균/최대 수치는 baseline과 완전히 같지 않으며 특히 최대값에는 OS scheduling/outlier가 포함된다. 모든 PC/부하에서 동일 지연이나 zero underrun을 보장하지 않는다.','');
line('23. 남은 Windows/DAC 한계','WASAPI shared engine/device buffering, ASIO driver buffering, USB transfer, DAC filter 및 스피커 전달 지연은 callback benchmark만으로 알 수 없다. ASIO driver latency 값 역시 실제 acoustic latency 측정이 아니다. Windows scheduling jitter는 남아 있다. SDK adapter는 설치 드라이버의 ASIO contract를 전제로 하고, 지원하지 않는 sample format/실패한 장치는 오류로 표시한다.','');
line('청취 메모: release 앱 재실행 및 ON/OFF 비교 안내 후 사용자에게서 "ON한게 확실히 좋다."라는 실제 청취 피드백을 받았다. De-click ON의 청감상 개선을 사용자 확인 결과로 기록하고 기본 ON을 유지한다. 사용한 backend와 개별 transport 조작별 결과는 명시되지 않았으므로 모든 backend/조작에서 click이 완전히 제거되었다는 판정으로 확대하지 않는다.');
writeFileSync('docs/AUDIO_OUTPUT_DECLICK.txt',lines.join('\n')+'\n');
writeFileSync(dir+'audio-output-summary.json',JSON.stringify(summary,null,2));
console.log(JSON.stringify({executableSha256:hash,validatedReports:gui.length+Object.keys(cases).length,output:'docs/AUDIO_OUTPUT_DECLICK.txt'}));
