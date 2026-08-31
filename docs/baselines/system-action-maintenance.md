# Confined system-action store maintenance checkpoint

On 2026-08-31, source commit
`b455f06df08c56061eeaea0d888be7b4b5fdd3c3` added the unshipped
`bli-kvm-system-action-maintenance` boundary. Its parser accepts exactly one of
two argumentless commands:

- `initialize-empty` creates only the canonical empty generation-one store and
  immediately reloads it as two healthy equal copies;
- `inspect-empty` performs no initialization or repair and accepts only that
  same clean empty generation-one state.

Unknown arguments, extra values, degraded copies, later generations, nonempty
state, backend errors, and wrong initialization generation fail closed. There
is deliberately no repair, floor override, path, reboot, shutdown, service,
shell, network, or actuator command. Success output is one fixed non-identity
line containing command, empty result, generation one, and healthy copies.

The binary opens only the store's compile-time production root. Its static
systemd init and inspect units are root-only one-shots with no install section.
Both use PrivateNetwork, PrivateDevices, exact ReadWritePaths, strict system
protection, AF_UNIX-only address families, empty capability/ambient sets,
closed device policy, IP denial, native system-call architecture and the
`@system-service` filter. The exact tmpfiles declaration is root:root mode 0700.

The units, tmpfiles file, and binary are intentionally absent from the
prototype bundle. Host verification installed synthetic binary placeholders,
loaded the new source templates, ran `systemd-analyze verify`, enforced the
offline security threshold, and parsed the tmpfiles configuration. Actual
tmpfiles creation is deferred to the root-capable reversible device gate; the
host test runner cannot perform synthetic-root chown.

Five focused Rust tests cover the exact two-command parser, empty
initialization, read-only inspection, degraded/nonempty/wrong-generation
rejection, and stable backend failures. Packaging tests prove the maintenance
crate is the only store consumer, lacks action/process/network surfaces, has
the expected static confinement, and is not selected by the bundle builder.
The full host suite passed with 102 Python tests plus workspace
format/check/test/clippy, source, packaging and systemd boundaries,
BasedPyright, production TypeScript build, ESLint, and Prettier. Two attempts to
obtain LSP diagnostics for the new Rust file hit the project server's
100-document limit, so new-file LSP diagnostics are not claimed; compiler and
Clippy checks with warnings denied passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,392-byte prototype archives:

`1e56a98961f42bac7d567195acc9e8bbbed72b043a7965e4fffce4e3bbf009e5`

The policy and journal rlibs remained byte-identical. The updated store rlibs
were byte-identical at 130,380 bytes:

`6106424a42e4c1f87c600b898aae87a61c92166f09b68a12b6debaecabb55f55`

Their 60,960-byte AArch64 `lib.rmeta` and 68,524-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`16137e7bf14f0a44fade5ce9e339396b80555362159a8ad5c9efed2ed95ec1d8`

`9240dee525629caa71bf1921acbace20dbf75c1aa1765f9b978934bc969abffa`

The unshipped maintenance rlibs were byte-identical at 62,718 bytes:

`cf3a5a593ce95dd94ac30e8cc9366ad8697aeefff5d4c5881fb92a0b9ece66e9`

Their 47,576-byte AArch64 `lib.rmeta` and 14,024-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`66163c5d183a96a92da2b20c2c979443389625b9f5a17c9a1ae8bacd6228a964`

`fc759e51933ea623d602b466fe8ae0d54668a0bc57d575b78f7f887d3100552a`

The maintenance executables were byte-identical 441,928-byte AArch64 static
PIEs:

`a323032a149d5bb17c8632141ce4f41607465bb29b9d5c2b05663f32ddfd57a2`

Direct byte comparison passed for both archives, all lower rlibs, the
maintenance rlib and binary, and the extracted store/maintenance metadata and
codegen members. The archive manifest identifies the exact source commit and
retains `releaseAccepted=false` and `deviceAccepted=false`; archive inspection
confirms no maintenance binary, unit, tmpfiles file, policy, journal, or store.

No production root was created and no device action was executed in this
source checkpoint. Next acceptance requires a rollback-gated H616 test that
snapshots absence, stages the exact binary/units/tmpfiles, creates and inspects
only an empty store under systemd confinement, then removes every created path
and proves the original absence plus unchanged services/rootfs mode. Even that
will not establish an external rollback floor, power-cut durability, or system
action eligibility.

## Reversible H616 production-root gate

The exact 441,928-byte AArch64 maintenance static PIE from source commit
`b455f06df08c56061eeaea0d888be7b4b5fdd3c3` has SHA-256:

`a323032a149d5bb17c8632141ce4f41607465bb29b9d5c2b05663f32ddfd57a2`

The rollback-gated device orchestration tool was committed at `761bab5` and
passed nine focused shell/parser/rollback tests plus the full 111-test Python
gate before SSH. It uploaded only the exact binary, two source units and
tmpfiles declaration into a marker-protected `/tmp` stage. A ten-minute
transient systemd rollback timer was active before the first persistent write.

Preflight proved that the production root, maintenance binary parent/binary,
runtime units, stage and watchdog were absent; rootfs was read-write and the
existing KVMD services were active. The test then:

- installed the exact maintenance binary and runtime-only unit copies;
- created the exact root:root mode-0700 production directory through the
  source tmpfiles declaration;
- ran confined `initialize-empty` successfully;
- verified two equal, root-owned, single-link, 2,076-byte mode-0600 copies at
  empty generation one;
- ran confined `inspect-empty` successfully;
- invoked the marker/hash/metadata-gated rollback before the watchdog deadline.

The separate final-state check proved that the production root, binary parent,
binary, both runtime units and remote stage returned to their original absent
state; the watchdog timer/service were inactive, rootfs remained read-write,
and both KVMD services remained active. The local hash-named binary snapshot
was removed after the mode-0600 private receipt was validated. The receipt is
Git-ignored and contains no routing, credential, hostname, address, serial or
other device identity.

This accepts reversible empty provisioning and inspection on the current H616
production filesystem under the source confinement. It does not accept an
external rollback floor, nonempty journal state, repair, power interruption,
reboot/shutdown execution, actuator wiring, packaging or reboot persistence.
