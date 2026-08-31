# Feature-gated diagnostic broker route checkpoint

On 2026-09-01, final source commit
`3461d770b751861810d2e9ea86ae7c0052fbfe64` completed and natively accepted the
first Platform Broker diagnostic read route. The integration is compiled only
with feature `diagnostic-export`; default broker protocol/client builds and the
prototype Platform Broker retain inventory-only schema v1.

The feature adds local schema v2 instead of widening v1. V1 diagnostics and v2
inventory are rejected. The only new operation reads a sealed snapshot. It has
no acknowledgement request or call, persistence, retry loop, configurable
path, server call site, HTTP route, UI, capability, package, install unit, or
collector startup dependency.

Before relaying a snapshot, the broker resolves all six fixed accounts and one
shared group. The existing pure peer policy requires every UID to be nonzero
and distinct and the GID to be nonzero. The current effective identity must map
to Platform Broker. Connection-bound `SO_PEERCRED` must then map a present,
positive-PID peer to the exact Server role and shared GID. Numeric credentials
are neither returned nor logged. Root, Platform Broker, Control Broker, and any
other role receive only the fixed `unauthorized` error and no snapshot.

The export client now parses every sealed payload back into `DiagnosticRecord`
and its closed component/event/reason/result/resource/health enums without
allocation, re-exports it, and requires byte identity. Alternate whitespace,
number spelling, severity, field order, count, trailing bytes, unknown values,
and invalid UTF-8 fail closed. Local JSON decoding repeats this validation;
snapshot `Debug` omits the bundle.

The feature broker starts and serves a typed `backend_unavailable` result while
the collector and export socket are absent. Once a sealed window exists, reads
do not mutate it. Two local schema-v2 reads returned the same frame, which was
reconstructed and compared byte-for-byte with the direct export response. A
subsequent direct read remained unchanged. Only the gate's later explicit
collector-protocol test acknowledged the frame; the broker path never did.

## Host verification

The source host job `3d6f7b0e386b45a4bd1d3db05baedad8` first accepted the
default-off implementation. Gate-source job
`1ab39dfc91b34a29b3ebde0b29e038e7` accepted the rollback extension. Final
schema-v2 job `d1501cd9eacd468d8f1074a3dca6cc5a` exited zero at `3461d77`.

The final matrix passed default workspace format/check/test/Clippy, an
all-features workspace check, explicit protocol/client/Platform Broker feature
tests and Clippy, source/packaging/systemd boundaries, 140 Python tests,
compilation and BasedPyright, production TypeScript/Vite build, ESLint, and
Prettier. Feature rustdoc with warnings denied passed. LSP diagnostics were
clean for broker protocol, broker client, and Platform Broker; the export core
and client hit the project server's 100-document limit on both attempts, so
their compiler/Clippy/test evidence is authoritative.

Locked metadata remains 159 packages: 51 workspace and 108 external, with no
missing external license expression. No dependency was added. A refreshed
1,233-advisory `cargo audit --deny warnings` scan accepted the graph.

## Independent AArch64 builds

Jobs `339d7aefb633420d93e057c805b95801` and
`3295866480214c77b975aae6fa84e1d4` used separate default and feature target
trees under `/mnt/d/debug/blikvm/`. Both built the complete default workspace,
the production web tree, and separate feature-enabled Platform Broker and
broker-client artifacts.

The default 4,797,815-byte prototype archives were byte-identical:

`3acf9e9c972b45d0195c9f8e66128443b1377c30cc48a244175a680c6a9e4fc1`

Their manifest identifies source `3461d77`, retains
`releaseAccepted=false`/`deviceAccepted=false`, and includes a 998,368-byte
default Platform Broker:

`9fed878eb794906b3533ae256f666c340019436578608180c01e7342b6c0a1ec`

The default binary contains no exact export-socket literal. The separately
built 1,038,504-byte feature binaries were byte-identical and do contain it:

`dd9a63ab8e7632686f2ab451e1f2c79e21816790400638df7387d1ff1c0ab4f1`

The two 715,032-byte collector binaries also matched:

`b04bb17a8bd1cb3c5297008cb8a9f42f75421a86420017c04a8bd4522228c609`

As in the previously accepted Serde-dependent control-broker boundary, the
upper Platform Broker, broker-protocol, and broker-client rlib/standalone-rmeta
pairs have equal sizes but encode their distinct target roots and therefore do
not match byte-for-byte. Their actual ThinLTO members do match:

- Platform Broker: 147,596 bytes,
  `fc25c50f46a4bde8ca9c40d1a202ff82b8483da4e44d11f8fb0bbd3c955b274f`;
- broker protocol: 546,428 bytes,
  `8295f85a3fbf6bc38e036118a88e8e2e3eccf44bbe4c93286f1cd86ee309cb1d`;
- broker client: 5,960 bytes,
  `08721568987c97503a3be8b4f9643a2218faa2062970c0361080d6d4ebbe696f`.

The lower 136,198-byte diagnostic export-client rlibs matched completely at
`1495bb9b968216e1cb7229a63d8a60cb73172c741e743b8dd76fb428196971d3`;
their 117,670-byte standalone metadata matched at
`45c8c39eefafee38ed03cb079db72a2084e9891c4f00af27e4717a59b43fe797`,
and their 16,692-byte ThinLTO members matched at
`e10518674e609b58d733af33b4dd38a4a88ebeb0f4711e348fff49c2036a776f`.
Both final ELFs and the default archives passed direct `cmp`.

## Native H616 acceptance and rollback

Longrun job `9571f8a0ebbc4b9496945caac0ee6ed9` exited zero in 44 seconds.
The private mode-0600 receipt is
`evidence/private/device-diagnostic-broker.json`; it contains only allowlisted
facts and hashes, no credential, numeric identity, raw bundle, process output,
or log. Its exact artifacts are the feature broker and collector hashes above.

Preflight required AArch64, rootfs `rw`, active unchanged KVMD and KVMD Nginx,
all six identities absent, both runtime directories/sockets/units absent, exact
account-database metadata, sufficient `/tmp` capacity, and an armed rollback
path. The ten-minute systemd watchdog covered every temporary account,
binary, unit, socket, runtime directory, and account-database change.

The gate accepted:

- broker startup and typed unavailable response before collector startup;
- exact Platform Broker current identity and exact Server downstream peer;
- two identical schema-v2 reads and byte equality with direct export;
- root, Platform Broker, and Control Broker denial;
- no broker acknowledgement;
- mode-0660 static shared ownership for broker/producer/export sockets;
- private network namespaces, empty capability sets, and `NoNewPrivileges` for
  broker and collector;
- the preceding five-producer, pressure, malformed/partial, mismatch/ack, and
  dual-shutdown collector matrix.

Rollback restored exact account-database hashes and removed the transient
broker/collector processes, units, binaries, credentials, watchdog, stage,
both runtime directories, and all three sockets. Final facts confirmed KVMD and
KVMD Nginx stayed active, rootfs stayed `rw`, and every original path/account
remained absent. No physical Power/Reset, hardware action, KVMD restart, raw
vendor log, public network listener, persistence, HTTP, or UI was exercised.

This checkpoint does not make diagnostics a shipped capability. The next layer
must define encrypted durable storage, ownership/mode/size limits, atomic
write/readback/sync, crash reconciliation, and instance/window-bound
write-before-ack behavior before any acknowledgement or HTTP/UI design.
