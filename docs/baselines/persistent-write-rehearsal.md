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
