# Runtime inventory v2

`GET /api/inventory` is authenticated and read-only. The network server sends a
single `inventory` request to the local platform broker and returns only the
sanitized `RuntimeInventory` contract. Inventory schema versions are
independent from the broker framing version; the read-only broker protocol
remains v1.

Version 2 retains every v1 field and adds one required thermal observation:

```json
{
  "schemaVersion": 2,
  "platformId": "bli-h616-pikvm",
  "backend": "kvmd_transition",
  "thermal": {
    "temperatureMillidegreesCelsius": 42125
  }
}
```

`temperatureMillidegreesCelsius` is either an integer from `-40000` through
`150000`, inclusive, or `null` when the fixed platform source is unavailable,
unsafe, or invalid. It is an observation, not a fan or thermal-control API.
The H616 adapter reads only `/sys/class/thermal/thermal_zone0/temp`, refuses a
final-component symbolic link, non-regular or multiply linked sources,
group/world-writable sources, oversized input, non-ASCII integers, and values
outside the accepted range. Backend failures are logged only as coarse error
classes; neither the path nor raw input enters the public response.

The remaining response may contain:

- platform and backend identifiers from fixed enums;
- KVMD and streamer version strings, bounded to 64 printable characters;
- optional `enabled`, `online`, `busy`, and `connected` booleans;
- optional power-LED and Janus-enabled booleans.

It contains no endpoint, socket path, username, credential, cookie, token,
serial number, network address, media filename, raw KVMD subtree, writable
thermal path, fan target, PWM value, or control method. If the broker is
absent, unauthorized to KVMD, slow, malformed, or oversized, the HTTP endpoint
returns a generic 503 without backend details.

Successful inventory does not authorize a mutation. `/api/capabilities`
advertises thermal observation separately, while fan, PWM, GPIO, and all other
board controls remain unavailable until their implementation and physical gate
promote those capabilities.
