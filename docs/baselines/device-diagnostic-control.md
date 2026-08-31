# H616 diagnostic control-broker integration checkpoint

On 2026-08-31, the first runtime producer boundary passed two complementary
gates on the designated test-only H616 device without enabling a control or
diagnostic-export capability. Source
`a143a75fd48e8802c1cc963c7d98312e12ecc726` links the
bounded reporter only into the denial-only control broker. Follow-up
`bc2e5f05a3bd545529537841d7c0493ee87459a0` removes that broker's direct
path-dependent error derive. Gate source `e4f12ed` and documentation commit
`34187aa8ee6a55c7619e9ba6851f399e5210f4b9` complete the accepted checkpoint.

For every valid control request, the broker first writes the exact
`capability_disabled` response and only then attempts one fixed
`RequestRejected(CapabilityDisabled)` event. Reporter construction is
best-effort, queue receipts are discarded, and no counter, delivery result, or
shutdown future returns to the control path. The collector remains excluded
from the prototype bundle and no service ordering dependency was added.

## Host and reproducible AArch64 evidence

The full host matrix passed with 136 Python tests plus locked workspace format,
check, test, and Clippy; source, packaging, and systemd boundaries; Python
compilation and BasedPyright; production TypeScript build; ESLint; and
Prettier. Five focused broker tests cover all valid control families, every
reachable queue receipt, response-before-diagnostics ordering, bounded task
termination, and invalid runtime-directory state. The broker tests also passed
20 consecutive direct runs. `cargo audit --deny warnings` loaded 1,233
advisories and accepted the exact 156-package lock graph.

Two clean independent builds of source `34187aa` produced byte-identical
4,795,694-byte prototype archives:

`d13b544c531f5e3be85ef2fe17163bd355649c958880d83b9668464d54f66fb0`

All six shipped static-PIE ELF pairs and the web tree matched directly. The
749,128-byte control-broker binary pair has SHA-256:

`c07ea39e3483ead676944783a2d90345d9ca29b19b9b14f335cb5592599b148e`

The separate 666,032-byte collector binary pair also matched:

`4e28024e7143e94054a4584e7c76a2430a61cf5eb6bb08792b42fbcd67fd2fcc`

The archive manifest identifies exact source `34187aa`, retains
`releaseAccepted=false` and `deviceAccepted=false`, and records the unchanged
web tree SHA-256:

`d85320a7cb3b6db3b2b5b55117ebab167f7c566bce69839a84f767fbe4731c44`

The changed broker's 11,592-byte ThinLTO member was byte-identical:

`1f95516c4bff4813936807d98be2b43d7c6f9e5be91a7ab8349b356d76ad9494`

Its non-shipped 90,928-byte `lib.rmeta` remained build-root-dependent, so the
103,680-byte rlibs were not accepted as equal. The same metadata-only variance
already existed in the pre-integration broker and was traced into the existing
control-protocol/Serde/core dependency chain. It did not enter the identical
ThinLTO member, final ELF, or deterministic archive. Rewriting the established
Serde protocol solely to normalize an unshipped compiler metadata container is
outside this checkpoint.

## Real collector gate

The current collector was installed only through one marker-protected stage.
A ten-minute systemd rollback watchdog was armed before the exact base and
diagnostic sysusers fragments were applied. `/etc/passwd`, `/etc/group`,
`/etc/shadow`, and `/etc/gshadow` were backed up with metadata and hashes; each
restore was allowed only while the live file matched the expected post-sysusers
hash.

The real collector then accepted the exact eight-byte
`RequestRejected(CapabilityDisabled)` frame from the
`hardened-blikvm-control` UID. It also repeated the prior five-source, root,
unknown-UID, duplicate-source, partial-frame, malformed-frame, and graceful
shutdown checks. The collector retained zero effective/bounding capabilities,
`NoNewPrivileges`, a private network namespace, the fixed mode-0660 socket, and
the exact peer-credential policy.

After the run, all four account database files matched their backups and
pre-test hashes. The temporary identities, collector unit, binary, runtime
directory, socket, watchdog, and staging root were absent. KVMD and KVMD Nginx
remained active and the root filesystem remained read-write.

## Stalled diagnostics and browser gate

The production control-broker binary was then run against a marker-owned exact
frame capture on the fixed collector path. The capture verified the canonical
control-capability-disabled event and withheld its response for seven seconds.
While the reporter worker was stalled, a non-root client issued 32 valid
denial-only requests. Every HID, ATX, and virtual-media request returned the
matching `capability_disabled` response; every individual response remained
below one second and the complete sequence below four seconds. With one worker
stalled and an eight-event ring, this necessarily exercised queue pressure
without feeding any receipt back into the broker.

The broker stopped within the bounded child-stop window while the diagnostic
capture was still stalled. The temporary diagnostic socket, directory, marker,
capture, control runtime, and all other stage paths were removed. No collector
response, ambiguous delivery, queue-full receipt, or reporter shutdown state
changed the control result or retained process liveness.

The same gate repeated the TLS 1.2/1.3 and Chromium regression. Secure session
and CSRF cookies, password rotation, logout, stream revocation, read-only
inventory, temperature/RTC rendering, MJPEG decoding, and four reconnects
passed. The browser observed zero control requests and zero WebSockets; HID and
power remained unavailable and virtual media remained read-only.

The independent final-state request confirmed that all stage and diagnostic
paths were absent, both test ports were free, KVMD and KVMD Nginx remained
active, and the root filesystem remained read-write. No raw log, device node,
HID injection, GPIO access, media mutation, update, reboot, physical Power, or
physical Reset action was used.

This checkpoint accepts the compositional fixed-frame mapping and the
post-response, non-blocking diagnostic producer inside the denial-only control
broker. It does not claim one externally observed simultaneous
broker-to-real-collector record because the collector intentionally has no
counter/export surface yet. It does not ship the collector, persist diagnostic
windows, expose authenticated diagnostic API/UI, accept raw logs, enable any
control backend, or establish privacy, soak, reboot, or release acceptance.
Those remain separate gates.
