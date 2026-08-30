# H616 TLS and browser checkpoint

On 2026-08-30, the commit-bound development bundle
`a5cca8ce9f973516213e7cc60f61909bce4a8cf012618758406e1985493d437f`
passed a real Chromium authentication flow through a temporary H616 TLS
reverse proxy. The bundle source commit was
`3cd926b2dca6c51b011c93577f69c89b2ca4582a`.

Nginx listened only on device loopback with an ephemeral self-signed
certificate. The host browser reached it only through a strict-host-key SSH
local forward bound to host loopback. The Rust server also remained on device
loopback, used secure cookies, and received the browser's external Host value
through the proxy for origin and CSRF validation.

Verified results:

- Nginx accepted TLS 1.2 and TLS 1.3 and its rendered configuration matched the
  packaged fixed-upstream template;
- Chromium completed first-account setup and login over HTTPS;
- the session cookie was Secure, HttpOnly, and SameSite=Strict;
- the JS-readable CSRF cookie was Secure and SameSite=Strict;
- authenticated inventory returned HTTP 200, schema v1, and backend
  `kvmd_transition` without forbidden sensitive fields;
- password rotation revoked the prior session, relogin succeeded, and logout
  cleared the session;
- thirteen browser API responses were observed with no HTTP 404;
- the temporary Nginx, Rust server, and broker stopped cleanly;
- certificate, key, tunnel, ports, and marker-protected staging were absent in
  the final check while the original KVMD services remained active;
- the root filesystem stayed in its pre-existing read-write state.

This checkpoint used one headless Chromium runtime and an ephemeral test
certificate. It does not accept certificate enrollment, permanent Nginx
installation, other browser engines, mobile layouts, reboot persistence,
recovery, video, HID, media, GPIO, physical power, or physical reset behavior.
