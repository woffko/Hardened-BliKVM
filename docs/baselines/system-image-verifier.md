# Signed system-image verifier baseline

This checkpoint records the host-only verifier and fixture tooling at source
commit `5031501d9762c34ca698d7028262ac4f73eaa98c` with Rust 1.95.0.
No device partition, boot state, service, or recovery medium was changed.

## Host verification

The complete project gate passed, including:

- 10 Rust system-verifier tests for the canonical manifest, `sys-` trust
  domain, local layout/recovery/boot-control policy, exact component sizes,
  signatures, payload hashes, filesystem signatures, archive shape, unsafe
  source inodes, and bounds;
- all workspace tests and Clippy with warnings denied;
- 52 Python tests, including the public-only system fixture builder;
- BasedPyright, systemd/source boundaries, frontend build/lint/formatting, and
  semantic Rust diagnostics;
- `cargo audit --deny warnings --no-fetch` against 1,226 advisories and all 131
  locked packages.

## Independent AArch64 builds

Two separate Cargo target and output directories under the designated
temporary build root produced byte-identical prototype archives:

```text
hardened-blikvm-prototype-0.1.0-dev-aarch64.tar.gz
fb7936a538f6f5db97296a7aa6766c4a73da184d46be4bafe49aca1c6c874300
```

All six linked static-PIE prototype processes also remained identical between
the builds:

```text
bli-kvm-server                  2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-platform-broker         df88717e09047514acc6138ace9de1174717484cc4a46f9c52861308d7e79a5f
bli-kvm-video-proxy             c3c68da81ed5a8a373332a6c9dc0f60410719d444429548a0e837c0426e0e93d
bli-kvm-control-broker          f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111
bli-kvm-update-supervisor       e6dd0b2396fd230830cb87d1a5b14bde03436df4b3be12d01b612b3fc87edee4
bli-kvm-update-health-reporter  d4d89d3ec59bbd368980a2975ac337771962f4002478b178a1c6a3ea3d0c201d
```

The system verifier is currently a library and is not shipped in that archive.
Its single AArch64 executable code-generation object matched byte-for-byte:

```text
8c2a70288621bc7a705ae73401babbf017679bb30fa0584aa7e294c1c47dbfeb
```

The intermediate rlibs differed only through compiler metadata:

```text
rlib A     cb723969ba5851df4033a058d1c016bd008c405aef995b534a38d4c63d0a5e87
rlib B     61bbd9803257088314c28ab9cb9aff185ea80a4201f7caf8d4e13890d6f8822a
lib.rmeta A b67cd35d18f89815933da41a4bbd027e0c30a9e5e87cf71d4818037331306a85
lib.rmeta B 0808fb2e1092ee9d6de2e53fa4596db9bc03948dd48d9c2f3706173b94b51765
```

`lib.rmeta` is not executable and is not shipped. A future verifier or writer
executable must receive its own two-build linked-binary gate.

## Public-only fixture sample

The commit-bound fixture generator produced one rootfs-only and one split-boot
archive:

```text
system-rootfs-only.tar  fcbdb4388cf4439f11cb7034073a021e7f0bb975976f0eb0c9fb372819be0eb5
system-split-boot.tar   faf68acc5be03659971e7469d62ccb3c90fc59f91ef962962fcdfec4239adff9
```

The ephemeral private key was never serialized. The D: temporary mount reports
all files as mode 0777 despite requested 0700/0600 modes, so the receipt
correctly records `posixModesEnforced: false`. These public fixtures are not
trusted across sessions; consumers must regenerate them or verify exact hashes
from a trusted invocation. They are minimal signature test data, not bootable
filesystems or accepted H616 layouts.

## Boundary

This baseline proves deterministic verifier code generation, strict signed
format handling, and cross-compilation. It does not establish a complete H616
disk image, inactive-slot mapping, writer, bootloader support, boot selection,
rollback counter, recovery card, reboot behavior, or power-cut recovery.
