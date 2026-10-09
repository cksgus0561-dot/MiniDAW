# WebView2-free Windows acceptance test (not yet executed)

The development PC has Windows 11 Pro and WebView2. WindowsSandbox.exe, the
Hyper-V management command, VirtualBox and VMware Workstation were unavailable
when checked. No disposable VM/image was provided. A hypervisor-presence flag
alone does not establish access to a clean guest. Do not uninstall the developer
PC's runtime or hide its registry entries to simulate this test.

1. On an existing VM host, create a licensed Windows x64 guest. Use a supported
   Windows 10/11 image and a disposable snapshot with **no WebView2 Runtime**.
   Windows 11 images often include it; such an image is not a valid test baseline.
   If necessary, use the runtime's supported uninstall in the disposable guest,
   then reboot and confirm absence. Never delete runtime files/registry by hand.
2. Copy the exact release installer, SHA256SUMS.txt, a small WAV and
   scripts/test-clean-windows.ps1 into the guest. No Rust, Node, SDK, libclang,
   VC development tools, ASIO driver or external plugin is needed for this test.
   Verify the installer's SHA256. Keep the VM snapshot before enabling internet.
3. Run PowerShell as the ordinary VM user (not elevated):

   ```powershell
   ./test-clean-windows.ps1 -Installer ./MiniDAW_0.1.0_x64-setup.exe -PreflightOnly
   ./test-clean-windows.ps1 -Installer ./MiniDAW_0.1.0_x64-setup.exe -DisposableVM
   ```

   Keep normal internet access enabled. MiniDAW's Tauri downloadBootstrapper mode
   detects the missing runtime, downloads Microsoft's installer and invokes its
   silent install. The script checks real registry absence before starting, then
   runtime registration, MiniDAW payload, window creation and removal. A human
   must confirm rendered UI and a short WAV/Synth test before it can report pass.
   It preserves the JSON report and installer hash. Save a screenshot and Windows
   version alongside that report. Existing WebView2 yields `not-run`, never pass.
4. Restore the no-runtime snapshot. Disable guest networking and run the installer
   interactively. The expected result is an explicit WebView2 download/install
   error, **not successful first installation**. This distribution requires internet
   when the runtime is missing. Verify no misleading success/launch, then restore
   the snapshot. Offline first installation is not advertised or certified.
5. Test normal upgrade/reinstall with WebView2 already present, plus uninstall.
   Never remove a shared WebView2 runtime as part of MiniDAW uninstall.

The automated script is a reproducible procedure, not evidence that the missing-
runtime branch has run here. No unperformed clean-machine check is marked passed.
ASIO/plugin acceptance is a separate hardware test on the installed app.

Official detection/distribution specification:
https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution
