param(
    [Parameter(Mandatory=$true)][string]$SdkPath,
    [Parameter(Mandatory=$true)][string]$LibClangPath,
    [Parameter(Mandatory=$true)][string]$Program,
    [Parameter(ValueFromRemainingArguments=$true)][string[]]$CommandArgs
)
# No installation, PATH edits, or persistent environment changes. Caller supplies
# tools stored outside the repository. Also prevent asio-sys' implicit SDK download.
$ErrorActionPreference = 'Stop'
$sdk = (Resolve-Path -LiteralPath $SdkPath).Path
$clang = (Resolve-Path -LiteralPath $LibClangPath).Path
if (-not (Test-Path -LiteralPath (Join-Path $sdk 'common/asio.h'))) { throw 'SdkPath must contain common/asio.h' }
if (-not (Test-Path -LiteralPath (Join-Path $clang 'libclang.dll'))) { throw 'LibClangPath must contain libclang.dll' }
$oldSdk = $env:CPAL_ASIO_DIR
$oldClang = $env:LIBCLANG_PATH
try {
    $env:CPAL_ASIO_DIR = $sdk
    $env:LIBCLANG_PATH = $clang
    & $Program @CommandArgs
    if ($LASTEXITCODE -ne 0) { throw "ASIO command failed: $LASTEXITCODE" }
} finally {
    $env:CPAL_ASIO_DIR = $oldSdk
    $env:LIBCLANG_PATH = $oldClang
}
