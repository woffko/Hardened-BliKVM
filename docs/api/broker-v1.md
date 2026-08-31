# Platform broker protocol v1

The broker protocol uses one length-prefixed JSON message per Unix-socket
connection. The four-byte unsigned big-endian length is limited to 64 KiB.
Unknown fields, unknown variants, trailing/short frames, and unsupported schema
versions are rejected.

Version 1 has exactly one request variant in every default build:

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

## Unshipped diagnostic schema v2

The default-off `diagnostic-export` feature adds a separate local-only schema
version `2`; it does not extend version `1`. Its only request is:

```json
{
  "schemaVersion": 2,
  "requestId": 71,
  "request": { "type": "diagnostics" }
}
```

Only a connection whose kernel credentials map to the exact non-root Server
role may receive a response. All six fixed service UIDs must be nonzero and
distinct, the shared GID must match, the serving process must map to Platform
Broker, and the peer PID must be present and positive. A wrong role receives
only `unauthorized`; missing collector state is a typed backend-unavailable
error.

A successful response contains either no sealed snapshot or one nonzero
instance/window binding, omission count, record count, and canonical typed
bundle. Decoding re-parses the closed bundle grammar and re-encodes it
byte-for-byte. `Debug` omits the bundle. There is no acknowledgement request:
schema v2 is read-only until a later durable write-before-ack coordinator
exists. Version/type cross-pairs such as v1 diagnostics or v2 inventory fail.

This schema is absent from the default broker protocol/client and shipped
Platform Broker binary. It has no server call site, HTTP route, UI, package,
capability, persistence, retry loop, or service dependency. Native evidence is
in `../baselines/device-diagnostic-broker.md`.
