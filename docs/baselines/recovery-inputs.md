# Recovery input clearance baseline

This checkpoint records `bli-kvm-recovery-inputs` at source commit
`4099751b02902556e0edca926b8a8d5f5a651de7`. It validates evidence
documents only; no recovery image, key, or device operation was created.

## Verified implementation

- nine tests cover complete canonical input, all ten required kinds, optional
  firmware naming, missing/renamed/unsorted components, blocked/no-assertion
  clearance, missing source, invalid binary provenance, branch-like revisions,
  zero hashes, invalid license evidence, SPDX 3.0.1 drift, cross-reference
  mismatch, unknown fields, non-canonical JSON, and size bounds;
- the complete workspace, Clippy, Python, BasedPyright, systemd/source,
  frontend, formatting, and semantic-diagnostic gates passed;
- RustSec audit passed all 134 locked packages against 1,226 local advisories.

Two independent AArch64 builds produced byte-identical prototype archives:

```text
1683416c41e1ed0f5e39acb27ca774244cb504c13741409f8979acc3dbbe1bc8
```

The recovery-input validator code-generation object matched exactly:

```text
ffde82e2080252c96c11135ff8af9bae038082fe8863f6afe2cedec7f955d69a
```

Only non-shipped compiler metadata varied:

```text
rlib A   e85f64dbd9be9cc06b662a477b3e88fcf3cf2f8098f8a13831ca3457ba9e0e67
rlib B   cfcbacbe5ceb5dcb348253c3f03163a3ea9b8bccc0fba2140cb1f4cfca67d7cc
rmeta A  8ffec1acc1c3683e7b8ecfb7678149e9a4882674878abebbcc726b8f80227758
rmeta B  57a46ad7a5d97404d0c83a390dcb061e2739af4cf1ddb9037c24b28be4be6b17
```

## Real input blockers

No non-test manifest can currently pass. Missing reviewed inputs are:

- an exact redistribution-cleared H616 U-Boot build, TF-A input, defconfig,
  patches, firmware dependencies, and corresponding source;
- the exact kernel/device-tree source, config, patches, and firmware provenance
  for the target recovery environment;
- deterministic initramfs and rootfs package repositories, locks, signatures,
  and complete corresponding-source/notice set;
- an accepted canonical recovery layout hash and selector implementation;
- a recovery/application/system key ceremony and public trust bundle;
- content-addressed SPDX 3.0.1 context/schema/model files and a pinned offline
  structural/semantic validator;
- a deterministic full-image toolchain/build recipe and cleared boot/rootfs
  source closure;
- trademark and every bundled firmware/binary redistribution decision;
- two disposable recovery cards and the physical boot/interruption/power-cut
  matrix.

The truncated field image, vendor prebuilt binaries, and synthetic unit-test
manifest are explicitly disallowed substitutes.

## Boundary

This baseline proves deterministic validation code and fail-closed evidence
requirements. It does not prove SPDX/legal correctness, real input clearance,
image reproducibility, bootability, recovery authority, or physical safety.
