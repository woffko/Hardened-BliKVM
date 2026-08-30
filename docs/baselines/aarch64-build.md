# AArch64 build checkpoint

Verified on 2026-08-30 with Rust 1.95.0 and target
`aarch64-unknown-linux-musl`.

Two clean target directories produced byte-identical release binaries:

| Binary | Size | SHA-256 |
| --- | ---: | --- |
| `bli-kvm-server` | 1,694,928 bytes | `3d444cf48135d6bb905abd1f53d15a0e06a64fa00717a4c96af202380c152d1e` |
| `bli-kvm-platform-broker` | 983,792 bytes | `7d6849ae1501a3885d5623010820a9d7e561b92516a0c7d5f4a69421a143e39d` |

Both are stripped AArch64 static PIE executables with `ET_DYN`, GNU RELRO,
non-executable stack, BIND_NOW, and PIE flags. Neither has an ELF interpreter,
`NEEDED` dependency, RPATH, or RUNPATH.

The source tree was uncommitted at this checkpoint, so these hashes identify a
development build only. Any source or lockfile change invalidates them. The D:
9p mount reports mode 0777 for temporary files; packaging must install the
verified binaries as mode 0755 in a Linux filesystem.

Reproduce and re-check with:

```sh
scripts/build-aarch64.sh /mnt/d/debug/blikvm/hardened-blikvm/target-aarch64
```

This proves reproducible cross-compilation and ELF properties. It does not
prove execution on H616, KVMD socket compatibility, systemd sandbox behavior,
video/HID/ATX/media behavior, or recovery.
