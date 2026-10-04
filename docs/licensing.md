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

## Rust crates of the QUIC gateway

The QUIC transport (`docs/design/quic-transport-v1.md`) adds a QUIC and TLS
stack from crates.io, pinned in `Cargo.lock`: `wtransport` 0.7.2
(WebTransport over HTTP/3) on `quinn` 0.11.12, `rustls` 0.23.45 with `ring`
0.17.14, and `rcgen` 0.14.10 for the gateway's certificate. It is linked
into one binary, `bli-kvm-quic-gateway`; the web server uses it only in its
tests, as a client.

The lock gained 94 external packages with it; 68 of them are built for the
board, the others belong to other targets or to tests. Each declares its
licence: MIT OR Apache-2.0 in its spellings (61), Unicode-3.0 (18, the
Unicode tables of the URL parser), MIT (7), ISC (2), Apache-2.0 OR ISC OR
MIT (2), BSD-2-Clause (1), MIT OR Apache-2.0 OR Zlib (1), Zlib OR Apache-2.0
OR MIT (1), and Apache-2.0 AND ISC (1, `ring`). All are compatible with the
project's GPL-3.0-only. `tools/license_inventory.py --report` lists 313
packages for the whole workspace, 80 of them its own, and none without a
declared licence.

`ring` compiles C and assembly for the board. It is built by the project's
recipe for C code (`scripts/build-aarch64.sh`: clang against the pinned musl
headers, `rust-lld`), and its objects carry no path of the build machine.

## Hardware video encoder: sources and the clean room

The work on the H616 video engine's H.264 encoder
(`docs/baselines/work-plan-20261002-hardware-h264.md`) has no open driver to
start from. What it may use, and what it may not:

- Read and used: sources with an open licence and public documentation.
  They are listed with their licences and versions in
  `docs/design/h616-ve-h264-encoder.md`: jemk's `h264enc`
  (GPL-2.0-or-later) and its register header (LGPL-2.1-or-later), Bootlin's
  encoder for the V3s in its `cedrus` branch (GPL-2.0), mainline `cedrus`
  (GPL-2.0), and the vendor kernel's `cedar_ve.c` (GPL-2.0-or-later).
  Register offsets and bit meanings are facts. Where code is adapted, the
  file keeps the notice and the licence of its source.
- Watched, only with the operator's approval: the vendor's encoder library
  running on the test board as a black box. Only its traffic with the
  hardware is recorded, and the register description is written from that
  in this project's own words.
- Never: disassembling or decompiling a vendor library; reading vendor
  source that was not published under an open licence, its interface
  headers excepted as the description of how to call it; copying any of
  it; a vendor file in Git, a package, a slot or an image.

Kernel modules of this project (`kernel/`) are works of their own, licensed
GPL-2.0-only as the kernel requires, beside the GPL-3.0-only application,
which reaches them only through a device node. Their source and build
recipe are in this repository. The lab module and the lab tool of that plan
are never part of a package, a slot or an image.

One module is distributed: the encoder driver, `hardened-blikvm-ve-enc.ko`,
in the update-runtime package from `0.1.0~dev36`, and with that package in
the SD image and the RAW system payload. It is built from
`kernel/hardened-blikvm-ve/` (`ve_enc.c`, `ve_enc_plan.c`, `ve_hw.c` and
their headers) by `scripts/build-ve-module.sh`, against Linux 5.19.4 from
kernel.org with the board vendor's configuration and the headers the
board's kernel itself provides, with the compiler that kernel was built
with. The package carries the module's complete source beside it
(`/usr/share/doc/hardened-blikvm-update-runtime/ve-module-source/`) and
names the licence's text on the system (`/usr/share/common-licenses/GPL-2`).
The module contains no vendor code: its register plan was written from the
project's own description of the engine
(`docs/design/h616-ve-h264-encoder.md`), and the shared hardware part
follows mainline `cedrus` (GPL-2.0), which the file says. No slot carries
the module: it belongs to the system, not to the application.

## The quality monitor and RustAdmin's

The page's quality monitor
(`docs/design/h264-engine-choice-and-quality-monitor-v1.md`) follows the
quality monitor of RustAdmin (woffko/rustadmin, branch `experimental`), which
is RustDesk's code under AGPL-3.0: its toggle on the toolbar, its window over
the picture, its two levels and what its rows mean where this product has the
same thing. It was written anew in TypeScript for this page; no code, text
or asset of RustAdmin was taken into this repository, a package or a slot.
Anything taken from it later would be RustDesk's under AGPL-3.0: it would be
recorded here, with the files it went into and its notice kept.
