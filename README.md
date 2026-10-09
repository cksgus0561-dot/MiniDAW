# MiniDAW

MiniDAW는 Windows용 오픈소스 Audio/MIDI DAW입니다. Rust/Tauri 2 기반이며,
실시간 오디오 처리와 편집 UI를 분리합니다. MiniDAW 자체 코드는 **GPLv3-only**입니다.

## 주요 기능

- 여러 Audio/MIDI Track, 비파괴 Clip 편집, Undo/Redo, 프로젝트 저장/불러오기
- Piano Roll, Velocity/CC/Pitch Bend/Sustain, MIDI 입력과 컴퓨터 키보드 연주
- 내장 Synth, 외부 VST3/CLAP 악기·Insert 및 플러그인 지연 보상(PDC)
- Mixer/Master, EQ·Compressor·Limiter·Reverb·Delay, Parameter Automation
- BPM/박자, PPQ 960000, Grid/Snap, Loop/Cycle, 정밀 숫자 편집
- Pitch 유지 Time Stretch, Pitch Shift, Tempo Sync, Glue/Bounce
- 실시간 Spectrum 및 두 Track 비교
- Offline WAV Mixdown/Selection/Track/Stem Export (16/24-bit PCM, 32-bit float)
- WASAPI Shared 및 ASIO, 한국어/영어, 단축키 재매핑, Zone/독립 패널 창

오디오 녹음은 현재 제공하지 않습니다. ASIO 드라이버와 외부 플러그인은 별도 설치합니다.

## 설치와 실행

Windows 10/11 **x64**가 필요합니다. 공개 후 이 저장소의 Releases에서
`MiniDAW_0.1.0_x64-setup.exe`를 내려받아 실행합니다. 현재 사용자의 계정에
설치되며 시작 메뉴에서 MiniDAW를 실행할 수 있습니다. 제거는 Windows 설정의
설치된 앱 → MiniDAW → 제거를 사용합니다.

Rust, Node.js, Visual Studio, ASIO SDK 또는 LLVM을 일반 사용자 PC에 설치할 필요는
없습니다. WebView2 Runtime이 없으면 설치기가 Microsoft의 공식 설치 프로그램을
다운로드하므로 인터넷 연결이 필요합니다. ASIO는 사용하는 장치 제조사의 드라이버가
필요합니다. 드라이버나 플러그인을 MiniDAW 설치판에 동봉하지 않습니다.

현재 패키지는 코드 서명 인증서가 적용되지 않았습니다. 공개 전 테스트 결과와 배포
상태는 [배포 문서](docs/DISTRIBUTION.md)를 확인하세요. 저장소/Release는 자동 게시되지 않습니다.

## 기본 사용

1. 오디오 설정에서 WASAPI 또는 ASIO 출력 장치를 선택합니다.
2. WAV/MP3/FLAC를 가져오거나 MIDI Track과 Part를 생성합니다.
3. MIDI Track에는 내장 Synth 또는 설치된 VST3/CLAP 악기를 지정합니다.
4. Space로 재생/일시정지, Stop으로 처음으로 이동합니다. 단축키는 앱 설정에서 변경할 수 있습니다.
5. 파일 메뉴에서 `.minidaw` 프로젝트를 저장하고 Export Audio Mixdown을 실행합니다.

원본 오디오는 외부 파일로 참조하므로 프로젝트를 다른 PC로 옮길 때 원본도 함께
옮겨야 합니다. 누락된 미디어는 다시 연결할 수 있습니다. 언어·단축키·오디오 장치·창
배치는 사용자 설정에 저장되고 프로젝트와 분리됩니다.

## 빌드 및 검사

[BUILDING.md](BUILDING.md)에 Windows MSVC 도구, ASIO SDK/libclang 준비,
빌드 명령, 대응 소스 이용법을 설명했습니다.

```powershell
npm ci
npm run check
npm run licenses:check
npm run tauri dev
# ASIO 포함 설치판: 경로는 자신의 개발 도구 위치로 지정
./scripts/package-windows.ps1 -SdkPath 'E:\Tools\ASIOSDK' -LibClangPath 'E:\Tools\LLVM\bin'
```

의존성은 `package-lock.json`과 `src-tauri/Cargo.lock`으로 고정합니다. 환경 변수는
빌드 프로세스에서만 사용하며 시스템 PATH를 수정하지 않습니다.

## 라이선스 및 소스

- MiniDAW: [GNU GPL version 3 only](LICENSE), 무보증
- [외부 의존성 전체 목록 및 고지](THIRD_PARTY_NOTICES.md)
- [대응 소스 제공 안내](SOURCE_CODE.md)
- [배포 범위·검증·남은 조건](docs/DISTRIBUTION.md)

외부 구성요소의 원래 라이선스를 유지합니다. ASIO SDK 공통 API에는 GPLv3 대안을,
Windows host helper에는 BSD 조건을 적용합니다. ASIO/VST는 Steinberg의 상표이며
이 프로젝트의 후원·인증을 의미하지 않습니다. 외부 플러그인의 사용·배포 권리는
각 플러그인 라이선스에 따릅니다.

설치 파일을 재배포할 때에는 같은 버전의 대응 소스 ZIP을 같은 다운로드 위치에서
함께 제공해야 합니다. 개발 도구, 사용자 음원, 프로젝트, 테스트 결과, 드라이버 및
외부 플러그인을 저장소나 Release에 일괄 복사하지 마세요.
