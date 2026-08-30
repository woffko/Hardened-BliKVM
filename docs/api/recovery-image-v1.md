# Signed recovery image v1

Recovery media is a third artifact and trust domain. It is neither an
application bundle nor an inactive-slot system update. Recovery key IDs begin
with `recovery-`; `app-` and `sys-` keys are rejected.

No recovery artifact is currently implemented or accepted. The user-supplied
field image is truncated and forbidden as an input.

## Scope

`hardened-blikvm-recovery-v1` is a complete removable-media image used to
provision or repair a recovery-first H616 layout while booted independently of
the target eMMC. It may contain an accepted SPL/U-Boot/TF-A chain, partition
table, read-only recovery root, layout profiles, public verification keys, and
offline repair tools.

It must not contain:

- a default password, host key, cookie, token, device credential, history, or
  private signing key;
- state, logs, user configuration, MSD data, or files copied from a live field
  image;
- an unsigned hook, network installer, request-derived command, or automatic
  live-device target;
- a component with unresolved source, copyright, license, trademark, export,
  or binary-redistribution status.

## Signed artifact

The first format is an uncompressed deterministic tar. Compression may be
added only in a later version with separately bounded streaming semantics.
Entries are fixed:

```text
manifest.json
manifest.sig
metadata/layout.json
metadata/licenses.json
metadata/sbom.spdx.json
metadata/sources.json
payload/recovery-card.img
```

The canonical manifest binds:

- format, product, target, `recovery-` key ID, version, and monotonic recovery
  sequence;
- exact source commit and reproducible build-recipe SHA-256;
- canonical accepted layout ID and profile SHA-256;
- complete raw image byte length and SHA-256;
- source, license, and SBOM document SHA-256 values;
- pinned bootloader, TF-A, kernel, initramfs, rootfs, and toolchain identities;
- the system-image and boot-control schema sequences understood by recovery.

The image length must cover every partition and backup table declared by the
layout. Extra or missing bytes, sparse-hole dependence, truncated partitions,
and an image whose parsed geometry differs from `layout.json` are rejected.

## Required build inputs

Every input is content-addressed and provenance-bound:

1. exact U-Boot and TF-A source revisions, defconfig, patches, firmware inputs,
   and corresponding-source decision;
2. exact Linux kernel source/config/patches and device trees;
3. deterministic initramfs and rootfs package locks, repositories, and package
   signatures;
4. exact Hardened-BliKVM source, Rust/npm locks, toolchain versions, and build
   scripts;
5. accepted canonical recovery layout and boot-control policy;
6. public application, system, and recovery trust roots from a reviewed key
   ceremony;
7. license texts, notices, source-offer/material, and an explicit cleared or
   blocked redistribution decision for every binary/firmware component;
8. a pinned offline SBOM validator and schemas.

`NOASSERTION`, missing license text, unavailable corresponding source, unknown
prebuilt firmware provenance, or an uncleared trademark/binary grant is a hard
release failure rather than a warning.

## SBOM contract

The artifact pins the official
[SPDX Specification 3.0.1](https://spdx.github.io/spdx-spec/v3.0.1/).
`metadata/sbom.spdx.json` uses the versioned SPDX 3.0.1 JSON-LD context and must
pass both the official JSON Schema structural validation and offline semantic
ontology/shape validation. The serialized bytes are canonical JSON with unique
lexicographically ordered keys and no insignificant whitespace before hashing
and signing.

The SBOM covers the disk image, boot chain, firmware, kernel, rootfs packages,
native libraries, Rust crates, npm packages, web assets, build tools, and source
relationships. A Rust-only or application-only SBOM is insufficient.

## Verification and build gates

- two clean builds in isolated roots produce byte-identical raw images,
  manifests, metadata, and final archives;
- the raw MBR/GPT, reserved sunxi ranges, partitions, filesystem signatures,
  and complete image length match the accepted layout;
- bootloader and rootfs inputs match their source/build provenance;
- secret, private-key, default-account, license, and source-availability scans
  pass;
- the image boots read-only on two independently written recovery cards and
  exposes no automatic target write;
- explicit selection, full readback, interruption, wrong-device refusal,
  repeated boot, and power-cut recovery pass on disposable media before any
  live-device conversion.

## Authority boundary

Recovery media may offer a local console workflow after identifying an exact
accepted device/layout. It may never select a target merely because it is the
first MMC disk. Every destructive operation requires an exact-device proof,
displayed plan, explicit confirmation, recoverable backup, write/readback
receipt, and a separately accepted physical test procedure.
