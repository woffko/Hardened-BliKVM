# AArch64 build checkpoint

Verified on 2026-08-30 with Rust 1.95.0 and target
`aarch64-unknown-linux-musl`.

Two independent target directories at source commit
`83ae0da7a43775d947d927a24999898e7c4a444b` produced byte-identical release
binaries:

| Binary | Size | SHA-256 |
| --- | ---: | --- |
| `bli-kvm-server` | 1,694,880 bytes | `33876305bc1835e5495c47b2a18245a7f84639387a139c2abbe9c1f41623baeb` |
| `bli-kvm-platform-broker` | 983,936 bytes | `0dbc9598f37374aff99e55c9ee41b328bd0c811d3439e6ffca64b54e1af5b906` |

Both are stripped AArch64 static PIE executables with `ET_DYN`, GNU RELRO,
non-executable stack, BIND_NOW, and PIE flags. Neither has an ELF interpreter,
`NEEDED` dependency, RPATH, or RUNPATH.

The linker wrapper requires the complete musl static-PIE CRT sequence
`rcrt1.o`, `crtbeginS.o`, and `crtendS.o`. It rejects partial, foreign, or
non-self-contained CRT substitutions. This is necessary because adding `-pie`
to rustc's ordinary static CRT sequence creates an `ET_DYN` file without the
musl self-relocator: the earlier server artifact exited with SIGSEGV before
`main` on H616. A link probe verified the corrected 488-byte `rcrt1.o`
`_start_c` and `__dls2` before the full builds.

The D: 9p mount reports mode 0777 for temporary files; packaging installs the
verified binaries as mode 0755 in a Linux staging filesystem.

Reproduce and re-check with:

```sh
scripts/build-aarch64.sh /mnt/d/debug/blikvm/hardened-blikvm/target-aarch64
```

The commit-bound server was subsequently executed successfully as an
unprivileged identity on H616. See `device-prototype.md`. This does not prove
KVMD broker credentials, systemd installation, video/HID/ATX/media behavior,
or recovery.
