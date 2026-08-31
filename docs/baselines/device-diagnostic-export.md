# H616 dual-socket diagnostic export checkpoint

On 2026-08-31, the first authenticated ephemeral sealed-window export boundary
passed on the designated test-only H616 device. Collector source
`9143fd8d660a7337011e1a9b63f68cc37510ded2` links the producer and export
listeners over one runtime. Gate source `a17b867` adds the dual-socket matrix,
and final source `437b65ff0168904d21f18764f1531814b9acf471` hardens its
single sealed-state fixture with a compile-time path, `O_NOFOLLOW`, and exact
type/owner/link/mode/size validation.

The unshipped collector atomically binds mode-0660 `collector.sock` and
`export.sock` inside one validated mode-0750 runtime directory. Producer and
export loops share one `Arc<DiagnosticCollectorRuntime>`. Completion, error, or
panic of either task triggers a drop-guard shutdown broadcast; both tasks join
before inode-bound socket cleanup. A partial second bind removes only the first
socket created by that owner.

The export listener accepts only the semantic platform-broker role returned by
fixed-name lookup and connection-bound `SO_PEERCRED`. It serves one exact
16-byte request and one response per connection, has two non-waiting permits,
bounds request reads to two seconds and response writes to five seconds, and
releases collector state before response I/O.

## Host and AArch64 evidence

The exact final source passed the complete host matrix with 138 Python tests,
locked workspace format/check/test/Clippy, source/packaging/systemd boundaries,
Python compilation and BasedPyright, production TypeScript build, ESLint, and
Prettier. The focused Rust matrix contains five collector, nine export-listener,
five export-protocol, 25 peer-adapter, and 11 window-policy tests. The listener
suite also passed 20 consecutive direct runs. `cargo audit --deny warnings`
loaded 1,233 advisories and accepted the exact 158-package graph.

Two clean independent builds of source `437b65f` produced byte-identical
4,796,918-byte prototype archives:

`75da5aa972f99d6c972bbd638d1f89a864828432ff4d3ed1c7e016d0708b6e24`

The collector remains absent from those archives. Their manifests identify
the exact source and retain `releaseAccepted=false` and
`deviceAccepted=false`.

The separate 715,032-byte static-PIE collector binaries were byte-identical:

`6aaa09fab9c3c513b56cce23c462dbcae67d8922ae8ca18c0d1c1cd662d7e9f4`

The 133,864-byte collector rlibs matched:

`a7608cd226549caf7fbaf52ad8217c59de5696e6aa332ee5770c192b004fc2ec`

Their 67,592-byte `lib.rmeta` and 64,320-byte ThinLTO members matched with
SHA-256, respectively:

`5810204b3aa61e4b03f98ce8f47acabe9911c7c160194670386860a5539d3af4`

`57aedb6b80cb713089b0032efaa7655fcac34426adc38935921af4387f4bbfb7`

The export-listener, export-protocol, peer-adapter, window-policy, and
collector-policy rlib pairs also matched directly with their recorded hashes.

## Native H616 result

Before mutation, the gate confirmed both sockets, runtime directory, unit,
binary, six fixed identities, shared group, watchdog, and marker-owned stage
were absent. KVMD and KVMD Nginx were active and the root filesystem was
read-write. A ten-minute rollback watchdog was armed before applying the exact
sysusers templates. All four account databases were copied with metadata and
pre-change hashes; restoration was allowed only while the live files matched
their expected post-sysusers hashes.

The live collector then passed all of the following:

- both fixed sockets had exact collector/shared-group ownership and mode 0660;
- only the platform-broker UID could read the sealed snapshot; root and the
  server UID were rejected without a response or state access;
- two authenticated partial export requests occupied both fixed permits, an
  authenticated third connection was rejected immediately, and a producer
  still received its accepted response within one second;
- an exact malformed request received no response and a partial request closed
  within the two-second bound;
- typed producer traffic rotated one nonempty sealed window;
- two reads returned byte-identical instance-bound snapshots;
- a wrong-instance acknowledgement returned mismatch and left the snapshot
  byte-identical;
- the exact instance/window acknowledgement succeeded;
- a later read and repeated acknowledgement both reported no sealed window;
- producer and export partial connections both closed during one bounded
  graceful collector shutdown; and
- the prior five-source, exact control rejection, duplicate, root, unknown,
  malformed-producer, partial-producer, capability, private-network, and
  no-new-privileges checks continued to pass.

The platform-broker identity in this gate ran a fixed generated client; no Rust
export client or platform-broker route exists yet. The result accepts the
collector-side authentication, protocol, shared-state, acknowledgement, I/O,
pressure, and shutdown boundary, not a product-facing consumer.

After the run, `/etc/passwd`, `/etc/group`, `/etc/shadow`, and `/etc/gshadow`
matched their backups and pre-test hashes. The six identities, shared group,
binary, unit, both sockets, runtime directory, watchdog, sealed fixture, and
staging root were absent. KVMD and KVMD Nginx remained active and the root
filesystem remained read-write.

No raw log, network listener, device node, HID injection, GPIO access, media
mutation, update, reboot, physical Power, or physical Reset action was used.
The collector remains unshipped. Persistence, a Rust export client,
platform-broker/server integration, authenticated HTTP API/UI, privacy review,
soak, reboot, packaging, and release acceptance remain separate future gates.
