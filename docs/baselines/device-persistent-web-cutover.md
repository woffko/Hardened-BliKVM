# Persistent Hardened web cutover checkpoint

On 2026-09-04, the designated test-only H616 completed the persistent web
listener replacement from PiKVM Nginx to Hardened BliKVM. The accepted source
head was `4185456acff47aac70e86e6eb614383b8db3e363`; full host Longrun job
`b53e906b1b7e4f9b8c3f9fdd0f5bf02e` passed 266 Python tests and the complete
locked Rust, Clippy, systemd, source-boundary, BasedPyright, frontend, ESLint,
JavaScript, and formatting matrix.

## Guarded H616 cutover

Device Longrun job `78b325ef16e140c4bc1f20b9b4bc50fd` began from the
independently verified provisioned-inactive state. Before changing a listener
or enablement link, it synchronized a marker-bound early-boot rollback unit and
a ten-minute transient watchdog. It then validated the hash-bound Nginx config
without binding, started the installed loopback Rust broker/video/server, and
replaced only `kvmd-nginx` on public TCP 80 and 443.

The committed state proves:

- `kvmd-nginx.service` is disabled and inactive but remains installed for
  recovery;
- `kvmd.service` and uStreamer remain active only as transition backends for
  fixed Unix-socket clients;
- the platform broker, MJPEG proxy, Rust server, and Hardened Nginx units are
  enabled and active;
- the Rust server has only its fixed `127.0.0.1:18080` listener, while Hardened
  Nginx has the exact wildcard IPv4/IPv6 listeners on 80 and 443;
- the existing root-owned device certificate/key pair was reused without a
  path, owner, mode, or file-hash change, and the live leaf certificate matches
  the source certificate in DER form;
- TLS 1.2 and TLS 1.3 health requests pass and HTTP redirects to HTTPS;
- the Nginx master retains only the bounded low-port and worker-transition
  capabilities, its request worker is `www-data` with zero effective
  capabilities, and writable access is limited to the systemd runtime tree;
- the denial-only control service/socket remain disabled and absent;
- no physical Power or Reset action was invoked.

The Chromium gate used a strict-host-key SSH local forward only for the guarded
commit decision. It rendered `First Setup - Hardened BliKVM`, confirmed the
three-field account form, observed three API GET requests, zero cookies, zero
control requests, zero WebSockets, and no PiKVM/NanoKVM branding. It deliberately
did not create an administrator. After that proof, a new SSH session rechecked
the persistent state and only then removed both rollback mechanisms.

A separate direct-LAN Chromium run subsequently passed the same account-free
First Setup contract without the SSH tunnel. Its private mode-0600 summary is
`evidence/private/device-web-cutover-lan-browser.json`; the complete cutover
receipt is `evidence/private/device-web-cutover.json`.

## Fail-closed refinements

Earlier attempts stopped before commit and restored the exact PiKVM baseline.
They identified and corrected alternate-config discovery, isolated validation
runtime paths, credential/config readability, worker-directory ownership,
PEM-versus-DER certificate comparison, and duplicate proxy security headers.
Independent read-only recovery inspections confirmed the original listener,
certificate, disabled Hardened units, absent app state, and absent staging after
each class of failure. The accepted run itself records no cleanup attempt
because its browser and independent final gates passed.

## Remaining boundary

This accepts the persistent current-runtime replacement and direct LAN access.
The first administrator must still be created by the operator. Guarded reboot
persistence, resource/privacy soak, certificate rotation, alternate browsers,
update/recovery, full hardware functions, and release acceptance remain
separate gates. Physical Power/Reset testing remains excluded.
