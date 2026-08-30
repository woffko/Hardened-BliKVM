# Recovery build inputs v1

`hardened-blikvm-recovery-inputs-v1` is the canonical, public build-input and
clearance document whose SHA-256 is bound by a future signed recovery manifest.
It does not itself grant a license, sign an image, or write media.

## Required top-level components

Exactly one of each kind is required:

- U-Boot;
- TF-A;
- Linux kernel/device trees;
- initramfs;
- rootfs package closure;
- Hardened-BliKVM source;
- toolchain/build environment;
- deterministic build recipe;
- accepted layout profile;
- offline SPDX validator/schema bundle.

Additional firmware inputs are permitted only as separately named records.
Every record is ordered by a unique lowercase identifier and binds version,
pinned source revision, source SHA-256, produced artifact SHA-256, SPDX license
expression, retained license-text SHA-256, corresponding-source availability,
provenance class, and a reviewed redistribution status.

`latest`, branch-only revisions, missing hashes, `NONE`, `NOASSERTION`, missing
corresponding source, or any status other than `cleared` fails the gate. The
validator checks evidence completeness and a human-reviewed decision; it does
not infer whether a license is legally sufficient.

## SPDX binding

The document pins SPDX 3.0.1 and these exact offline resources:

```text
https://spdx.org/rdf/3.0.1/spdx-context.jsonld
https://spdx.org/schema/3.0.1/spdx-json-schema.json
https://spdx.org/rdf/3.0.1/spdx-model.ttl
```

It binds the final SBOM SHA-256 and records successful structural and semantic
validation. A network fetch at release time cannot silently replace these
content-addressed build inputs.

## Boundary

`bli-kvm-recovery-inputs` validates canonical JSON, fixed product/target,
sequence, layout/build/source/notices hashes, SPDX binding, required component
coverage, ordering, and fail-closed clearance. It has no network, filesystem,
package-manager, license-scanner, signing, image, or device API.

The current project has no real document that passes this gate because boot,
rootfs, firmware, key-ceremony, and accepted-layout inputs are incomplete. Test
manifests are synthetic evidence for the validator only and must not be used to
claim recovery readiness.
