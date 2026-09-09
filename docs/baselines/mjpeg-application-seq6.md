# MJPEG startup fix: sequence-six host checkpoint

Source and builder commit `975d350dad6edb826bdd54ce0e5a57f35ac91975`
contains the one-request MJPEG mount fix. Full host check
`8aa3185a029d45b2ab6b79c03d3d6a3e` passed, alongside the isolated actual
React-component red/green fixture described in
`status-outage-browser-first-attempt.md`.

The initial build using an older shared target directory was rejected because
dependency paths remained in ELF data. No archive from that attempt was
accepted. Fresh independent target builds `5418a14ad4cc40c891f1f6127b09d2bd`
and `a1486a8b456240a0a1aa67b3a8dcb37c` passed all ELF/path checks and produced
byte-identical prototype archives:
`5e8446a3147ab70080d8633f63bc54173466ed29ee300d33f6ec89e429458e33`.
The old target directory was preserved.

Separate deterministic runtime-package builds compared byte-identical:
`7e95fce2aab87cbc6f24bd52503db5e6057a5357563690523505af951861f40d`.
The inert intermediate package format remains `0.1.0~dev2`; it is not a
request to install or downgrade a Debian package on the device.

Two local signing runs with the existing enrolled key produced identical
`0.1.0-dev5`, sequence-six archives:

- Archive: `4f5a307e2d1014de360dc2cdb75cb5186a60511cfd772f3be0b98b8aff9a6036`.
- Archive size: 8,601,600 bytes; 27 payload files totaling 8,560,269 bytes.
- Manifest: `8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac`.
- Target slot: `app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac`.
- Minimum system sequence: one.

The host Rust supervisor
`430690f7e9d2ec6d0fc8b658deb9010becd8a6a762f5903f979585f7f07db32b`
verified signatures, all immutable payload files, repeat staging, initialization,
tamper/wrong-key rejection and unchanged state after negative tests. Its
temporary roots were removed. No payload binary was executed and no device
was contacted. Receipt: `evidence/private/mjpeg-seq6-host-verification.json`.

The new named `seq5-to-seq6-mjpeg` driver profile binds the accepted sequence-five
source slot and exact sequence-six target. Unlike earlier transitions, it must
retain and validate the permanent current/candidate status overrides. Only the
known temporary health-failure override is permitted during the staged
rehearsal phase; other overrides fail closed.

The transition driver's full host check passed in job
`fe2f1829876c482bb87920e30b6ae45b` (exit 0).

Host transition validation, device preflight, rehearsal, activation, phase-bound
browser checks and finalization/cleanup are still required. The device has not
been updated by this checkpoint. Existing sequence-five status fixtures must
not be mistaken for sequence-six acceptance.

Read-only device preflight `ab459f629b244c2cb6f7fc657027f8fa` subsequently
passed using the host-validated dev6/dev7 package bindings. It verified the
installed dev7 files, trust/config, persistent status transport, source-slot
executable, and absence of an application transaction or target slot. The
application remained confirmed at sequence five, floor five, generation 22,
with no pending operation or rollback. State/slot fingerprints and serving
identities were unchanged. No remote staging files, guards, installation or
activation were created. Rehearsal and activation remain pending.

The complete host check including the read-only preflight tools passed in job
`140741b6fa0b40ae963479065d16b416` (exit 0).
