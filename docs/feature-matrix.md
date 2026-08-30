# Feature and evidence matrix

`Runtime verified` means an authenticated read-only API reported the subsystem
online or enabled. It does not mean the BLi-KVM implementation exists or has
passed a physical interaction gate.

| Feature | Existing test baseline | BLi-KVM status | Required gate |
| --- | --- | --- | --- |
| HTTPS/login | Reachable; authenticated boundary verified | H616 TLS 1.2/1.3 Chromium setup/login, secure cookies, rotation, and logout passed | Certificate enrollment, other engines, and mobile matrix |
| Loopback server runtime | Not applicable | Unprivileged H616 server, systemd sandbox, Nginx loopback, and SSH-forward gates passed | Permanent packaging and reboot persistence |
| Read-only inventory | Five authenticated GET endpoints verified | Distinct-UID broker, dedicated-account rollback, `LoadCredential`, sanitizer, and repeated H616 inventory passed | Permanent least-privilege provisioning and reboot persistence |
| MJPEG | Streamer source online through authenticated API | Observed but not exposed | Stream bytes, reconnect, latency, soak |
| Janus WebRTC | Disabled on current PiKVM baseline | Not advertised | Deliberate adapter decision and browser matrix |
| Keyboard | HID online through authenticated API | Observed but not exposed | 10,000-event and reboot test |
| Absolute/relative mouse | HID online; modes not physically exercised | Observed but not exposed | Mode and coordinate matrix |
| ATX/LED | Enabled and idle through authenticated API | Observed but not exposed | Timing, polarity, exclusive GPIO owner |
| Virtual media | Enabled, online, and idle through authenticated API | Observed but not exposed | Upload, eject, BIOS boot, interruption |
| Fan/OLED/RTC/buttons/switch | Source evidence only | Hidden | Shadow and physical tests |
| Signed app update | Not established | Unavailable | Signature, atomicity, rollback |
| Signed system update | Not established | Unavailable | Recovery/A-B and power-cut tests |
| Automatic rollback | Not established | Unavailable | Boot-health and rollback matrix |
