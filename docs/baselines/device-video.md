# H616 read-only MJPEG checkpoint

On 2026-08-30, the first Hardened BliKVM video slice passed two independent
gates on the designated test-only H616 device without invoking HID, ATX,
virtual-media, GPIO, physical Power, or physical Reset behavior.

The reversible systemd gate used bundle
`8d0e60039e6082aa79f038dbb4215acaf5b74e314cf6a1a2173baecf96f4534e`
from source commit `062f6c3f266ad5a7028f1e64098d1f971540c08a`. It ran the
existing inventory broker, the new video proxy, and the network server as
separate non-root identities. Only the broker and video proxy received the
supplementary `kvmd` group; the server had neither that group nor a KVMD
credential.

Verified systemd results:

- the upstream uStreamer socket was mode 0660 and group `kvmd`;
- the video proxy ran in a private network namespace with no capabilities,
  no device access, `NoNewPrivileges`, and a mode-0660 downstream socket;
- the authenticated Rust route returned HTTP 200 with an allowlisted
  multipart MJPEG content type and captured 74 JPEG frames;
- five sequential reconnects and two simultaneous streams completed;
- logout revoked an already-running stream and a subsequent request returned
  HTTP 401;
- twenty sanitized inventory requests continued to return HTTP 200;
- the temporary dedicated KVMD account was removed and the complete htpasswd
  file returned byte-for-byte to its original SHA-256;
- runtime units, sockets, staging, watchdog, account, and alternate port were
  absent in a separate final-state SSH check;
- KVMD and KVMD Nginx remained active and were not restarted, while the root
  filesystem stayed in its pre-existing read-write state.

The capability-selected GUI and TLS gate then used byte-identical bundle
builds with SHA-256
`1127b0cfd6a6425af258385c22a460de13fdf8fd89a3c45f1cf5760849e997ca`
from source commit `0dc0c1dc43d622d8e9360e1fdd51c19aa5dd622e`.
Chromium reached an ephemeral loopback-only TLS proxy solely through a
strict-host-key SSH local forward.

Verified browser results:

- Chromium rendered the rebranded `Remote Desktop - Hardened BliKVM` page and
  decoded the MJPEG image to a non-zero natural width;
- four MJPEG connections were observed across initial login, navigation,
  password rotation, and relogin;
- the active stream terminated within the bounded logout check;
- Secure, HttpOnly, SameSite=Strict session cookies and Secure,
  SameSite=Strict CSRF cookies remained enforced;
- inventory, password rotation, relogin, and logout passed over TLS;
- eighteen API responses contained no HTTP 404;
- no legacy control request and no WebSocket connection was attempted while
  HID, ATX, media, and writable video capabilities remained unavailable;
- the temporary certificate, key, tunnel, processes, ports, and staging were
  removed, and a separate final check again found KVMD services active.

This checkpoint accepts only authenticated read-only MJPEG exposure. It does
not accept writable video configuration, WebRTC/Janus, HDMI hotplug and
resolution switching, long-duration latency/resource limits, a 24-hour soak,
permanent installation, reboot persistence, other browser engines, mobile
layouts, recovery, or physical interaction behavior.
