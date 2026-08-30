# Signed application verifier build baseline

This checkpoint records two independent AArch64 builds of source commit
`5603c2b35d7f7dddd45a5883c13f073339365f7e` with Rust 1.95.0. Both builds
used fresh, separate Cargo target and output directories under the designated
temporary build root. No device mutation was involved.

## Reproducible shipped payload

The two deterministic prototype archives were byte-for-byte identical:

```text
d84fc1a84bbcee7f0b654933558265dbc8a04f591cd50ca1c0174a4eb0cb4eb7
```

The independently linked static PIE payloads also matched exactly:

```text
bli-kvm-server           2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-platform-broker  df88717e09047514acc6138ace9de1174717484cc4a46f9c52861308d7e79a5f
bli-kvm-video-proxy      c3c68da81ed5a8a373332a6c9dc0f60410719d444429548a0e837c0426e0e93d
bli-kvm-control-broker   f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111
```

Both builds explicitly compiled `bli-kvm-update-verifier` and its Ed25519,
SHA-256, and tar dependencies for `aarch64-unknown-linux-musl`.

## Non-shipped compiler metadata

The verifier's single AArch64 code-generation object was byte-for-byte
identical in both builds:

```text
0213b89e1726fd8ab16f7c68811250080a1040f7a9939d1eb2971cbb292f72fd
```

The intermediate Rust `rlib` files were not byte-identical. Archive member
inspection isolated the difference to `lib.rmeta`; the sole executable object
member matched. An `rlib` is not shipped by this prototype. This does not count
as proof for a future update-service executable: that final linked executable
must receive the same two-build byte-for-byte gate once it exists.

## Scope

This baseline proves deterministic production payload generation and target
compilation of the verifier. It does not prove extraction, activation,
anti-rollback state, health confirmation, power-cut recovery, or full-system
image updates.
