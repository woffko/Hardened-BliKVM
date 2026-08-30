# System recovery core baseline

This checkpoint records the recovery geometry and pure boot-control models at
source commit `4372df93c3fc6cc6d77adbf5bdc7248586751ef4`. Neither crate
contains persistence, device I/O, partitioning, boot selection, or reboot code.

## Host gates

- seven layout tests cover valid recovery MBR/GPT examples, exact live-layout
  rejection, bounds/alignment/ordering/overlap, role and A/B pairing, partition
  limits, raw sunxi ranges, boot-control policy, and canonical documents;
- nine boot-control tests cover canonical checksummed copies, torn/degraded
  selection, adjacent generation choice, split brain/generation gaps, phase
  material, attempt decrement, and all specified reconciliation branches;
- the complete workspace test/Clippy, 52-test Python, BasedPyright,
  systemd/source, frontend, formatting, and semantic-diagnostic gates passed;
- `cargo audit --deny warnings --no-fetch` passed all 133 locked packages
  against 1,226 local advisories.

## Independent AArch64 builds

Two fresh, separate Cargo target/output directories produced byte-identical
prototype archives:

```text
hardened-blikvm-prototype-0.1.0-dev-aarch64.tar.gz
89e9fa2f43605896cbbaad1be415a37d9ace1180b68a604fed884e378653ab2c
```

All six linked static-PIE prototype processes remained byte-identical. The two
new libraries are not shipped in that archive. Their AArch64 executable
code-generation objects matched exactly:

```text
bli-kvm-system-layout        656e1887dd4c24cd8240b5b88c125f3c36363af3fe69d8d1176cd8e3e7c26f53
bli-kvm-system-boot-control  19c7466ebf253e1dc1f9d837f9edce5eb42617593420a2df5dfb3f9505c4e62c
```

As with the earlier verifier libraries, only non-shipped compiler metadata
varied:

```text
layout rlib A  ec8bacc56f8f410f3cebe43a7c79530222711fe5e12096e9d9004fab32c6c1d4
layout rlib B  8c28f624fdd5237b86dfe9e9dacb7f23263123f9792613e7af305285c3b480c0
layout rmeta A aff42efc782811ea93f9da208d3b3b1a3ecbca2e3e8d6e66066530bc32b1694e
layout rmeta B 417570738f3abfaa0bbf9471426f16a1588c4f9d6260120166e2933f7240b322

boot rlib A    38cba39a5b7bd05ba7bbda03d92d68d50bc40b24710d22c7d5eb1336f949b71f
boot rlib B    66647751a13b5c7b4145486c40a2fe6c387fc2e141686d5a70a8617d0e76e9d7
boot rmeta A   67fbb18f45fcb6e34399e4539a3cc691c2203ab8052554aaad5ba0eb44f6f1f4
boot rmeta B   9ff645e86db4b4d3ad568ee5fd4673d64cc11c66d3f2e4ccf514b2df8fb101d7
```

Future storage or selector executables must receive independent linked-binary
reproducibility gates.

## Boundary

This proves deterministic machine code for validation and reconciliation only.
It does not accept a concrete layout hash, recovery image, U-Boot build,
journal storage adapter, selector adapter, block writer, application/system
floor coupling, recovery card, reboot, or power-cut behavior.
