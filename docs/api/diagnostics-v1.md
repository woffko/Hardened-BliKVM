# Authenticated diagnostics view v1

`GET /api/diagnostics` is an authenticated, read-only view derived from the
already sanitized runtime inventory. It uses the same opaque HttpOnly session
middleware as `/api/inventory`; unauthenticated, expired, and revoked sessions
receive the existing sanitized `401` response. Other methods are not routed.

The network server performs one ordinary read-only inventory request through
the existing Platform Broker client. It has no diagnostic credential, key,
store, floor, collector-export client, acknowledgement, process, service, or
hardware authority.

## Response

The successful response uses the normal API envelope. Its `data` field is:

```json
{
  "schemaVersion": 1,
  "availability": "current_only",
  "current": {
    "temperature": {
      "state": "available",
      "millidegreesCelsius": 42125
    },
    "rtc": {
      "state": "available",
      "unixSeconds": 1759489393,
      "offsetFromSystemSeconds": -28647995
    },
    "components": {
      "video": {
        "enabled": "active",
        "online": "active",
        "busy": "inactive",
        "connected": "unavailable"
      },
      "hid": {
        "enabled": "unavailable",
        "online": "active",
        "busy": "unavailable",
        "connected": "unavailable"
      },
      "power": {
        "enabled": "active",
        "online": "active",
        "busy": "unavailable",
        "connected": "unavailable"
      },
      "virtualMedia": {
        "enabled": "active",
        "online": "active",
        "busy": "inactive",
        "connected": "inactive"
      },
      "powerLed": "active"
    }
  },
  "history": {
    "state": "unavailable",
    "reason": "persistence_not_provisioned"
  }
}
```

Every optional inventory boolean becomes exactly `active`, `inactive`, or
`unavailable`. Temperature and RTC values are present only with `available`;
missing observations carry `unavailable` and null values. Invalid or
incoherent inventory fails as the existing generic `503 service unavailable`
without backend detail.

## Privacy and deferred history

The view deliberately omits platform ID, software versions, diagnostic bundle
text, collector instance/window bindings, omission counters, paths, account or
numeric identities, device identifiers, addresses, topology, credentials,
keys, nonces, raw I/O errors, and vendor logs.

`history` is fixed to `unavailable / persistence_not_provisioned`. The endpoint
does not read the unshipped persistence stack or acknowledge a collector.
Production history cannot become available until a separate key-custody,
trusted-floor, daemon/unit, retention, privacy, reboot, and release gate is
accepted.
