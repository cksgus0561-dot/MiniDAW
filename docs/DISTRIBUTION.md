# Windows distribution and license review

Release version: 0.1.0, Windows x64. Prepared locally; no GitHub upload is performed.

## GPLv3 scope

MiniDAW's own sources are GPL-3.0-only. Original permissive third-party notices
are preserved. `licenses/reviewed-dependencies.json` records every locked Rust
and npm package, version, declared license, deployment scope and notice hashes.
An SPDX declaration alone does not certify all files: native SDK inputs are
reviewed and hash-pinned separately. GPLv3 compatibility is exercised through
permissive OR alternatives, AND terms are retained, and MPL §3.3 applies to the
GPL larger work. No covered MPL source has been modified by this packaging task.

ASIO 2.3.4 has a GPLv3 alternative for files referencing its root license. Its
Windows host files have their own BSD-3-Clause terms. The release wrapper stages
only the eleven reviewed files; unrelated driver samples (`combase`, `dllentry`,
`register`, debug helpers) are neither compiled nor redistributed. Their unclear
old Microsoft sample notices are therefore outside this release.

VST3 3.8.1, CLAP 1.2.10, Signalsmith Stretch/Linear: MIT. Tauri/CPAL and the
other selected permissive dependencies are compatible with GPLv3. Symphonia,
selectors/cssparser and the other included MPL components retain exact source
availability in the matching source archive. The WebView2 loader has its own
Microsoft BSD-3-Clause notice. Native runtime/tool licenses are not inferred from
the Rust wrappers.

The earlier "RealFFI" finding is RealFFT 3.5.0, compiled through Rubato's
fft_resampler into MiniDAW's worker-side SRC. It is not a user-installed plugin.
The author's exact official README and Cargo manifest declaring MIT are now
included beside the standard terms and author attribution in
licenses/supplemental/rust/realfft-3.5.0. No separate full-text LICENSE/copyright
line exists in the reviewed upstream source; none is fabricated. See PROVENANCE.md
there for the original URLs and the precise documentary limitation. The explicit
MIT declaration supports the selected license; it is not a declaration of no
license. Only upstream can provide the missing standalone notice if the publisher
requires that additional confirmation. No SRC replacement was warranted or made.

Inactive macOS/Linux dependencies remain in the lock inventory but are excluded
from the Windows binary and Windows source-component collection. In particular,
the objc2 upstream itself flags Apple SDK-derived licensing uncertainty. No
claim of clearance for an Apple-platform release is made.

## What is and is not shipped

Installer: MiniDAW executable, GPL and third-party notices, source-access notice,
and the Tauri/NSIS installer machinery. It uses zlib compression. WebView2's
official bootstrapper is downloaded only when needed. VC runtime is statically
linked. System DLLs, Microsoft toolchains, ASIO SDK distribution, LLVM, Node/npm,
FFmpeg, test plugins, TOPPING drivers and user media are not installed by MiniDAW.

Corresponding-source archive: the exact project sources and build scripts,
vendored libraries, GPL/BSD ASIO host subset, and Windows dependency sources.
This source-only SDK subset is necessary for GPL correspondence; it is not an
SDK or developer-tool bundle installed on an end user's computer.

## Publication requirements and limits

- Upload the matching installer, corresponding-source ZIP and SHA256SUMS together
  on the same GitHub Release page. Keep source available with the binary (GPL §6(d)).
- Verify ownership/permission for the MiniDAW code being licensed and the actual
  eligibility of the publisher's Visual Studio license. Tool installation alone
  does not establish organizational licensing eligibility.
- No signing certificate has been supplied; builds are unsigned. Signing is a
  publisher decision, and SmartScreen behavior on another PC is not guaranteed.
- No proprietary external plugin or driver is redistributed. Whether a particular
  plugin and GPL host form a derivative combined work is context-dependent. This
  task does not grant a blanket proprietary-plugin linking exception or declare
  every third-party plugin combination legally cleared.
- A successful check on this development PC is not a clean-machine certification.
  WebView2-free installation needs the separate VM procedure in
  docs/CLEAN-WINDOWS-TEST.md. No usable Sandbox/VM tooling or clean guest was
  available here. The delivered installer requires internet when WebView2 is
  missing; offline first installation is not supported by downloadBootstrapper.

Official references: [GPLv3](https://www.gnu.org/licenses/gpl-3.0.html),
[MPL §3.3](https://www.mozilla.org/en-US/MPL/2.0/),
[ASIO SDK](https://www.steinberg.net/developers/),
[VST3 licensing](https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Licensing.html),
[CLAP license](https://github.com/free-audio/clap/blob/a47f6badb49d948fd009998f28309cdab78979c9/LICENSE),
[Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/),
[NSIS license](https://nsis.sourceforge.io/License),
[WebView2 Loader](https://www.nuget.org/packages/Microsoft.Web.WebView2/1.0.3800.47/License).

## Validation

Local validation results are recorded under `output/package-validation/` and
summarized here after the installer smoke tests. Logs and screenshots are excluded
from the public source tree.

- Frontend type check and Vite production build: passed.
- ASIO x64 release/NSIS build: passed. Static VC runtime; no MSVCP/VCRUNTIME DLL
  import, and no developer checkout/profile paths found in the application PE.
- Installed payload: MiniDAW, uninstaller and license documents only. Developer
  probes are gated behind `dev-tools`; no plugin/driver/test/SDK binary payload.
- Installed-app smoke: WAV/MP3/FLAC, internal Synth, WASAPI and TOPPING ASIO,
  actual Surge XT 1.3.4 VST3/CLAP instruments and effects, four native editors,
  Spectrum/Mixer, project save/restart/restore and all four Export modes passed.
- 48 kHz playback intervals: WASAPI average callback 0.095 ms; ASIO Audio/Synth
  0.018 ms; ASIO with four external plugin instances 0.184 ms. Overrun, device
  overrun, starvation and stream error deltas were all zero in those short checks.
- Selection/Track/Stems produced exactly 96,000 samples for the tested two-second
  interval; WAV PCM16/PCM24/float32 headers passed. Mixdown rendered at 7.99× real
  time, Selection 6.20×, Track 31.80× and the four-file Stem batch 26.18×.
- The app ran from its installed directory with System32-only PATH and no ASIO
  SDK/libclang environment. Original user preferences were restored after tests.
- License gate rejection tests passed for lock/feature/vendor changes, altered
  GPL/notice text, an unknown AND license term, helper lock and integration changes.
- New helper: real NSIS Unicode ABI, equal/newer/older version comparisons and
  string replacement passed. Final installer /S /R launched the installed app
  through the locally built RunAsUser. Installed payload: 1,017 files, including
  1,011 verified original notice hashes, with no excluded SDK/plugin/test payload.
- Final notice-only rebundle: actual install/uninstall passed; application bytes
  match the function-tested executable exactly. Same-version reinstall passed.
  Uninstall removes registration and executable, while preserving preferences.
- Production frontend, Rust audio/UI command code and vendored code: 306 files
  unchanged compared with the preceding source archive (non-shipped logo PDF
  excluded from comparison). Changes are confined to packaging/notices/tests.

## Installer provenance remediation

The upstream prebuilt Tauri helper has been replaced in the **final** installer
with a locally source-built nsis-tauri-utils 0.5.3. Its official source archive,
seven locked external crates, their license texts and the newly pinned Cargo.lock
are supplied. The upstream build.rs also embeds nsis-process, nsis-semvercompare
and nsis-string from that same source tree. All are covered by the original
Tauri MIT/Apache-2.0 licenses; MIT is selected. Original NSIS 3.11 components
(zlib stub, NSISdl, nsDialogs, System) remain separately identified and licensed.
See licenses/tools/INSTALLER-COMPONENTS.md.
The helper's run_as_user is a Rust port of Chromium elevation_util.cc. Its
additional BSD-3-Clause text and 2024 Chromium Authors copyright were missing
from the previous notice set and have now been added from the exact cited commit
as licenses/tools/Chromium-BSD.txt and Chromium-NOTICE.txt. Both the Tauri and
Chromium terms are retained; the helper is not treated as exclusively MIT.

scripts/rebundle-nsis.mjs changes only the helper name/path in Tauri's generated
installer inputs, then runs the verified NSIS compiler again. This avoids a
forked installer UI template. The final generated script cannot resolve the old
helper name. The source/build/notice gate covers the new lock and build scripts.
The old unattested-prebuilt-helper dependency is resolved; byte-identical builds
across arbitrary compiler versions are not promised. Current rerun results are
recorded in output/distribution/VALIDATION.md and output/package-validation/.
