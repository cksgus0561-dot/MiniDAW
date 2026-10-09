param(
  [Parameter(Mandatory=$true)][string]$Installer,
  [switch]$DisposableVM,
  [switch]$PreflightOnly,
  [string]$Report = 'clean-windows-result.json'
)
# Run ONLY in a disposable, licensed Windows x64 VM. Never removes WebView2.
# A pre-existing runtime makes this test NOT RUN, rather than a false pass.
$ErrorActionPreference='Stop'
$guid='{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
function RuntimeVersions {
  @("HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$guid",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$guid",
    "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$guid") | ForEach-Object {
      $v=(Get-ItemProperty -LiteralPath $_ -Name pv -ErrorAction SilentlyContinue).pv
      if($v -and $v -ne '0.0.0.0'){[pscustomobject]@{Key=$_;Version=$v}}
    }
}
$result=[ordered]@{status='not-run';preflightOnly=[bool]$PreflightOnly;runtimeBefore=@(RuntimeVersions);runtimeAfter=@();installerSha256=$null;installation=$false;window=$false;uninstallation=$false}
$app=$null
try {
  $setup=(Resolve-Path -LiteralPath $Installer).Path
  $result.installerSha256=(Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
  if($result.runtimeBefore.Count){throw 'NOT RUN: WebView2 is already installed. Restore a genuinely runtime-free VM snapshot.'}
  if($env:WEBVIEW2_BROWSER_EXECUTABLE_FOLDER){throw 'NOT RUN: fixed/browser override must be absent in the VM.'}
  if($PreflightOnly){$result.status='preflight-ready';return}
  if(!$DisposableVM){throw 'NOT RUN: explicitly select -DisposableVM inside a disposable VM.'}
  $dir=Join-Path $env:LOCALAPPDATA 'Programs\MiniDAW'
  if(Test-Path -LiteralPath $dir){throw 'NOT RUN: MiniDAW installation already exists.'}
  $result.status='failed'
  $p=Start-Process -FilePath $setup -ArgumentList '/S' -WindowStyle Hidden -PassThru
  if(!$p.WaitForExit(600000)){throw 'Installer timed out; inspect the VM and preserve logs.'}
  if($p.ExitCode -ne 0){throw "Installer exit code $($p.ExitCode)"}
  $result.runtimeAfter=@(RuntimeVersions)
  if(!$result.runtimeAfter.Count){throw 'Installer returned without installing WebView2.'}
  $exe=Join-Path $dir 'minidaw.exe'
  if(!(Test-Path -LiteralPath $exe)){throw 'MiniDAW executable missing'}
  foreach($name in @('LICENSE','THIRD_PARTY_NOTICES.md','SOURCE_CODE.md')){if(!(Test-Path -LiteralPath (Join-Path $dir $name))){throw "Missing $name"}}
  $result.installation=$true
  # Visible by design: the operator must inspect the actual app in the VM.
  $app=Start-Process -FilePath $exe -PassThru
  for($i=0;$i -lt 60;$i++){Start-Sleep -Milliseconds 500;$app.Refresh();if($app.HasExited){throw 'MiniDAW exited during startup'};if($app.MainWindowHandle -ne 0){$result.window=$true;break}}
  if(!$result.window){throw 'Main window not observed'}
  Write-Host 'Confirm rendered Arrangement and buttons, Play/Pause with a small WAV, and a built-in Synth note.'
  $answer=Read-Host 'Type VERIFIED only after inspecting the app; otherwise type the failure'
  if($answer -cne 'VERIFIED'){throw "Manual rendering/audio verification not passed: $answer"}
  $null=$app.CloseMainWindow()
  if(!$app.WaitForExit(15000)){throw 'App did not close; inspect the VM before uninstalling'}
  $p=Start-Process -FilePath (Join-Path $dir 'uninstall.exe') -ArgumentList '/S' -WindowStyle Hidden -PassThru
  $null=$p.WaitForExit(60000)
  # NSIS may relaunch its uninstaller from a temporary path. Wait for both the
  # payload and final registry cleanup, not just the short-lived launcher.
  for($i=0;$i -lt 60;$i++){
    if(!(Test-Path -LiteralPath $exe) -and !(Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MiniDAW')){break}
    Start-Sleep -Milliseconds 500
  }
  if(Test-Path -LiteralPath $exe){throw 'Uninstall left MiniDAW executable'}
  if(Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MiniDAW'){throw 'Uninstall registration still present'}
  $result.uninstallation=$true;$result.status='passed'
} catch {$result.error=$_.Exception.Message;if($result.installation){$result.status='failed'};Write-Warning $result.error}
finally {$result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Report -Encoding UTF8;Write-Output ($result | ConvertTo-Json -Depth 6)}
if($result.status -ne 'passed' -and $result.status -ne 'preflight-ready'){exit 2}
