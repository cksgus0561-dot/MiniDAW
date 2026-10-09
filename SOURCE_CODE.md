# Corresponding Source / 대응 소스

MiniDAW is licensed under **GNU GPL version 3 only**. See `LICENSE`.
No warranty is provided. Third-party components retain the notices and licenses
listed in `THIRD_PARTY_NOTICES.md` and `licenses/`.

Every published Windows installer must be accompanied, on the **same download
page and without an additional fee**, by `MiniDAW-0.1.0-corresponding-source.zip`
and `SHA256SUMS.txt`. This is the source distribution method under GPLv3 §6(d),
not a promise to supply source later. Do not publish the installer alone.
The GitHub-generated repository archive does not replace this complete source
archive: external ASIO host sources and Rust/JavaScript component sources are
provided in the separate corresponding-source archive.

The source archive contains the exact MiniDAW sources and build scripts, the
vendored VST3/CLAP/Signalsmith code, the reviewed ASIO Windows host subset, and
sources for the Rust and frontend components used by the Windows build.
`BUILDING.md` explains extraction, prerequisites and compilation. Compilers,
standard system libraries, Windows SDKs, device drivers and third-party musical
plugins are not included. Building requires separately installed Windows
development tools and may require internet access for package resolution.

MPL-2.0 files (including Symphonia and the MPL UI dependencies) remain available
under MPL-2.0. In this GPLv3 larger work, those compatible MPL files are also
distributed under GPLv3 as permitted by MPL §3.3. Their original notices are
retained; no “Incompatible With Secondary Licenses” designation is applied.
The exact covered source is included, not just a link to the newest upstream.

ASIO common API files referencing the SDK license are used under its **GPLv3
alternative**. Windows host utility files retain their BSD-3-Clause licenses.
The Steinberg proprietary agreement is not the license selected for this build.
No TOPPING driver or external VST3/CLAP instrument/effect is distributed.

This local packaging workspace has not been published. When publishing, upload
the matching source asset alongside the installer and retain both for that release.

한국어: 설치 파일을 공개할 때 같은 다운로드 페이지에 위 대응 소스 ZIP과
SHA256SUMS.txt를 함께 제공해야 합니다. 저장소의 자동 생성 Source code ZIP만으로
대체하지 마세요. GPL/MPL 고지와 각 외부 코드의 원래 라이선스를 보존합니다.
