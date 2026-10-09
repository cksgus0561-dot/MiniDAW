# Installer component provenance

The installer is a separate program; its licenses are not inferred from Tauri's
application framework license. These notices accompany the installed MiniDAW.

| Component | Actual use | License / origin |
| --- | --- | --- |
| NSIS 3.11 makensis | Build tool, not installed | Official NSIS toolchain, mirrored by tauri-apps/binary-releases; NSIS-3.11-COPYING.txt |
| NSIS 3.11 zlib-x86-unicode | Installer/uninstaller stub | zlib/libpng; Copyright (C) 1999-2025 Contributors |
| NSISdl | Downloads Microsoft's WebView2 bootstrapper when missing | NSISdl-License.txt; Yaroslav Faybishenko and Justin Frankel |
| nsDialogs | Installer pages | General NSIS zlib/libpng license |
| System | Windows API calls in installer | General NSIS zlib/libpng; Copyright 2002 brainsucker (Nik Medved), 2002-2025 NSIS Contributors (Docs/System/System.html) |
| Tauri generated NSIS scripts | Installer logic | Tauri MIT / Apache-2.0, notices in components/npm/@tauri-apps__cli-*; exact preferred sources in the Tauri source archive |
| nsis-tauri-utils 0.5.3 | Version comparison, RunAsUser and string replacement | MIT OR Apache-2.0; MIT selected, nsis-tauri-utils-MIT.txt |
| Chromium elevation_util.cc Rust port | run_as_user in nsis-process | BSD-3-Clause; Chromium-BSD.txt and Chromium-NOTICE.txt, Copyright 2024 The Chromium Authors |

The helper is **not an official NSIS plugin**. MiniDAW builds it from the exact
tag archive in ../source-inputs.json, using packaging/nsis-helper/Cargo.lock.
It embeds nsis-process 0.4.4, nsis-semvercompare 0.3.0 and nsis-string 0.1.0 via
build.rs, plus nsis-plugin-api and the nsis-fn build macro, all from the same
MIT/Apache-2.0 licensed archive. The seven external crates and their exact
licenses are listed in nsis-helper/dependencies.json; their originals are in
the adjacent crate directories. Rust standard-library notices are also retained.
The Chromium-derived function retains its additional original BSD terms and
attribution; the workspace's MIT declaration does not replace those terms.

Tauri may download its upstream prebuilt helper while generating installer
inputs. scripts/rebundle-nsis.mjs then recompiles the generated NSIS scripts
with the locally built **minidaw_nsis_utils.dll**. The downloaded upstream DLL
is not used in the final installer. Build provenance and final hashes are
recorded locally under output/package-validation. The pinned source and lock
are distributed; no claim of cross-compiler bit-for-bit reproduction is made.

Only the zlib compression stub is selected. The full, unmodified NSIS COPYING
also documents bzip2/CPL-LZMA alternatives, but those stubs are not selected.
Plugin binaries, the compiler and the helper are hash-checked during packaging.
No SDK, musical plugin, TOPPING driver or developer compiler is installed.

Official references:
- https://nsis.sourceforge.io/License
- https://github.com/tauri-apps/binary-releases/releases/tag/nsis-3.11
- https://github.com/tauri-apps/nsis-tauri-utils/tree/nsis_tauri_utils-v0.5.3
- https://github.com/tauri-apps/tauri/tree/30da1fd6e17de6107ecc850c95dfb16b5729f2dd/crates/tauri-bundler/src/bundle/windows/nsis
