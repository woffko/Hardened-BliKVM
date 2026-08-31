# Runtime inventory v1

This is the historical inventory contract. The current prototype emits
[`Runtime inventory v3`](inventory-v3.md); the broker framing protocol remains
at version 1 because its request variants did not change.

`GET /api/inventory` is authenticated and read-only. The network server sends a
single `inventory` request to the local platform broker and returns only the
sanitized `RuntimeInventory` contract.

The response may contain:

- platform and backend identifiers from fixed enums;
- KVMD and streamer version strings, bounded to 64 printable characters;
- optional `enabled`, `online`, `busy`, and `connected` booleans;
- optional power-LED and Janus-enabled booleans.

It contains no endpoint, socket path, username, credential, cookie, token,
serial number, network address, media filename, or raw KVMD subtree. If the
broker is absent, unauthorized to KVMD, slow, malformed, or oversized, the HTTP
endpoint returns a generic 503 without backend details.

Successful inventory does not change `/api/capabilities`. Controls remain
unavailable until the corresponding implementation and physical gate promote
their capability separately.
