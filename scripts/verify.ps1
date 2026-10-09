param([switch]$Build, [switch]$Asio)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$missing = [System.Collections.Generic.List[string]]::new()
$featureArgs = @()
if ($Asio) {
    if (-not $env:CPAL_ASIO_DIR -or -not $env:LIBCLANG_PATH) { throw 'Use scripts/with-asio.ps1 with explicit SDK/libclang paths for -Asio.' }
    $featureArgs = @('--features','asio')
}

foreach ($tool in @('node', 'npm.cmd', 'cargo', 'rustc', 'rustup')) {
    $command = Get-Command $tool -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        Write-Output "[없음] $tool"
        $missing.Add($tool)
    } else {
        Write-Output "[확인] $tool : $($command.Source)"
        & $command.Source --version
        if ($LASTEXITCODE -ne 0) {
            $missing.Add($tool)
        }
    }
}

if ($missing.Count -gt 0) {
    Write-Output "필수 도구가 준비되지 않아 빌드를 시작하지 않았습니다: $($missing -join ', ')"
    Write-Output 'README.md의 개발 환경 안내를 확인해 주세요. 이 스크립트는 설치나 환경변수 변경을 하지 않습니다.'
    exit 1
}

if (-not $Build) {
    Write-Output '명령행 도구가 확인되었습니다. 전체 검증: .\scripts\verify.ps1 -Build'
    exit 0
}

Push-Location $projectRoot
try {
    if (-not (Test-Path -LiteralPath 'node_modules')) {
        throw '프로젝트 의존성이 없습니다. 프로젝트 폴더에서 npm install을 먼저 실행해 주세요.'
    }

    & npm.cmd run check
    if ($LASTEXITCODE -ne 0) { throw '프론트엔드 타입 검사에 실패했습니다.' }

    & npm.cmd run build
    if ($LASTEXITCODE -ne 0) { throw '프론트엔드 빌드에 실패했습니다.' }

    & cargo check --manifest-path src-tauri/Cargo.toml @featureArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust 컴파일 검사에 실패했습니다.' }

    & cargo test --manifest-path src-tauri/Cargo.toml @featureArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust 테스트에 실패했습니다.' }

    & cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Rust 코드 형식을 확인해 주세요.' }

    & cargo clippy --manifest-path src-tauri/Cargo.toml @featureArgs --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Rust 정적 분석에 실패했습니다.' }

    & npm.cmd run tauri -- build --no-bundle @featureArgs
    if ($LASTEXITCODE -ne 0) { throw 'Tauri 실행 파일 빌드에 실패했습니다.' }

    Write-Output '전체 빌드 검증을 통과했습니다. 실제 앱 검증: npm run test:ui -- src-tauri/target/release/minidaw.exe'
} finally {
    Pop-Location
}
