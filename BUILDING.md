# Building MiniDAW for Windows x64

Prerequisites (development only): Windows 10/11 x64, Rust stable MSVC, Node.js
22.12 or newer, npm, PowerShell 7, Visual Studio C++ build tools and Windows SDK.
The packaging verification used Rust 1.99.0, Node 24.19.0 and npm 11.17.0.
ASIO additionally requires ASIO SDK **2.3.4 (2025-10-15)** and libclang (tested
with LLVM 18.1.8). Keep these tools outside the Git repository. Do not copy any
developer runtime, SDK, driver or plugin into the installer.

```powershell
npm ci
npm run check
npm run licenses:check
rustup target add i686-pc-windows-msvc
```

Optional decoder/unit tests require the locally generated fixtures (not shipped):
install FFmpeg as a development tool, run `node scripts/generate-fixtures.mjs`,
then `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib`.

WASAPI development: `npm run tauri dev`. The release installer includes ASIO.
Developer audit/probe binaries require `--features dev-tools` (or
`--features asio,dev-tools`). This explicit gate keeps them out of installers.

```powershell
./scripts/package-windows.ps1 -SdkPath 'E:\Tools\ASIOSDK' -LibClangPath 'E:\Tools\LLVM\bin'
```

These example paths are user-supplied inputs. The script temporarily sets
CPAL_ASIO_DIR, LIBCLANG_PATH and compiler path-remapping flags only for its child
build. It does not change PATH, CARGO_HOME, RUSTUP_HOME or npm configuration.
The compiler uses a verified 11-file Windows ASIO host subset staged outside
production sources. It excludes the legacy Microsoft COM/driver sample files.
`licenses/asio-host-files.json` pins every SDK input by SHA-256; another SDK
revision requires an explicit file-level license review.

The Tauri CLI links the VC runtime statically. The installer obtains the official
WebView2 Evergreen bootstrapper if WebView2 is absent; an internet connection is
then required. No developer tools are needed on the end user's machine.

The packaging wrapper also builds the separate x86 NSIS helper from the pinned
official nsis-tauri-utils 0.5.3 source archive and
`packaging/nsis-helper/Cargo.lock`. The i686 Rust target is needed only on the
developer machine. The wrapper recompiles Tauri's generated NSIS script with
that helper; use `package:windows`, not a bare `tauri build`, for publication.
It does not replace the shared Tauri helper cache. Compiler version, helper hash
and final installer provenance are recorded under `output/package-validation/`.
Changing the helper source, lock or build script requires explicit license review.

## Corresponding source archive

Extract `MiniDAW-0.1.0-corresponding-source.zip`. `MiniDAW/` is the source root;
`dependencies/rust/` contains checksum-pinned Windows-build crate sources;
`dependencies/npm/` contains runtime/helper package sources, and `dependencies/asio/`
contains the exact selected SDK files. See `dependencies/README.md` for using
the bundled Rust sources. Other-target dependency metadata and development tools
can be fetched by Cargo/npm during a rebuild. The source archive is not a toolchain
or an offline Visual Studio installer. Use the bundled ASIO folder as `-SdkPath`.
`dependencies/nsis-helper-crates/` contains the seven exact helper dependency
sources; the helper's preferred source is the pinned tarball in dependencies/npm.

## License gate and updates

`npm run licenses:check` checks the lockfiles, GPL-compatible reviewed license
expressions, every vendored source hash, and original license/notice hashes. It
fails when a dependency version, SDK input or evidence changes. It is run by the
packaging script before compiling. This is a license gate, not a security audit.

For dependency changes, first run `cargo vendor --locked --versioned-dirs` with
`--manifest-path src-tauri/Cargo.toml` into a temporary directory. Set the
process-local `MINIDAW_RUST_SOURCES` to that directory and run
`npm run licenses:update`. This only generates a candidate; it **does not approve**
new dependencies. Review original licenses, AND obligations, MPL secondary-license
eligibility, and files missing notices. Then update the reviewed manifest and
notice files together. Review SDKs/build-tool stubs separately.

The scripts never publish or upload artifacts. Build logs and validation results
are local files and must not be committed as public project data.

## Prepare GitHub Release assets

After the installer and device checks pass, fetch the hash-pinned source archives
with `node scripts/download-source-inputs.mjs`. Run `cargo vendor --locked
--versioned-dirs --manifest-path src-tauri/Cargo.toml <temporary-rust-source-folder>`.
Use the reviewed ASIO subset folder created by the packaging build (shown under
`output/package-build/asio-*`). Then run:

```powershell
node scripts/package-source.mjs '<temporary-rust-source-folder>' '<reviewed-asio-subset>' output/source-inputs src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/MiniDAW_0.1.0_x64-setup.exe
```

The output directory is `output/distribution/`. The clean GitHub source ZIP is
an allowlisted repository seed; the larger corresponding-source ZIP additionally
contains third-party Windows source dependencies and SDK interface files.
Neither archive includes personal projects or local validation outputs.

For a runtime-free Windows VM acceptance procedure, see
`docs/CLEAN-WINDOWS-TEST.md` and `scripts/test-clean-windows.ps1`. The development
PC's successful installation is not evidence of the missing-WebView2 branch.
