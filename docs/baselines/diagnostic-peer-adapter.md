# Safe diagnostic peer adapter checkpoint

On 2026-08-31, source commit
`b62d827c107d035a9b1580476fc7662054e4e315` completed the first safe Rust
account-resolution and Unix peer-credential adapter. The
`bli-kvm-diagnostic-peer-adapter` crate forbids unsafe code and is not linked
into any server, broker, process, package, API, UI, systemd unit, or device
runtime.

The adapter contains exactly seven compile-time names: future collector,
server, platform broker, video proxy, control broker, update supervisor, and
shared group. There is no caller name, path, environment, numeric override, or
fallback. Safe `nix 0.30.1` `User::from_name`/`Group::from_name` calls resolve
the fixed names; returned names must compare exactly, and lookup error, absence,
or mismatch are distinct hard failures.

The resolved collector UID must equal the process effective UID from safe
`nix::unistd::geteuid`. The resulting six UIDs and shared GID then pass through
the previously verified pure assignment validator. Missing, root, duplicate,
wrong-current-user, and invalid group state have no permissive fallback.

For each connected Tokio Unix stream, the adapter calls the safe
`UnixStream::peer_cred` API. Signed PID is converted without wrapping; negative
values become an invalid bounded PID. PID/UID/GID are immediately passed to the
pure mapper, which returns only `DiagnosticSource`. Numeric peer credentials
are not returned, logged, exported, or persisted. The public credentials and
assignment containers deliberately do not implement `Debug`, preventing an
accidental generic debug print of their fields.

Six focused adapter tests cover:

- every fixed account name and a valid mock resolution;
- missing, name-mismatched, and failed lookup for all six accounts;
- group lookup failure, absence, and name mismatch;
- wrong current collector UID and duplicate assignment rejection;
- missing, negative, unknown, and wrong-group credential conversion;
- a real local Tokio Unix-stream pair proving connection-bound safe
  `peer_cred` mapping and source-only output.

The live-pair test held only the current test process credentials in memory.
It printed and persisted no numeric value. All named account/group resolution
tests used a deterministic mock; the production resolver was not invoked
because no collector account is provisioned by this checkpoint.

The full host suite passed with 122 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,779,925-byte prototype archives:

`429a2bd4bfd35f0de3dfb2a39c0f9db4802e671db723d51531056b4f7413ee53`

The unshipped adapter rlibs were byte-identical at 50,860 bytes:

`a5da5302fced52175b7cf0d20e47e9626c6f664f059e5f0553a2d641e663cfb4`

Each adapter rlib contained one 38,088-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`01cd0f6b6e316901a851744243db14031bf370d1f47aaf38c1c752e8559e05b4`

The single extracted 11,928-byte adapter ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`f2ab2586f59815633b7ab745faab7d8ded3ec2074b59ef792e86c3c347164d3a`

The updated peer-policy rlibs were byte-identical at 49,146 bytes with SHA-256
`eb46a988514414a323a5499502c0da418f5f66a7059dcc4dba66ebece8c36b34`.
Their 46,560-byte metadata matched with SHA-256
`d7e023177c5c3efdc7c5e7a8aa231e42b134487595c8124691f563811ee48992`;
their 2,220-byte codegen remained unchanged and byte-identical.

The new `nix` rlibs were byte-identical at 776,772 bytes with SHA-256
`4c9fc33304d3e20341a74a0e305c38df36440706131a3e01fbd9272ad446e0dd`.
Their 653,640-byte metadata and 117,220-byte ThinLTO member were byte-identical
with SHA-256 `0aac5671aa6e06c1837f44dd7a54db78c4a2e8f0b7bf7ddf9b8145380c96312b`
and `c8c61b34e4652baba5c431e3db3e2d3ad95917a11a9a4281a3bf10ef4b653bf8`.
Collector-policy and export layers also remained byte-identical.

The new workspace member and lock entries changed the development runtime
binary hashes relative to the prior checkpoint even though their sizes stayed
constant and `cargo tree -p bli-kvm-server` contains neither adapter nor nix.
No cross-commit binary-equality claim is made. Within this exact commit, both
independent runtime/bundle builds are byte-identical, and the archive manifest
retains `releaseAccepted=false` and `deviceAccepted=false`. No diagnostic crate
is present in the prototype.

The locked graph adds only `nix 0.30.1` and its build helper
`cfg_aliases 0.2.2`; both declare MIT. Nix default features are disabled and
only `user` is enabled. Existing Tokio, libc, bitflags, and cfg-if packages are
reused. Locked offline metadata resolves 152 packages: 46 workspace and 106
external, with no missing external license expression.
`cargo audit --deny warnings` refreshed and loaded 1,233 RustSec advisories and
accepted all 152 locked dependencies.

No device or named live-account gate was run at this adapter-only checkpoint.
There was no listener, collector process/account, runtime composition,
instance/clock source, systemd unit, server export protocol, authenticated
API/UI, package, or advertised capability. Raw vendor logs remained outside
the design.

Source `6de71c28df1eb521108a955d046e098feb3bacb3` later adds the unlinked
in-memory composition core without weakening fixed-name lookup or peer
credential disposal. It still creates no listener, process, account, clock,
service, API, UI, or capability; see `diagnostic-runtime-core.md`.

Source `89050939af218ddbdd7143fb63e320e1ca9eac67` then adds the unlinked
caller-bound listener and I/O owner while retaining the same fixed-name and
connection-bound credential path; see `diagnostic-listener-runtime.md`. It
does not bind a path or create a process/service capability.
