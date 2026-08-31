# Runtime inventory v3

`GET /api/inventory` remains authenticated and read-only. Inventory schema
versions are independent from the broker framing version; the broker protocol
still has only its v1 `inventory` request.

Version 3 retains every v2 field and adds one required external-RTC
observation:

```json
{
  "schemaVersion": 3,
  "platformId": "bli-h616-pikvm",
  "backend": "kvmd_transition",
  "thermal": {
    "temperatureMillidegreesCelsius": 42125
  },
  "rtc": {
    "unixSeconds": 1759489393,
    "offsetFromSystemSeconds": -28647995
  }
}
```

`rtc.unixSeconds` and `rtc.offsetFromSystemSeconds` are either both integers or
both `null`. Accepted Unix time is 2000-01-01 through the last second of 2099;
the absolute offset is limited to ten years. Offset direction is external RTC
minus current system clock, so a negative value means the external RTC is
behind. The value is an observation, not a synchronization request or an
accuracy claim.

The H616 adapter reads only `/sys/class/rtc/rtc1/name` and
`/sys/class/rtc/rtc1/since_epoch`. It requires exact identity
`rtc-pcf8563 0-0051`, final-component no-follow opens, safe regular-file
metadata, bounded ASCII input, a plausible epoch, and a bounded offset. It does
not open `/dev/rtc1` or a raw I2C device and has no clock-write method. Missing,
rebound, unsafe, malformed, or implausible input becomes two null fields and a
coarse local error class.

The response still contains no endpoint, credential, socket path, raw backend
tree, device path, I2C command, RTC identity, alarm, writable clock field, or
control method. Thermal, RTC, power-LED, and other observations never authorize
fan, PWM, GPIO, ATX, HID, media, system-time, or hardware-time mutation.
