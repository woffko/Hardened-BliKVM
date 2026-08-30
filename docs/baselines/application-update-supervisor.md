# Application update supervisor build baseline

This checkpoint records two independent AArch64 builds of source commit
`1a885428393f6060edfd98020686a11272fed064` in separate Cargo target and
output directories under the designated temporary build root.

Both builds produced a byte-for-byte identical static PIE supervisor:

```text
bli-kvm-update-supervisor  139cd2c5439a9d7c8966738ea894f5bd56067b976fc7108536f5a632ec32534b
```

The complete prototype archives, including the supervisor and its systemd
service, timer, socket, and sysusers templates, were also byte-for-byte
identical:

```text
c08f82a22fea9670988ed7b93becb5e10a28e95e4b0a0e9ba247ab448fde751d
```

The existing independently linked payloads remained unchanged:

```text
bli-kvm-server           2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-platform-broker  df88717e09047514acc6138ace9de1174717484cc4a46f9c52861308d7e79a5f
bli-kvm-video-proxy      c3c68da81ed5a8a373332a6c9dc0f60410719d444429548a0e837c0426e0e93d
bli-kvm-control-broker   f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111
```

The first AArch64 link attempt at source commit `c2d5da3` exposed that the
self-contained musl C library did not export a `renameat2` wrapper. Both the
stager no-replace publication and manager pointer replacement now invoke the
kernel's `SYS_renameat2` through `libc::syscall`. Host transition tests and the
two clean AArch64 builds passed after this change. No fallback to overwrite or
non-atomic rename was introduced.

This final checkpoint also includes the local signed `stage` command. It can
publish a verified immutable slot but cannot change the active pointer. The
two-build comparison covers that exact linked command path.

This baseline proves target linking, static-PIE hardening, and deterministic
artifact generation. It does not prove runtime syscall availability, systemd
socket activation, live watchdog behavior, storage interruption recovery, or
H616 acceptance.
