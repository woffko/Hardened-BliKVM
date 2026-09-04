# Installed-runtime shadow TLS browser gate design

Status: design boundary only. Retained package, permanent identities and KVMD
credential, and reversible native installed-unit activation are accepted on
H616. This design does not authorize a persistent certificate, production
listener cutover, service enablement, reboot, update trust, diagnostic
persistence, or hardware action.

## Shadow boundary

The gate starts the already installed platform broker, MJPEG proxy, and server
without a drop-in, so the production `BLI_KVM_SECURE_COOKIES=1`, fixed
`127.0.0.1:18080` upstream, identities, credentials, and systemd sandbox remain
in force. The denial-only control service and any socket unit remain inactive
and unavailable.

A separate transient Nginx service listens only on
`127.0.0.1:18443`. Its fixed generated configuration is derived from the
committed production proxy boundary but uses only marker-owned ephemeral
certificate, key, logs, PID, and temp directories. It permits TLS 1.2 and 1.3,
forwards the external Host and HTTPS scheme, disables buffering and tickets,
and proxies only to the installed server. Existing PiKVM Nginx units,
configuration, listeners, certificates, and file-tree hash are never changed.

The transient proxy runs as the existing unprivileged web identity on the high
port with no capabilities, closed devices, strict system/home/kernel
protection, localhost-only IP policy, and a writable exception only for its
private state directory.

## Rollback before start

Preflight first runs the independent provisioned-inactive validator and
requires ports 18080/18443, application/runtime roots, shadow unit, and all gate
artifacts absent. Browser, Nginx, OpenSSL, strict SSH host-key data, CSPRNG,
KVMD, and uStreamer prerequisites must be available.

Prepare creates one marker-bound root with a one-day self-signed test
certificate, private key, exact Nginx configuration and state directory. It
persists and synchronizes an early-boot rollback unit, then starts an
independent timed watchdog before any product or proxy service.

Rollback stops the shadow proxy and the three product units, waits for empty
cgroups, removes only the bounded account state plus marker-owned TLS files and
directories, confirms runtime/listener/socket-tail cleanup, and removes the
boot/timed rollback last. It preserves the package, fixed identities, dedicated
KVMD credential, working PiKVM Nginx, and all unrelated state.

## Browser acceptance

After installed units and transient Nginx are ready, the host opens only a
strict-host-key SSH local forward to device loopback port 18443. The established
local Playwright/Chromium script uses an ephemeral browser-generated web
administrator and requires:

- HTTPS document load plus secure HttpOnly session and secure readable CSRF
  cookies with Strict SameSite;
- setup, login, password rotation, logout and revoked-session behavior;
- authenticated inventory, capabilities and current-only Diagnostics;
- desktop and 390x844 localized mobile Diagnostics layout, refresh lifecycle,
  loading/unavailable/stale/error states and sanitized data;
- decoded MJPEG, reconnect and session-bound stream termination;
- no unexpected control request, WebSocket, raw log, credential, identity,
  topology, path, or hardware action.

The browser returns only the existing bounded semantic summary. Browser
passwords, cookies, CSRF values, frames, responses, endpoint, and private device
metadata never enter the receipt or ordinary logs.

## Cleanup and acceptance

On browser success, commit revalidates TLS versions, loopback-only listeners,
Nginx sandbox, product units, control absence, KVMD/PiKVM Nginx continuity and
unchanged Nginx tree. It then stops proxy/server/video/broker, proves empty
cgroups and clean TCP tails, deletes account/TLS state, and only afterward
disarms rollback. The independent provisioned-inactive validator must pass in a
new SSH request.

Host acceptance requires exact generated configuration/rollback tests, POSIX
shell validation, existing browser-contract tests, BasedPyright,
source/systemd boundaries, and the complete project matrix. Native acceptance
requires TLS 1.2/1.3, full desktop/mobile Chromium summary, and private
mode-0600 device receipt with exact cleanup.

Persistent service start/enablement, first-account handoff, production
certificate custody and listener cutover, long privacy/resource soak, reboot
persistence, update trust/recovery, persistent diagnostic history, other
browser engines, physical Power/Reset, and release acceptance remain separate
gates.
