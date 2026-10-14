# The target agent, Windows: gates and slots (2026-10-14)

Plan: `docs/baselines/work-plan-20261014-target-agent.md`.

## Gate 0: the Windows build

`3c940a7` adds `crates/bli-kvm-target-agent`, one program for the target's
Windows with the user's rights: an icon in the notification area with its
name, version and commit, a menu with Quit; it comes back when the taskbar
is started again and runs once per session. `scripts/build-target-agent.sh`
builds it on the development host with `cargo-xwin` and the Windows SDK
already in its cache (no download): the `x86_64-pc-windows-msvc` target,
the C runtime linked in, no debug information, the build's paths mapped
away and the linker's timestamp made from the content (`/Brepro`). The
first try left two builds apart in 20 bytes: the PE timestamp and the GUID
of the debug file, which followed the build directory's path; without the
debug file the bytes are the same. `scripts/check.sh` lints the Windows
code and builds the program twice in different directories, comparing the
bytes.

Two builds of `3c940a7` in different directories:
`c0ada86edab2fa558cafbcffd889fa48f2a68506217ba6f33bad6e4b4309be46`
(213,504 bytes, PE32+ for Windows, a GUI program; no path of the host in
it). The operator started it on the target from the host's disk: it
started without a warning from SmartScreen or Defender, and showed its
icon as "Hardened BliKVM agent 0.1.0 (3c940a7e681c)" with Quit (2026-10-14,
"Запустился без предупреждений"). A program copied from a local or USB
drive carries no mark of the web, which is what SmartScreen judges; one
downloaded from a release page would. Gate 0 passed.
