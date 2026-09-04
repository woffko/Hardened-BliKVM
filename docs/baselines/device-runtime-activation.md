# Reversible installed-runtime activation checkpoint

On 2026-09-04, the retained and provisioned installed runtime passed a complete
reversible start/test/stop gate on the designated test-only H616. Platform
broker, MJPEG proxy, and loopback server ran from their package unit templates
without being enabled. The denial-only control service and any corresponding
socket unit remained inactive and unavailable. Final state returned to the
accepted provisioned-but-inactive boundary.

## Source and host acceptance

Source commit `795d2639e8f07e8c1d07772776fdf64592e24b13` added the gate. It
uses one temporary marker-owned `/run/systemd/system` drop-in to disable secure
cookies only for direct localhost HTTP testing. Production unit source remains
`SecureCookies=1` behind the separately deferred Nginx/TLS boundary.

The gate arms a synchronized early-boot rollback unit and independent timed
watchdog before daemon reload or service start. Its device-generated test web
password, account request, responses, cookie, CSRF header, and captured frames
remain inside one root-owned private rollback root and are never emitted to
host output, argv, environment, receipts, source, Git, or Project Memory.

Full Longrun job `f1ac7387abde40d58dd15349d28effd6` exited zero on that source
with 239 Python tests plus the complete locked Rust default/all-features,
feature-specific, Clippy, source/systemd, Python compilation, BasedPyright,
frontend build, ESLint, and formatting matrix.

## Fail-closed revocation refinements

Initial H616 job `05ba8c0c32aa4f39abd52fd707cb4fcf` passed service start,
isolation, inventory, Diagnostics, and two MJPEG captures, then stopped in the
broad session-revocation phase. Its remote rollback reported complete cleanup,
and independent read-only job `17b7e0a8147c490fa1897ab29e24b52a` confirmed the
exact provisioned-inactive state.

Commit `4a83f5e` split revocation into start, warmup, CSRF, logout, stream-stop,
and post-logout session phases. Full Longrun job
`51106cd86da24182938d1c6264dff605` exited zero. H616 job
`d296470e2c05417eb49635d305da71e6` then localized the rejection to logout and
again completed rollback.

The authenticated state-changing logout request carried the private CSRF value
but omitted the mandatory Origin header. Commit
`eb5d08b9f0f764a4a02164dba2d9eb88c268bfac` added Origin to the same root-only
curl config without placing either value in argv. Full Longrun job
`6c4c31dd79874fcaafac18de189967ad` exited zero on the fixed source.

## Accepted H616 runtime

Final Longrun job `28bd2fea56244731b2d621d1f73396fe` completed prepare, apply,
commit cleanup, and independent provisioned-state final validation. It proved:

- the exact package and permanent identity/credential state before start;
- broker and video ran in private network namespaces with the fixed KVMD-only
  supplementary group, zero capabilities, closed device policy, expected
  runtime directories/sockets, and systemd credential copies;
- server ran as its fixed non-KVMD identity in the host network namespace with
  exactly one listener at IPv4 `127.0.0.1:18080`;
- no product process owned a UDP or abstract Unix socket;
- all five service identities remained unable to read the persistent root-only
  KVMD credential sources;
- the control service stayed disabled/inactive, its runtime root/socket stayed
  absent, and its `.socket` unit remained not found;
- one device-generated test administrator completed first-account setup/login;
- authenticated inventory and current-only Diagnostics returned their exact
  typed schemas without credential, identity, topology, path, or raw-log data;
- two MJPEG captures decoded at least two frames each and reconnect succeeded;
- logout with cookie, private CSRF header, and exact Origin revoked the live
  stream and made the retained cookie unauthorized; and
- all marker-owned private artifacts stayed bounded.

Commit cleanup stopped server, video, then broker; proved every cgroup empty;
removed the private application account state and temporary drop-in; reloaded
PID 1; confirmed runtime roots, product-named temporary/shared-memory artifacts,
and loopback `CLOSE_WAIT`/`FIN_WAIT2` entries absent; and only then removed the
watchdog, boot rollback, and private root.

The final independent validator reconfirmed the retained package, five service
identities, shared group, dedicated KVMD authentication, root-only credential
tree, disabled/inactive units, free ports, clean dpkg audit, active KVMD, and no
rollback artifact. The private mode-0600 receipt is
`evidence/private/device-runtime-activation.json`.

## Remaining boundary

This accepts reversible native activation of the installed three-service
read-only path. Services remain disabled and inactive, application state is
absent, and the UI remains in first-account setup state on the next start.
Persistent start/enablement, production Nginx/TLS, long privacy/resource soak,
reboot persistence, update trust/recovery, persistent diagnostic history,
other browser engines, physical Power/Reset, and release acceptance remain
separate gates.
