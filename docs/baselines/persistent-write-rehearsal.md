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
The proposed device remedy is streaming only the application-upload route
with `proxy_request_buffering off`, preserving authentication/CSRF and body
limits. This changes the previously preserved Nginx boundary, so it has not
been applied to the device. Backed-up configuration validation/reload and a
new guarded browser acceptance are required before retention can proceed.
