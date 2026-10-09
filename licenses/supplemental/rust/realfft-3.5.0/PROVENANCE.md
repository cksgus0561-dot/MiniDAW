# RealFFT 3.5.0 (not RealFFI)

No separate dependency named RealFFI exists in the reviewed Cargo/npm/native
inputs. The earlier missing-license-file finding refers to **RealFFT 3.5.0**.

MiniDAW src-tauri/src/audio/resample.rs uses Rubato 5.0.1's Fft resampler.
Rubato's fft_resampler feature uses RealFFT 3.5.0, which wraps RustFFT 6.4.1.
This is compiled DSP code in MiniDAW's Windows application, not a user-installed
VST3/CLAP plugin. Source is included in dependencies/rust/realfft-3.5.0 in the
corresponding-source archive. No audio implementation was replaced for this review.

The author's official source at commit
d0d4eee0525fd27c96c8a046d6d107acd5ed84a6 explicitly says `License: MIT` in
README.md and `license = "MIT"` in Cargo.toml. Unmodified originals are preserved
here as UPSTREAM-README.md and UPSTREAM-Cargo.toml and hash-pinned by the release
gate. These are upstream's permission declaration, not a license inferred from
the unrelated RustFFT dependency.

MIT-NOTICE.txt preserves the author's manifest attribution and the standard
MIT terms (https://opensource.org/license/mit). It is a distributor notice,
**not** a claim that upstream supplied a standalone LICENSE or copyright year.
The official 3.5.0 source and current repository have no such standalone file.
There is an explicit MIT declaration; there is no evidence that this code is
unlicensed or that a separate commercial agreement is required. The remaining
documentary limitation is the absent upstream full-text/copyright notice. Only
the rights holder can supply that additional confirmation; it has not been
invented or represented as obtained. A publisher requiring that exact document
should request it from HEnquist before publishing. Replacing the SRC algorithm
solely because a LICENSE filename is absent is not justified by the evidence.

Official originals:
- https://raw.githubusercontent.com/HEnquist/realfft/d0d4eee0525fd27c96c8a046d6d107acd5ed84a6/README.md
- https://raw.githubusercontent.com/HEnquist/realfft/d0d4eee0525fd27c96c8a046d6d107acd5ed84a6/Cargo.toml
