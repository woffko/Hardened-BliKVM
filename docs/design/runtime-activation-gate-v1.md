# Reversible installed-runtime activation gate design

Status: design and host implementation boundary only. The retained package,
fixed service identities, and dedicated KVMD credential are accepted on H616;
the installed product services remain disabled and inactive. This design does
not authorize persistent enablement, production Nginx/TLS activation, reboot,
update trust, diagnostic persistence, or hardware action.

## Exact native scope

The gate uses only the already installed package bytes, identities, and
root-owned KVMD credential sources. It starts exactly:

- `hardened-blikvm-platform-broker.service`;
- `hardened-blikvm-video-proxy.service`;
- `hardened-blikvm-server.service`.

The denial-only control broker remains disabled and inactive, its runtime
socket/root must remain absent, and a corresponding `.socket` unit must be
absent/inactive/disabled. No service is enabled and no Nginx file or process is
changed.

Production server configuration requires secure cookies behind an HTTPS
reverse proxy. This native gate installs one marker-owned drop-in under
`/run/systemd/system` that changes only `BLI_KVM_SECURE_COOKIES=0` for direct
`127.0.0.1:18080` testing. The fixed bind, web/state roots, broker/video
sockets, origin enforcement, identities, sandboxing, and every other unit
property remain unchanged. The drop-in is removed before acceptance and cannot
survive reboot. Production `SecureCookies=1` plus Nginx/TLS is the next gate.

## Rollback before start

Before daemon reload or service start, prepare creates one root-owned mode-0700
persistent rollback root. It generates a random test-only web password through
nonblocking kernel CSPRNG, writes a fixed rollback script, synchronizes a
marker-bound early-boot unit, and starts an independent timed watchdog.
Password, session cookie, CSRF token, account request/response, and streamed
frames remain only in this private rollback root and are never emitted to host
output, argv, environment, receipts, source, Git, or Project Memory.

The original application state root, runtime directories, drop-in, control
socket/root, listener, and activation rollback artifacts must all be absent.
The package and provisioned identity/credential inspector must pass before
prepare.

## Runtime acceptance

After the rollback paths are active, apply installs the one temporary drop-in,
reloads PID 1, and starts broker, video, then server. It verifies:

- exact User, Group, supplementary-group and loaded credential boundaries;
- private network namespaces for broker/video and host network only for the
  server;
- zero effective/bounding capabilities, `NoNewPrivileges`, closed device
  policy, fixed writable paths, and expected systemd sandbox properties;
- exact runtime-directory and Unix-socket modes/owners;
- one TCP listener only at IPv4 loopback port 18080 and no server UDP bind;
- no abstract Unix socket owned by any product process;
- control service/socket unit and runtime endpoint still absent;
- root-owned persistent KVMD credential sources remain unreadable to all
  service identities;
- first-account setup with one device-generated test administrator, login,
  authenticated inventory and current-only Diagnostics privacy/typing;
- decoded/reconnected MJPEG and stream termination after session revocation;
- no unexpected product-named artifact in `/tmp`, `/var/tmp`, or `/dev/shm`.

The application state directory may contain only the private fixed
`account.json` or one bounded `.account-*` atomic temporary. No update,
diagnostic persistence, control backend, or other state root may appear.

## Stop and cleanup

Commit revalidates the live state while rollback is still armed, then stops
server, video, and broker in that order. Every unit must become inactive and
its cgroup must contain no process. Runtime sockets/directories, server-owned
TCP/UDP/abstract sockets, and loopback `CLOSE_WAIT`/`FIN_WAIT2` entries must be
absent.

Only then may commit remove the closed test account state, private browser/API
artifacts, and temporary drop-in. It runs `systemctl daemon-reload` after the
drop-in removal and proves the original provisioned-but-inactive state through
the independent inspector. The timed watchdog and boot rollback remain armed
through these assertions and are removed last.

Rollback performs the same fixed-unit stop, empty-cgroup wait, marker-bound
state/drop-in/private-artifact deletion, and daemon reload. It never removes
the retained package, Unix identities, dedicated KVMD account/credential,
Nginx state, or unrelated files. There is no broad recursive deletion,
physical Power/Reset, reboot, shutdown, update, or hardware action.

## Acceptance and deferred work

Host acceptance requires generated POSIX shell validation, exact
start/stop/namespace/socket/resource/session/privacy allowlist tests,
BasedPyright, source/systemd boundaries, and the complete project matrix.
Native acceptance requires a private mode-0600 receipt proving runtime checks,
rollback removal, and exact provisioned-inactive final state.

Persistent service start/enablement, production Nginx/TLS, long privacy and
resource soak, reboot persistence, update trust/recovery, persistent diagnostic
history, other browser engines, physical Power/Reset, and release acceptance
remain separate gates.
