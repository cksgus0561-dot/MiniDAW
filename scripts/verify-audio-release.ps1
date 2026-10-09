param([Parameter(Mandatory=$true)][string]$Primary)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
function Run-Node([string[]]$Arguments) {
    & node @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Release validation failed: $($Arguments[0])" }
}
try {
    foreach ($backend in @('asio','wasapi')) {
        foreach ($mode in @('off','on')) {
            Run-Node @('tests/ui-transport-benchmark.mjs','src-tauri/target/release/minidaw.exe',"docs/validation/transport-$backend-$mode.json",$backend,$mode)
        }
    }
    Run-Node @('tests/ui-smoke.mjs','src-tauri/target/release/minidaw.exe')
    Run-Node @('tests/ui-streaming.mjs','src-tauri/target/release/minidaw.exe')
    Copy-Item -LiteralPath 'docs/validation/streaming.json' -Destination 'docs/validation/streaming-wasapi.json'
    Run-Node @('tests/ui-primary.mjs',$Primary)
    Copy-Item -LiteralPath 'docs/validation/primary-after.json' -Destination 'docs/validation/primary-wasapi.json'
    Run-Node @('tests/ui-streaming.mjs','src-tauri/target/release/minidaw.exe','--asio')
    Copy-Item -LiteralPath 'docs/validation/streaming.json' -Destination 'docs/validation/streaming-asio.json'
    Run-Node @('tests/ui-primary.mjs',$Primary,'--asio')
    Copy-Item -LiteralPath 'docs/validation/primary-after.json' -Destination 'docs/validation/primary-asio.json'
    Run-Node @('tests/ui-audio-settings.mjs')
} finally { Pop-Location }
