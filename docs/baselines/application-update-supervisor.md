# Application update supervisor build baseline

This checkpoint records two independent AArch64 builds of source commit
`b754ae0388ba3219afed4403656dfe9383fd8f12` in separate Cargo target and
output directories under the designated temporary build root.

Both builds produced a byte-for-byte identical static PIE supervisor:

```text
bli-kvm-update-supervisor  a7f21525534a968cae4ce096b9d18583f3abf0d706a3f50b153d43ea3177dcfe
```

The complete prototype archives, including the supervisor and its systemd
service, timer, socket, and sysusers templates, were also byte-for-byte
identical:

```text
4f95cfd6961e1f075b6a8e08e75133bc9793b747e8ec22e88da3498d33fd619b
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

This baseline proves target linking, static-PIE hardening, and deterministic
artifact generation. It does not prove runtime syscall availability, systemd
socket activation, live watchdog behavior, storage interruption recovery, or
H616 acceptance.
