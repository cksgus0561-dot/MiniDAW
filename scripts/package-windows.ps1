param(
  [Parameter(Mandatory=$true)][string]$SdkPath,
  [Parameter(Mandatory=$true)][string]$LibClangPath,
  [string]$WorkDir = 'output/package-build'
)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
Push-Location $root
$names=@('CPAL_ASIO_DIR','LIBCLANG_PATH','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CXXFLAGS')
$previous=@{}
foreach($n in $names){$previous[$n]=[Environment]::GetEnvironmentVariable($n,'Process')}
try {
  & node scripts/licenses.mjs check
  if($LASTEXITCODE -ne 0){throw 'License gate failed'}
  & node scripts/download-source-inputs.mjs
  if($LASTEXITCODE -ne 0){throw 'Source archive verification failed'}
  & node scripts/build-nsis-helper.mjs
  if($LASTEXITCODE -ne 0){throw 'Reviewed installer helper build failed'}
  $sdk=(Resolve-Path -LiteralPath $SdkPath).Path
  $clang=(Resolve-Path -LiteralPath $LibClangPath).Path
  if(!(Test-Path -LiteralPath (Join-Path $clang 'libclang.dll'))){throw 'libclang.dll missing'}
  $work=[IO.Path]::GetFullPath((Join-Path $root $WorkDir))
  New-Item -ItemType Directory -Force -Path $work | Out-Null
  $staged=Join-Path $work ('asio-'+[guid]::NewGuid().ToString('N'))
  & node scripts/prepare-asio-sdk.mjs $sdk $staged
  if($LASTEXITCODE -ne 0){throw 'ASIO license/source verification failed'}
  $env:CPAL_ASIO_DIR=$staged
  $env:LIBCLANG_PATH=$clang
  $cargoRegistry=if($env:CARGO_HOME){$env:CARGO_HOME}else{Join-Path $env:USERPROFILE '.cargo'}
  $flags=@('-C','target-feature=+crt-static',('--remap-path-prefix='+$root+'=/minidaw'),('--remap-path-prefix='+$cargoRegistry+'=/cargo'))
  $env:RUSTFLAGS=$null
  $env:CARGO_ENCODED_RUSTFLAGS=$flags -join [char]31
  & npm.cmd run tauri -- build --target x86_64-pc-windows-msvc --features asio --bundles nsis -- --locked
  if($LASTEXITCODE -ne 0){throw 'Windows package build failed'}
  & node scripts/rebundle-nsis.mjs
  if($LASTEXITCODE -ne 0){throw 'Final installer helper replacement failed'}
  & node scripts/check-release.mjs src-tauri/target/x86_64-pc-windows-msvc/release/minidaw.exe
  if($LASTEXITCODE -ne 0){throw 'Release artifact inspection failed'}
} finally {
  foreach($n in $names){[Environment]::SetEnvironmentVariable($n,$previous[$n],'Process')}
  Pop-Location
}
