# Persistent account and resource soak checkpoint

On 2026-09-04, the persistent Hardened BliKVM listener completed its first
operator-account and bounded resource soak gates on the designated test-only
H616. Test credentials remained encrypted in Project Memory and entered the
browser only through a one-time secret stdin handle. No credential value,
session material, endpoint response, or device identity is included here.

## Existing-account Chromium gate

Source `772658e3f732522cbb1648f264c0825e21bf2cfd` added a direct-LAN Chromium
gate that reads one password from stdin and writes only a mode-0600 bounded
receipt. Full host Longrun job `1989d28ec0e046b5a08128a6c582d8be` exited zero
with 268 Python tests and the complete locked Rust, Clippy, systemd,
source-boundary, BasedPyright, frontend, ESLint, JavaScript and formatting
matrix.

Browser job `5717bc9fad2348ec800bf7a4a94906c3` proved that First Setup was
closed, then completed login, authenticated inventory v3, current-only
Diagnostics v1, and logout over direct LAN HTTPS. It observed ten API requests
and ten responses, valid Secure/HttpOnly/SameSite session policy, a Secure
SameSite CSRF cookie, cleared the test session on logout, and observed zero
control requests and zero WebSockets. It did not change the account or retain
a browser session. The private receipt is
`evidence/private/device-existing-account-browser.json`.

## Five-minute persistent soak

Source `aa10be6f9757abb1fac6a959fb546d4a195ffb8f` added a fixed read-only
30-sample soak at ten-second intervals. Full host Longrun job
`65518261e1c544e1be71317d11d92a2e` exited zero with 272 Python tests and the
same complete matrix.

Device job `9e0d5f8f2e2848dd8fc3a12db4ae42b2` passed all 30 samples:

- every Hardened unit remained active and enabled with stable main PID and
  restart counter;
- `kvmd` remained active, vendor `kvmd-nginx` remained disabled/inactive, and
  the control service/socket remained absent;
- exact 80/443 listener scope and the HTTP redirect remained stable;
- 30 HTTPS health checks and 30 closed-setup checks passed;
- unauthenticated inventory, Diagnostics, and MJPEG each returned the expected
  401 response 30 times;
- TLS 1.2 and TLS 1.3 passed before and after the sampling window;
- observed temperature stayed between 42.101 and 49.634 degrees Celsius;
- maximum cgroup RSS was 888 KiB for the platform broker, 0 KiB reported for
  the idle video proxy, 1,860 KiB for the Rust server, and 14,672 KiB for
  split-privilege Nginx, all below their fail-closed limits;
- the root filesystem remained in its existing read-write state.

The soak made no persistent write, used no authenticated HTTP request, did not
reboot the BliKVM device, and invoked no target Power/Reset action. Its private
mode-0600 receipt is `evidence/private/device-persistent-soak.json`.

## Remaining boundary

Current-runtime authentication and a short idle/read-only soak are accepted.
Guarded software reboot persistence of the BliKVM device remains the next gate.
Longer video/input load, alternate browsers, certificate rotation,
update/recovery and release readiness remain separate. ATX Power/Reset of the
attached target remains excluded.
