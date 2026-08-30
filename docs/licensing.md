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
