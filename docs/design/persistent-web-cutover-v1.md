# Persistent web cutover v1

This gate replaces only the PiKVM HTTP/TLS frontend on the designated test
H616. It does not remove or restart `kvmd.service`, uStreamer, their Unix
sockets, drivers, or hardware support. Those components remain private
transition backends for the Rust platform broker and MJPEG proxy.

## Persistent boundary

- `hardened-blikvm-server.service` remains fixed to
  `127.0.0.1:18080` with secure cookies and strict Origin handling.
- `hardened-blikvm-nginx.service` conflicts with `kvmd-nginx.service` and is
  the only Hardened process allowed to bind public TCP 80 and 443.
- Nginx runs as `www-data`, not root, with only
  `CAP_NET_BIND_SERVICE`. Its writable filesystem is limited to its systemd
  runtime directory.
- PID 1 loads the existing root-only device certificate and key through
  `LoadCredential`. Their persistent files, modes, owners, hashes, and paths
  are not changed, copied to another persistent path, or included in the
  package; only systemd's ephemeral credential view is exposed to Nginx. The
  separate non-secret Nginx configuration is root-owned, mode 0644, hash-bound,
  and read-only to the service.
- HTTP redirects to HTTPS. HTTPS permits TLS 1.2 and 1.3 and proxies only to
  the fixed loopback Rust server.
- Platform broker, video proxy, server, and Hardened Nginx are enabled. The
  control broker remains disabled and its socket remains absent.
- No administrator is created by the deployment gate. The first browser visit
  must render the branded First Setup form; the operator creates the first
  account afterward.

## Transaction and rollback

The device state is re-read immediately before mutation and must match the
private preflight receipt. Before any listener or enablement change, the gate
syncs a marker-bound rollback script, an early-boot systemd unit, and a
ten-minute transient watchdog.

The apply phase installs hash-bound config/unit candidates, validates Nginx
without binding, enables and starts the loopback Rust services, then disables
only `kvmd-nginx` and starts Hardened Nginx. Any failed phase stops the
Hardened units, disables them, removes only marker-owned config/state, restores
`kvmd-nginx` to its exact enabled/active state, and checks the original
certificate and listeners. A lost host response remains covered by the timed
and early-boot rollback paths.

Rollback is disarmed only after device-side TLS/port/service checks and a
strict-host-key SSH-forwarded Chromium check prove the account-free First
Setup screen. A new SSH session then independently rechecks the committed
state.

## Acceptance and exclusions

Acceptance requires exact 80/443 listener scope, TLS 1.2 and 1.3 health,
unchanged live certificate identity, HTTP redirect, systemd sandbox and
capability bounds, active KVMD backend, enabled Hardened services, disabled
vendor web service, no control endpoint use, no cookies, no existing account,
and no PiKVM/NanoKVM branding in the setup page.

This gate does not accept reboot persistence, resource/privacy soak, alternate
browsers, certificate rotation, diagnostic history, update/recovery, release
readiness, or physical Power/Reset behavior. Those remain separate gates.
