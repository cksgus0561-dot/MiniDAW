param([Parameter(Mandatory=$true)][string]$Primary)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    if (-not (Test-Path -LiteralPath $Primary -PathType Leaf)) { throw 'Primary MP3 path does not exist.' }
    & node scripts/generate-fidelity-fixtures.mjs
    if ($LASTEXITCODE -ne 0) { throw 'Fixture generation failed.' }
    & cargo build --release --manifest-path src-tauri/Cargo.toml --bin fidelity_audit --bin fidelity_probe
    if ($LASTEXITCODE -ne 0) { throw 'Audit build failed.' }
    $audit = '.\src-tauri\target\release\fidelity_audit.exe'
    & $audit conversion docs/validation/conversion.json tests/generated/fidelity
    if ($LASTEXITCODE -ne 0) { throw 'Conversion audit failed.' }
    $sources = @('tests/fixtures/stereo-44100.wav','tests/fixtures/stereo-44100.mp3','tests/fixtures/stereo-44100.flac','tests/fixtures/untagged-vbr.mp3')
    $sources += @('embedded-info.mp3','understated-info.mp3','overstated-info.mp3','pcm16.wav','pcm24.wav','pcm32.wav','float32.wav','float64.wav','mono16.wav','with-seektable.flac','unknown-length.flac','large-id3.mp3') | ForEach-Object { "tests/generated/fidelity/$_" }
    & $audit compare docs/validation/compare.json @sources
    if ($LASTEXITCODE -ne 0) { throw 'Synthetic PCM audit failed.' }
    & $audit compare docs/validation/primary-pcm.json $Primary
    if ($LASTEXITCODE -ne 0) { throw 'Primary PCM audit failed.' }
    & $audit raw-mp3 docs/validation/primary-direct-decoder.json $Primary
    if ($LASTEXITCODE -ne 0) { throw 'Direct decoder comparison failed.' }
    & $audit raw-mp3 docs/validation/tagged-direct-decoder.json tests/fixtures/stereo-44100.mp3
    if ($LASTEXITCODE -ne 0) { throw 'Gapless decoder comparison failed.' }
    & $audit src docs/validation/src-after.json
    if ($LASTEXITCODE -ne 0) { throw 'SRC measurement failed.' }
    & node scripts/audit-src-reference.mjs
    if ($LASTEXITCODE -ne 0) { throw 'Independent SRC measurement failed.' }
    & node scripts/check-fidelity-reports.mjs
    if ($LASTEXITCODE -ne 0) { throw 'Exact PCM assertions failed.' }
    Write-Output 'Fidelity and independent reference audit completed. See SRC_UPGRADE.txt for measured quality and limitations.'
} finally { Pop-Location }
