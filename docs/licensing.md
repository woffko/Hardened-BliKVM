# Licensing and branding gate

BLi-KVM is currently declared `GPL-3.0-only`, matching the Hardened NanoKVM
application baseline and remaining compatible with GPL-3.0-or-later BliKVM
sources used in a combined derivative work.

The planned public repository identity is `woffko/Hardened-BliKVM`. The Rust
crate prefix and internal API namespace remain `bli-kvm`; changing the GitHub
slug does not resolve product-name or trademark questions.

Before redistribution:

1. retain original copyright and license notices;
2. mark modified files and publish corresponding source;
3. inventory submodules, Rust crates, npm packages, firmware, kernel patches,
   native libraries, media components, and board support;
4. generate an SPDX-compatible SBOM and license report;
5. block release for non-redistributable or `NOASSERTION` components unless a
   documented grant exists;
6. provide the exact build and installation information required by the
   applicable licenses.

## Pinned BliKVM source findings

The pinned `blikvm/blikvm` and `woffko/Bli-PiKVM` repositories both carry a
GPLv3 license file. The vendor checkout also contains two pinned web submodules
whose licenses must be inspected separately before any code is imported.

The vendor tree bundles material that is not cleared merely by the top-level
license:

- ArmbianIO source declares GPL-3.0-or-later;
- the bundled SPI LCD directory carries GPLv3;
- the bundled Waveshare SSD1306 source has provenance text but no license file
  found in that subtree;
- prebuilt H616 uStreamer, test, PHP phar, and multi-architecture Ventoy
  artifacts are present;
- the Ventoy bundle and every bundled component need their own notices,
  corresponding source assessment, and redistribution review.

No prebuilt vendor binary will be copied into a Hardened-BliKVM artifact by
default. Missing or ambiguous license/provenance is a release blocker, not an
invitation to infer rights from the repository-level GPL file. Rust ports will
be based on documented behavior and clearly licensed source, with file-level
attribution retained where code is adapted.

Source-code rights do not automatically grant trademark rights. The proposed
wordmark uses the vendor name and remains a publication blocker until written
permission or a separate trademark review is available. Development uses the
distinct `BLi-KVM` product identity.

## Recovery-image gate

A recovery release must include the complete boot chain, firmware, kernel,
rootfs, native/Rust/npm/web dependencies, build tools, and their relationships
in an SPDX 3.0.1 SBOM. Each shipped binary or firmware item also needs an exact
source/provenance record, retained notices/license text, and an explicit
redistribution/corresponding-source decision. `NOASSERTION` or missing source is
release-blocking. The current field-image and bundled vendor/Ventoy binaries do
not satisfy this gate.

The pure recovery-input validator enforces evidence completeness and an
explicit reviewed clearance state. It does not infer legal sufficiency from a
repository license or automatically approve an SPDX expression.

## Native libraries of the video encoder

`bli-kvm-video-encoder` links three pinned static C libraries, each built from
a release archive checked by SHA-256 (`scripts/build-*.sh`), whose license
texts are installed with the encoder under
`/usr/share/doc/hardened-blikvm-video-encoder/`:

- libvpx 1.15.2 (VP8): BSD-3-Clause with the WebM patent grant;
- x264 build 165 (H.264): GPL-2.0-or-later;
- libjpeg-turbo 3.2.0 (decoding the capture's JPEG frames): the IJG License,
  BSD-3-Clause and zlib. As the IJG License asks of binary distributions:
  this software is based in part on the work of the Independent JPEG Group.

All three are compatible with the project's GPL-3.0-only.
