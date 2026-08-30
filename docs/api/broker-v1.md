# Platform broker protocol v1

The broker protocol uses one length-prefixed JSON message per Unix-socket
connection. The four-byte unsigned big-endian length is limited to 64 KiB.
Unknown fields, unknown variants, trailing/short frames, and unsupported schema
versions are rejected.

Version 1 has exactly one request variant:

```json
{
  "schemaVersion": 1,
  "requestId": 42,
  "request": { "type": "inventory" }
}
```

The response echoes the request ID and contains either sanitized runtime
inventory or one fixed error code. It never contains a free-form backend error.

There is intentionally no generic command, path, URL, shell, GPIO, HID, ATX,
media, update, or write variant. Any future mutation requires a new typed
variant, authorization policy, idempotency/timing rules, negative tests, and a
physical acceptance gate.
