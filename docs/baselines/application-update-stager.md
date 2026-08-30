# Immutable application stager build baseline

This checkpoint records two independent AArch64 builds of source commit
`0bc00877a4c29d9074d10ea657c4a781ed895348` with separate Cargo target and
output directories under the designated temporary build root.

Both builds explicitly compiled `bli-kvm-update-stager`,
`bli-kvm-update-verifier`, and the complete workspace for
`aarch64-unknown-linux-musl`. Their prototype archives were byte-for-byte
identical:

```text
0314e86d6a04b50f291fe8d72e139b56b6f67d13849ed6ea4ed55ca929863b88
```

The independently linked production payloads also remained identical:

```text
bli-kvm-server           2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-platform-broker  df88717e09047514acc6138ace9de1174717484cc4a46f9c52861308d7e79a5f
bli-kvm-video-proxy      c3c68da81ed5a8a373332a6c9dc0f60410719d444429548a0e837c0426e0e93d
bli-kvm-control-broker   f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111
```

The stager's sole code-generation object was byte-for-byte identical in both
builds:

```text
e9f22817ba1f7f6a4c64f402c1c812f2dc048f4c92100324832770fb6825bb85
```

As in the verifier checkpoint, the intermediate non-shipped Rust `rlib`
varied only in `lib.rmeta`; its executable object member matched. A future
linked update-worker executable must pass its own byte-for-byte two-build gate.

This baseline proves target compilation and deterministic shipped payload
generation. It does not prove active-slot switching, health confirmation,
watchdog rollback, interruption recovery, or H616 runtime behavior.
