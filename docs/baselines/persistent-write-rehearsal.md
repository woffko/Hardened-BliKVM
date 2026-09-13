# Persistent write-policy rehearsal

Date: 2026-09-09. Source: `8bafb6078e773df7a6afef5d3a195ef66b3389de`.
This is a successful same-boot wiring/restoration rehearsal, not approval to
retain HTTP write admission and not completion of the update acceptance plan.

## Evidence

The complete host check passed in job
`ce15bc30ab124afa9bfc378a99fb182d` (104.112 seconds). The installed read-only
preflight passed in job `7979b2ef5de542a6b5fa602d8d241ad0` (40.047 seconds).
The installed prepare/restore/cleanup rehearsal passed in job
`759d687be4be42ff8f844c2478cf54be` (76.079 seconds).

Private receipts, excluded from Git:

| Receipt                                              | SHA-256                                                            |
| ---------------------------------------------------- | ------------------------------------------------------------------ |
| `evidence/private/persistent-write-preflight-a.json` | `b4cb96ff19d2a4c7a90420ac06cd486e67a77dc85d630ecd76f1b87472f0179b` |
| `evidence/private/persistent-write-rehearsal-a.json` | `dd5abc018af6902aca34714f889a29d4d0c2f6c041ec305eb3681ee3361dcfb4` |

The controller verified the retained dev9 package, prepared exactly one owned
policy transaction, armed its independent deadline and boot guard, installed
the two server write overrides and socket-enable links, and verified loaded
and running configuration. It then closed and masked write admission, observed
natural pipeline settlement, removed only its owned policy, restarted the
confirmed server, verified preserved runtime/state, released the masks, and
disarmed recovery.

Cleanup verified idle owned services and absent loaded guard dependencies,
removed the dormant owned guard unit definitions, and archived the root-private
code and original-absence backup under the restored-transaction namespace.
The private receipt records its exact archive path. No backup or signed slot
was deleted. Temporary SSH staging was removed.

Independent observations before and after reported the same boot, selected and
confirmed slot eight, floor 8, generation 40, previous slot seven, and no pending
or rollback state. Both write transports and loaded/running write settings were
closed after restoration. The controller's preservation checks compared all
retained immutable state entries and protected account/TLS/trust/Nginx/peer
facts against the preparation snapshot.

## Boundaries

No application archive was sent, no application activation was invoked, and no
reboot, target-power operation, garbage collection or permanent retention was
requested. This rehearsal proves installed idle wiring/restoration and cleanup;
it does not prove restoration with active workers, power-loss/boot recovery,
browser failure/retry/cancel/logout behavior, or committed-policy survival.
Those remain distinct acceptance gates. Permanent HTTP writes remain disabled.

## September 12 browser qualification: incomplete

Fresh preflight passed on September 12. The guarded browser runs confirmed
invalid-archive rejection (HTTP 200/code -1), cancellation at 1% progress and
retry of the exact already-present seq8 archive. No activation was requested.
The logout gate has **not** passed: in the latest run, Logout returned HTTP 200
but the original partial upload returned HTTP 502. A prior timeout-only probe
did not record enough detail to distinguish that result from browser teardown.
Releasing the artificial upload throttle did not establish the required result.
Neither a timeout nor HTTP 502 is treated as successful session-revocation
acceptance. The HTTP/proxy transport path needs a local reproduction before
another installed write rehearsal; the deployed application has not been patched.

Latest source: `91905ac`. Failed browser job:
`3a942c5deacc4589aa20765a35d9e8b6`. Its native recovery verified writes closed.
Follow-up cleanup job `8beeb42e7889479684c0e467759712b4` passed, preserved floor
8/generation 40, removed owned dormant guard units, and archived the backup.

| Private receipt                                                     | SHA-256                                                            |
| ------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `evidence/private/persistent-write-browser-20260912-d.json`         | `eff7155d0861269973c5e8ddc159ff677885a809f0dee122c25ca10b864283c8` |
| `evidence/private/persistent-write-browser-20260912-d-cleanup.json` | `85e854a43c4798a71822fa53551944b4ba9696813c7f057a70179e09b4b72176` |

### Proxy transport cause reproduced locally

The read-only device probe found no request-buffering directive and no includes
in the retained Nginx configuration; response buffering alone was disabled.
It also counted an Nginx error while sending a request upstream. Both write
sockets were inactive. Receipt: `evidence/private/persistent-write-logout-proxy-20260912.json`,
job `f50b79bc690c4bf89a9a4cea650645db`.

A real loopback TCP test passed: an authenticated partially delivered upload
received HTTP 401 after logout, and no subsequent bytes reached its synthetic
Unix worker (job `13582cc1d1334d289ff0c7f05172b8b3`). An isolated Nginx 1.24.0
fixture then reproduced the distinction in job
`6fa151781ee049ad8749d47533a5279b`:

| Request-body buffering | Worker contacted before logout | Upload result                          |
| ---------------------- | ------------------------------ | -------------------------------------- |
| `on`                   | No                             | HTTP 502                               |
| `off`                  | Yes                            | HTTP 401; no worker bytes after logout |

The fixture uses a disposable local account, Unix frontend socket, loopback
upstream and temporary directories. Nginx was unpacked from Ubuntu package
`1.24.0-2ubuntu7.17` under `/tmp`, not installed as a system service. The
explicitly opt-in test is `nginx_buffering_logout_transport_diagnostic` in
`crates/bli-kvm-server/src/tests/http_transport.rs` and takes the reviewed binary
path from `BLI_KVM_TEST_NGINX`.

Nginx request buffering defaults to `on`; `proxy_buffering off` affects
responses, not uploaded bodies. See the
[Nginx directive documentation](https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_request_buffering).
The device remedy proposed at that checkpoint was streaming only the application-upload route
with `proxy_request_buffering off`, preserving authentication/CSRF and body
limits. This changed the previously preserved Nginx boundary and required
separate approval before application.

### Subsequent device streaming and system maintenance

With user approval, upload-only streaming was applied and reload-verified.
The retained configuration hash became
`a1bf9d746c15988c3806b50f5dc7a3f7a30660d2a7ead7e95d7b0b32340deb16`.
The following browser run (`persistent-write-browser-20260912-e.json`) still
returned HTTP 502 for the original upload after Logout. Its restoration and
cleanup completed; this is not a passed session-revocation gate.

The subsequent same-release system update installed 214 packages, including
Nginx `1.18.0-6ubuntu14.20`, while preserving the custom Nginx configuration.
See [system maintenance](system-maintenance.md) for the verified package and
storage baseline. The custom Nginx service was deliberately not restarted by
package installation. Its running executable must be checked and, if necessary,
restarted under the existing device authorization before attributing a new
Logout/upload result to the updated package. No post-update browser gate has
yet been recorded; permanent write retention remains unaccepted.

Full host checks passed in job `339cbfe6f14149978eb8b0ed42ac99cc` after system
maintenance. Independent executable inspection found the old Nginx master;
the scoped restart subsequently replaced it with the installed executable
(`8c9a61fc3185bb31cd5e9b82b0640ca416fadb8b1764c293d3cce6472787e0a6`).
The restart controller itself returned failure in its postchecks, so it is not
claimed as fully accepted. Independent inspection proved the new executable,
unchanged proxy configuration, active application services and closed sockets.
The subsequent native preflight (`20c0dfc052c74b0a800919ffba71a4a9`) passed:
selected/confirmed sequence 8, floor 8, generation 40, no pending/rollback, writes
closed and staging removed. A new guarded browser gate is the next step.

### Post-update guarded browser gate passed

Job `a19e0e96d2c743f1898cb98e87f4d42d` passed on source `a0a5504`.
The native controller prepared its deadline/boot-protected temporary policy,
ran the real Chromium scenario, restored closed admission, cleaned the guard
and staging, and independently verified selected/confirmed slot 8, floor 8,
generation 40, previous slot 7, and no pending or rollback state on the same boot.

The browser observed invalid archive HTTP 200/code -1, cancellation at 1%,
successful retry of the exact already-present seq8 archive, and Logout HTTP 200
from a second page in the same session. The original upload ended with
`net::ERR_ABORTED` after Logout was requested and before browser teardown.
Both pages reached Login and protected status returned HTTP 401. There were
four upload requests, no activation requests, no blocked mutations and no page
errors. This proves the specified browser acceptance, not HTTP 401 on the
original upload, native worker stop latency, or causation by one package update.

Private receipt: `evidence/private/persistent-write-browser-20260912-f.json`.
Receipt SHA-256: `c635900372411b2a3518c79e047e68cbb8eae6d49bad11bc22c89c49be696815`.
Pending-work restoration, uncommitted boot recovery, retained-policy receipt
qualification and explicit permanent-retention approval remain separate gates.

### Pending-work fixture preparation and recovered first attempt

The first `pending` attempt (`45485feebfd04ee8b9fb8dd850b229e1`) timed out
before submitting native restore; it sent no complete archive. The adapter was
missing the explicit `dev7-dev9` profile initialization used by the native
controllers, so its worker executable verifier could use the wrong hash.
Profile-before-state initialization is now regression-tested. This explains a
concrete adapter defect; the first receipt did not retain the exact timed-out
observation and is not treated as a passed gate.

Bound recovery `a9251a829b2e4c96851fe568471c3d99` restored/cleaned the owned
transaction, preserved floor 8/generation 40 and closed writes. Subsequent
hash-bound cleanup `63742ca38e854521988df3adfc5dccd6` removed its 35 staged files
and preserved the private recovery archive. The pending-work gate still needs
a successful installed run with the corrected adapter.

### Held-upload restoration passed

The corrected adapter at source `7249a8e` passed installed job
`65c31e48b7824eda8c7959076bc43f74`. It observed an exact upload instance and its
anonymous spool, submitted native restore once, then proved owned admission
masks and closed sockets while that same worker was still live and restoration
had not finished. Only then did it stop the probe heartbeat. The worker exited
naturally (`ExecMainCode=1`, `ExecMainStatus=50`) on the incomplete upload; no
complete archive, publication, activation, reboot or worker signal was issued.

Native restore/cleanup and the independent same-boot postcheck passed. Selected
and confirmed slot 8, floor 8, generation 40, previous slot 7 and absent pending/
rollback state were preserved. Temporary probe source and controller staging
were removed; the private recovery archive was retained.

Receipt: `evidence/private/persistent-write-pending-20260912-b.json`.
SHA-256: `06192da0603595f70bd85087c24bb6cea5f27d02d177fc4265d7a529d09dbb9e`.
This qualifies one real pending-upload timeout/restoration path. It does not
qualify pending activation/confirmation failure injection, early-boot recovery,
committed-policy survival, or permanent retention.

### Uncommitted policy early-boot restoration passed

Source `2f467a0` passed full host checks in job
`f849395344ed48ca8dd3d37908e79190` and the single-reboot installed gate
`d2b686bf2d944edfad5ca6f3a23fb2f1`. The boot identity changed. The exact owned
guard exited successfully at 17,033,103 monotonic microseconds after boot;
the current server started at 36,060,253 microseconds. The deadline recovery
service had not executed. Before runtime finalization, the observer proved
revoked/restored state with no terminal marker, owned closed socket masks,
absent write settings/links/dependencies, unchanged protected configuration,
zero workers/jobs and unchanged selected/confirmed seq8, floor 8, generation 40.

Only after that early-boot proof did the driver perform runtime verification,
release the fence and clean/archive the guard. Final independent observation
passed; private observation staging was removed. No upload or activation was
issued and the attached target's power/reset controls were not used.

Receipt: `evidence/private/persistent-write-boot-20260912-a.json`.
SHA-256: `453ba6a61db73dfe7ae5396789342ab9d455d27d1033bd941c7db236ce0ccd00`.
This qualifies uncommitted early-boot restoration only. Permanent retention and
committed-policy boot survival remain unaccepted.

Post-reboot Chromium closed-mode check `919ad409ac9e4e68baecfcd87680541a`
also passed: login, desktop/mobile seq8 status, three reads and two refresh
checks, no page errors and zero upload/activation requests. Receipt:
`evidence/private/persistent-write-boot-20260912-a-gui-closed.json`.
This is authenticated UI availability/status evidence, not permission to retain
write admission or proof of committed-policy boot survival.

### Provenance audit and next activation boundary

Read-only audit `932a894f28724f009b3dd580b1f13de9` compared the successful
browser, pending-upload and boot receipts against their retained root-private
archives. All native controller/helper source hashes matched the current code;
the package, serving executable and protected account/TLS/trust/Nginx-config
hashes matched across these three archives. No raw private configuration was
exported. This is provenance evidence, not a current-transaction qualification,
an audit of the earlier activation receipt, or approval for permanent writes.

The current commit contract still requires exact current-transaction evidence.
Destructive recovery prerequisites must not be relabeled as if they ran in that
transaction. Current GUI checks can be repeated without policy revocation, but
a new full activation cannot use the already-active seq8: the network supervisor
rejects it as `AlreadyActive`. Upload admission is limited to eight slots, and
all eight retained slots have been preserved. A new positive seq9 cycle therefore
requires separately reviewed/authorized slot reclamation or another explicitly
approved capacity strategy; neither garbage collection nor seq9 publication/
activation has been performed. Permanent retention also remains unapproved.

The user subsequently authorized one old unprotected slot reclamation and a
new seq9 cycle. Fresh inventory `ae59fd7755d64bae939f2b035448cc35` confirmed
exactly eight slots and no pending operation; seq8 and previous seq7 are protected.
The reviewed oldest unreferenced target is
`app-00000000000000000001-7a088d20f639fabf5ed4f59b7223cd34b8f39cb9fe2c5764acff6df1789fbc6f`.
Native `gc 5` retains the other five unreferenced slots. The guarded host tool
rechecks the complete inventory/tree hashes and idle closed pipeline, invokes
only native GC, and requires exactly that deletion with unchanged remaining
trees and status. No reclamation result is claimed at this preparation point.

Native reclamation `f7b2bdfefb59471889278742465ffb2a` subsequently passed:
only the inventoried seq1 was removed; seq2–seq8 tree fingerprints and the full
status were unchanged, including active/confirmed seq8, previous seq7, floor8
and generation40. Write admission remained closed. No disk clone was created.

Two independently packaged seq9 application archives were then built under
`/mnt/d/debug/blikvm/hardened-blikvm/production-application-seq9-{a,b}` using the
same accepted application payload/source `7c813a7844b75774fea451516fc2b82d6a579d13`,
runtime package SHA `da51a1990e39cbc7889cd369a599ddd5a62b1b2544237cb96d30bf2287fd67fb`,
and existing enrolled signing key. They are byte-identical (8,724,480 bytes):
bundle SHA `8f858ec697928f39bcfc6a7242aa2d1fdbb061b8cf9232048d9468d03dff247f`,
manifest SHA `5f1aee4802addd6c0c0b444224345624cd8b9b8c46950a2231ad7e2b42de37c2`.
Version remains `0.1.0-dev6`, sequence9, minimum system sequence1; no new
application implementation is implied by this activation-test artifact.
Host verification `evidence/private/seq9-host-verification-20260912-a.json`
passed signature/payload validation, idempotent stage, repeat-initialize refusal,
tamper/wrong-key rejection and temporary-root cleanup. It contacted no device
and executed no payload binary. Seq9 device staging/activation is still pending.

Post-GC native preflight `739b01623b4e4e19a1efa620f3ee5c9b` passed with closed
seq8 state and removed temporary staging. The browser harness now has explicit
`seq7-to8` (existing default) and `seq8-to9` profiles. The latter binds the new
archive hash, `published` disposition, generation40 confirmation and target9;
closed-state observation requires seq9/floor9/generation43 with previous8.
The persistent rehearsal driver's separate `activate` action performs one GUI
upload/activation under the existing deadline, then natural settlement,
restoration and cleanup. It does not retain write permission or retry activation
after an uncertain response. Installed acceptance of this action is pending.

### Authorized seq8-to9 activation passed

Source `1acd504` passed full host job `c8dfd4f9ab014f92abc5fefbf0bd2ce8` and
installed activation job `4fd9db143649488c8deb4ca21d65e13a`. The real GUI verified
the exact seq9 File, published it with one upload, reviewed generation40 and
issued one activation. HTTP202 carried the expected awaiting-health receipt;
no request was retried and there were no page errors or blocked mutations.

Native restoration waited for the pipeline and independently proved selected/
confirmed seq9, floor9, generation43, previous seq8, and no pending or rollback
state on the same boot. The prior seq2–seq8 immutable trees were preserved.
Write admission was closed again, the guard archived/cleaned, and staging removed.
No device reboot or target power/reset was issued by this activation gate.

Receipt: `evidence/private/persistent-write-seq9-activation-20260912-a.json`.
SHA-256: `f33ac87ccf4b30f7404895e75a892256780d1fe344807f12cbb7312aa99d72f1`.
This is a completed real update cycle, not permanent write-policy retention.

Post-activation closed-mode Chromium job `b44a4b63f0554929a66aa72283ffff19`
passed authenticated desktop/mobile seq9 status, three repeated reads and two
refresh checks, with zero uploads/activations or page errors. Receipt:
`evidence/private/seq9-gui-closed-20260912-a.json`, SHA-256
`6688d078e60921fae607709bb5ab996bd5d59730834ae8befe1981aca4675760`.
The current retained baseline is now seq9/floor9/generation43, previous seq8,
with seq2–seq9 present and writes disabled. Earlier seq8-only preparation gates
must not be rerun against this baseline without an explicitly bound new profile.

Explicit seq9 idle preparation/restoration at source `4b25cef` passed host job
`4479b93c984f40c8a09053f979522143` and installed job
`9d1dd1bebf5946eb9eec92f2926cbcda`. Its binding was seq9/floor9/generation43;
loaded/running policy, restoration, cleanup and independent final observation
passed without uploads, activation or reboot. Receipt
`evidence/private/persistent-write-seq9-idle-20260912-a.json` has SHA-256
`7271592c080280f9bfea6d01c0829ba6fb979d253623c7181571e750531708eb`.

Historical seq9 activation archive audit `589a5472ace34f2fa4bc6d02d51bfe07`
matched producer `1acd504be4b1fabc8114915d57c6864e2e51f812`, the original receipt
digest, root-owned archive binding, canonical revoked decision and terminal/
observed counters 9/43. All selected native source files matched that producer.
Receipt: `evidence/private/seq9-activation-archive-audit-20260912-b.json`.
An earlier mistyped producer SHA was rejected locally before device access.
This audit proves archive correspondence, not current-controller equivalence
or permanent-write permission.

Strict recovery-content plus archive audit `37f875da99a84b0196a95acd1b6e1d36`
also passed for the original seq8 boot receipt with producer
`2f467a07d676c2d25e3a6ef538513e01fc9d4d23`. Archived source hashes and terminal
state matched the independently parsed receipt. This remains historical seq8
evidence and performs no current-policy qualification or device mutation.

### Seq9 negative browser gate remains failed (2026-09-13)

Job `7718cfa5e91d4c36a3e3495ca624877c` rejected the invalid archive, exercised
partial cancellation and the exact seq9 retry, but failed the Logout upload
transport gate: Logout returned HTTP200 while the upload returned HTTP502.
This is not accepted as session-revocation success. Receipt:
`evidence/private/persistent-write-seq9-browser-20260913-a.json`.
Independent cleanup job `3535d939fa724f2f8a4ec98bc3f19c99` passed, proved writes
closed at floor9/generation43, and archived the transaction as
`hardened-blikvm-write-restored-b60a9e4e598047794618e147`.

Read-only diagnostic job `ee34325f64e941f891c1c6d5a759a1c1` captured Nginx
1.18.0 `writev` errno32 while sending the request upstream. Its bounded journal
window contained no connection-refused, disk-space or permission errors.
The exact upload location still had request buffering disabled, no includes,
and configuration SHA-256
`a1bf9d746c15988c3806b50f5dc7a3f7a30660d2a7ead7e95d7b0b32340deb16`.
Receipt: `evidence/private/seq9-logout-502-diagnostic-20260913-a.json`.
This supports investigating early upstream closure, not a proved root cause.

Local opt-in transport diagnostics have not yet reproduced this streaming
failure. An unpacked Nginx1.24 and locally compiled upstream Nginx1.18 both
returned401 with an unread-body pressure case. Nginx1.18 also returned401 with
continuous transmission, with two Tokio worker threads (ten bounded runs,
job `67b4ec2562034c5ab51744a45db3510b`), and when the session was revoked before
dispatch (job `5e3ea36900504e4c8add1960f48a159c`). Buffered control cases returned
502. These synthetic HTTP fixtures are not browser/TLS or installed-device
acceptance; they do not establish that the device failure is fixed. No
production handler change or permanent write-policy retention follows from
these results. Current seq9 pending/boot qualification also remains outstanding.

### Local Logout transport reproduction and bounded-discard fix

A TLS Chromium fixture then reproduced the failure without touching the device.
It sends a synthetic 8,724,480-byte body at 64KiB/s, logs out at partial progress,
and releases the throttle. Pausing only its disposable Nginx process for 150ms
after Logout queues both upstream closure and resumed upload data. Before the
fix, job `e1399383032d4b4aaeccfbadbe8e6175` returned upload502 with one `writev`
failure while sending upstream. Without that scheduling perturbation, the same
fixture returned401. This isolates a transport race; it does not prove the
physical device experienced precisely that scheduling sequence.

The server now retains ownership of the HTTP body across native-stage
cancellation. It drops the native IPC future first, then discards remaining
HTTP frames for at most five seconds under a single HTTP-upload admission
permit, with a 128MiB frame-byte budget and explicit yielding. Trailers/body
errors stop discard. No discarded bytes are forwarded to the updater; stalled
or oversized streams may still be closed rather than drained indefinitely.
This change covers authenticated in-flight session revocation, not every
possible early HTTP rejection.

The same failing TLS fixture returned401 with no upstream-send errors in job
`414ba1fbafea42898759f5060c6a7fba`. Full host checks and both opt-in Nginx
fixtures passed job `0d4a6fbc35694fb6a2bf564996890e9b`. Native cancellation is
tested independently of delayed HTTP completion, and a second authenticated
upload receives API code -409 while discard holds admission. Additional checks
cover stalled-body timeout, byte exhaustion, trailers and body errors.
Independent advisory review `bf1af5c3c86a4ab291ec63ec86e7ca22` found no blockers;
it is not installed acceptance. The source fix has not yet been built into a
new signed device application or deployed, and the seq9 browser gate remains
failed until a new installed run proves otherwise.

Final source validation job `fdcd033aae5249b799ad7d8b70f8d494` passed after adding
admission-release coverage and stopping discard at exact byte-budget exhaustion.
The fix and tests are committed as `3b57ee45463a7c90a06b61a2b7b874c7fd66df81`.

### Prepared seq10 artifact; installation not performed

The first ARM64 build rejected host paths inherited from an existing dependency
cache before producing any bundle. A fresh remapped cache passed all ELF checks.
Job `0f8c7fd0183d4f71845e372b0bdff2f6` produced two byte-identical prototype
archives in `/mnt/d/debug/blikvm/hardened-blikvm/runtime-logout-fix-{a,b}`:
4,966,024 bytes, SHA-256
`0649103c5ed24b0becd64b2aef1bfa57b98adfa3401a858ef69849041ce1c984`.
Both packaging passes used the same fresh compiler cache; this comparison is
not a claim of independent clean-room compilation.

The inert runtime packages in `runtime-logout-fix-deb-{a,b}` also match:
2,676,120 bytes, SHA-256
`6df67f5bf124e5d7eaeb40adb770359662d22a609c654fdcdb6fb21be35bf289`.
Source and builder are both the full commit above. These packages were not
installed over device services.

Signed application archives in `production-application-seq10-{a,b}` are
byte-identical, version `0.1.0-dev7`, sequence10, minimum system sequence1,
8,724,480 bytes. Bundle SHA-256:
`85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f`.
Manifest SHA-256:
`8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01`.
The existing enrolled key was used without changing trust or including private
key material. Host Rust verification job `f22f3d72561a4384b98767164a5f9dcb`
passed signature/payload validation, idempotent stage, wrong-key/tamper rejection,
repeat-initialize refusal and temporary-root cleanup. Receipt:
`evidence/private/seq10-host-verification-20260913-a.json`.
No payload executable was run and no device was contacted by these checks.

Device installation is pending a new reviewed cycle and slot-capacity check.
The last observed device retains seq2–seq9; the previous authorization reclaimed
only seq1 for seq9. Do not overwrite immutable slots or silently reclaim another
slot to install seq10. No permanent write-policy retention is authorized here.

The user subsequently explicitly authorized deleting only seq2 and installing
seq10 for renewed acceptance. Fresh seq9 preflight
`57dc3912e8d74318a6045cb3d699b241` and idle restoration
`b09907d52ce0459e913e076778919840` passed. The latter archived the current helper
set at `hardened-blikvm-write-restored-0315fcc232c041572435c5e2` with closed9/43.
Inventory `c2acc6ce177b4441a7d11b686c050af8` bound all eight immutable trees and
protected seq8/seq9. Native GC `8ecdaae074cd4ddeb97632a939eda04f` then deleted only
`app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53`.
Seq3–seq9 trees and complete status were unchanged; admission remained closed.
Receipt: `evidence/private/slot-reclamation-seq2-20260913-a.json`.

The harness now explicitly supports seq9-to10 publication and seq10 retry,
including the changed dev7 payload size and exact10/46/previous9 terminal
baseline. Later seq11 activation is not enabled. Full host job
`9511c9e1cf2c47fdbebcc487a2c3d85f` passed before submitting device activation.
