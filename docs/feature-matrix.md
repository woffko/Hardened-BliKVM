# Feature and evidence matrix

`Runtime verified` means an authenticated read-only API reported the subsystem
online or enabled. It does not mean the BLi-KVM implementation exists or has
passed a physical interaction gate.

| Feature | Existing test baseline | BLi-KVM status | Required gate |
| --- | --- | --- | --- |
| HTTPS/login | Reachable; authenticated boundary verified | H616 TLS 1.2/1.3 Chromium setup/login, secure cookies, rotation, and logout passed | Certificate enrollment, other engines, and mobile matrix |
| Loopback server runtime | Not applicable | Unprivileged H616 server, systemd sandbox, Nginx loopback, and SSH-forward gates passed | Permanent packaging and reboot persistence |
| Read-only inventory | Five authenticated GET endpoints verified | Distinct-UID broker, dedicated-account rollback, `LoadCredential`, sanitizer, and repeated H616 inventory passed | Permanent least-privilege provisioning and reboot persistence |
| MJPEG | Streamer source online through authenticated API | Read-only Rust proxy/server path passed on H616: 74 frames, 5 reconnects, 2 concurrent streams, session revocation, systemd isolation, and full rollback | TLS Chromium image decode/reconnect gate, then longer latency/soak matrix |
| Janus WebRTC | Disabled on current PiKVM baseline | Not advertised | Deliberate adapter decision and browser matrix |
| Keyboard | HID online through authenticated API | Fixed-route translator, 30-second session/target lease, fail-closed release, and crash ledger implemented but not wired | Safe target/focus, startup replay, 10,000-event, reconnect, and reboot test |
| Absolute/relative mouse | HID online; modes not physically exercised | Typed relative/absolute translation and release ledger implemented but not wired; Back/Forward rejected | Safe target/focus, mode, coordinate, scroll, button, reconnect, and reboot matrix |
| ATX/LED | Enabled, idle, and power LED=true through authenticated API | LED observation exposed read-only through sanitized inventory; all ATX actions remain unavailable | Persisted confirmation/idempotency/cooldown, timing, polarity, exclusive GPIO owner, and physical gate |
| Virtual media | Enabled, online, idle, and disconnected through authenticated API | Read-only state advertised; opaque catalog, persisted policy, and fixed set-params/connect client implemented but not wired; mutating UI requires read_write | Catalog provisioning, disconnected no-op, safe attach/eject, upload, BIOS boot, interruption, reconnect, and reboot |
| Fan/OLED/RTC/buttons/switch | Source evidence only | Hidden | Shadow and physical tests |
| Signed app update | Not established | Unavailable | Signature, atomicity, rollback |
| Signed system update | Not established | Unavailable | Recovery/A-B and power-cut tests |
| Automatic rollback | Not established | Unavailable | Boot-health and rollback matrix |
