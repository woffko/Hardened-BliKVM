# Application activation manager build baseline

This checkpoint records two independent AArch64 builds of source commit
`fdaf41833778873f19aac486a8610c63e3e33a47` in separate Cargo target and
output directories under the designated temporary build root.

Both builds explicitly compiled `bli-kvm-update-manager` and the complete
workspace for `aarch64-unknown-linux-musl`. Their prototype archives were
byte-for-byte identical:

```text
3c197b712be1fdb63b896d79b0f48c43f3d8c2a2f5166dbe97330589fb34b0ea
```

The independently linked production payloads remained identical:

```text
bli-kvm-server           2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-platform-broker  df88717e09047514acc6138ace9de1174717484cc4a46f9c52861308d7e79a5f
bli-kvm-video-proxy      c3c68da81ed5a8a373332a6c9dc0f60410719d444429548a0e837c0426e0e93d
bli-kvm-control-broker   f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111
```

The manager's sole AArch64 code-generation object was byte-for-byte identical
in both builds:

```text
e5650fd96dec31715529dbd374a3bad803564e41d91ef66869bf94da66505463
```

The intermediate non-shipped Rust `rlib` differed only in `lib.rmeta`; its
executable object member matched. The future linked supervisor executable must
pass a fresh byte-for-byte two-build gate.

This baseline proves target compilation and deterministic shipped payload
generation. It does not prove systemd supervision, live watchdog timing,
process health, power-cut behavior on H616 storage, or device rollback.
