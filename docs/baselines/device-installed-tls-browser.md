# Installed-runtime shadow TLS and browser checkpoint

On 2026-09-04, the retained/provisioned installed runtime passed a complete
desktop/mobile Chromium flow behind a separate unprivileged shadow Nginx on the
designated test-only H616. The existing PiKVM Nginx service, files,
certificates, listeners, and content hash were not changed.

## Source and host acceptance

Source commit `7fc49ad78c24749fbc889c5f9ab848590f28d014` added a separate
installed-runtime gate. It starts the package's disabled platform broker,
MJPEG proxy, and server units with production secure cookies unchanged, then
starts an unprivileged transient Nginx service at only
`127.0.0.1:18443`. The fixed upstream remains `127.0.0.1:18080`.

The shadow proxy uses one marker-owned one-day self-signed certificate and
key. Its systemd boundary runs as the existing unprivileged web identity with
no capabilities, closed devices, strict system/home/kernel protection,
localhost-only IP policy, one high-port bind allowlist, and one private writable
state directory. The denial-only control service/socket remain unavailable.

Full Longrun job `43e531fd7ba143e4b4399969d13f6f18` exited zero with 248
Python tests plus the complete locked Rust default/all-features,
feature-specific, Clippy, source/systemd, Python compilation, BasedPyright,
frontend build, ESLint, browser-contract, and formatting matrix.

## H616 shadow TLS and Chromium

Longrun job `08fcd876a4664a6e9e970fc056b07385` passed the exact installed
runtime. Before start, the independent provisioned-state validator confirmed
the retained package, five identities/shared group, dedicated KVMD
authentication, root-only credential tree, disabled/inactive units, absent
application/runtime/gate state, free ports, clean dpkg audit, and active KVMD.

The device phase then proved:

- the early-boot rollback and timed watchdog were active before product or
  shadow-proxy start;
- installed broker/video/server units were active while the control
  service/socket remained absent;
- the Nginx master ran unprivileged with zero capabilities and the fixed
  localhost-only listener/upstream policy;
- TLS 1.2 and TLS 1.3 health requests both succeeded;
- port 18443 had exactly one IPv4 loopback listener and no IPv6/wildcard bind;
- working PiKVM Nginx stayed active and its complete file-tree hash stayed
  unchanged.

The host exposed device loopback only through strict-host-key SSH local
forwarding. The established local Playwright/Chromium gate returned this
bounded summary:

- HTTPS title `Remote Desktop - Hardened BliKVM`;
- secure HttpOnly Strict session cookie and secure readable Strict CSRF cookie;
- first setup, login, password rotation and logout passed;
- inventory schema 3 with `kvmd_transition`, HTTP 200 and sanitized data;
- read-only video/media/power-LED/thermal/RTC capability states and unavailable
  HID/power actions;
- thermal and RTC observations stayed within accepted bounds and rendered;
- authenticated current-only Diagnostics rendered as a separate action-free
  desktop tab and responsive Traditional-Chinese mobile view;
- loading, unavailable, stale-last-good, sanitized-error, and explicit
  persistence-not-provisioned states rendered;
- MJPEG decoded and reconnected four times, with stream revocation passing;
- 38 API responses, zero control requests, and zero WebSockets were observed.

Browser-generated passwords, cookies, CSRF values, frames, responses, local
forward port, endpoint, numeric identities, and private device metadata were
not included in host output or the receipt.

## Cleanup

Commit cleanup stopped shadow Nginx, server, video and broker; proved their
cgroups empty; removed only the bounded application account state, certificate,
key, generated config, PID/log/temp directories, boot/timed rollback and marker
root; and confirmed ports 18080/18443 plus TCP tail states clean.

The independent final validator again accepted the provisioned-inactive state.
The private mode-0600 receipt is
`evidence/private/device-installed-tls-browser.json`.

## Remaining boundary

This accepts installed-runtime TLS/UI behavior only through an ephemeral
loopback shadow listener and strict SSH forward. Product services remain
disabled/inactive, application state is absent, and no production certificate
or Nginx listener is installed. Persistent first-account handoff, certificate
custody/listener cutover, service enablement, privacy/resource soak, reboot
persistence, update trust/recovery, persistent diagnostic history, other
browser engines, physical Power/Reset, and release acceptance remain separate
gates.
