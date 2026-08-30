# Feature and evidence matrix

`Runtime verified` means an authenticated read-only API reported the subsystem
online or enabled. It does not mean the BLi-KVM implementation exists or has
passed a physical interaction gate.

| Feature | Existing test baseline | BLi-KVM status | Required gate |
| --- | --- | --- | --- |
| HTTPS/login | Reachable; authenticated boundary verified | Local first-account/session flow verified | TLS proxy and on-device browser matrix |
| Read-only inventory | Five authenticated GET endpoints verified | Typed KVMD client, broker IPC, sanitizer, protected HTTP endpoint | Dedicated on-device KVMD account and recovery-media run |
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
