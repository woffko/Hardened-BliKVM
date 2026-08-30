# Application update supervisor build baseline

This checkpoint records two independent AArch64 builds of source commit
`858f12f57dc52274f3553ed6469b8582f0cd9726` in separate Cargo target and
output directories under the designated temporary build root.

Both builds produced a byte-for-byte identical static PIE supervisor:

```text
bli-kvm-update-supervisor  dba225bd9b4040a7aa8a32d670c2f16a35c78f280b65e54af96df131a158e65e
```

The complete prototype archives, including the supervisor and its systemd
service, timer, socket, and sysusers templates, were also byte-for-byte
identical:

```text
4845c62c8cb8e4d9523cdbb5cb877684ce544aa822fd868e45b2d16d006ee368
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
linked supervisor now also includes marker-safe staging recovery and explicit
reference-aware `gc N` with durable deletion tombstones. The two-build
comparison covers those exact command paths.

This baseline proves target linking, static-PIE hardening, and deterministic
artifact generation. It does not prove runtime syscall availability, systemd
socket activation, live watchdog behavior, storage interruption recovery, or
H616 acceptance.

## Candidate-process checkpoint

Source commit `2be3f7ff767ae9ed3fa07d843310d667ca7dd418` added bounded
lock acquisition and stable local-only manager failure codes. Two independent
AArch64 builds produced the same supervisor
`e6dd0b2396fd230830cb87d1a5b14bde03436df4b3be12d01b612b3fc87edee4`
and byte-identical prototype archives
`e7e7ffa6e43007fcf0f9fdf2f864fcf2281553ec44ba75d3c1883f06c48ca674`.
That exact supervisor passed the isolated H616 candidate-process gate described
in `device-update-process-orchestration.md`.
