# Virtual-media transition boundary

The virtual-media transition is split into read-only inventory, a private
opaque-ID catalog, a persisted policy journal, and a fixed KVMD client. These
components are not connected to the control broker or server, and no media
mutation capability is advertised.

## Read-only state

The authenticated H616 baseline reported KVMD MSD enabled, online, idle, and
disconnected. The manifest advertises virtual-media status only as
`read_only`. The web capability layer requires `read_write` for Image,
Download, Power, and other mutating menu items, so no media control appears.

## Opaque image catalog

Control messages contain only a non-zero numeric `imageId`. A private catalog
maps that ID to one validated ASCII basename, expected size, SHA-256, allowed
CD-ROM/removable modes, and writable policy. Names cannot contain a slash,
space, traversal, repeated dots, URL, or shell syntax. IDs and names are
unique, and writable CD-ROM is always invalid.

The catalog's SHA-256 and size are provisioning evidence; a future installer
must verify them against the actual KVMD storage image before adding an entry.
The current source provides no upload or catalog-write API.

## Persisted policy

Each mutation carries a non-zero operation ID, opaque session and confirmation
bindings, a confirmation timestamp, and a typed select/connect command. The
public eligibility constructor is disabled.

Internal policy tests establish these rules:

- confirmation expires after 30 seconds;
- offline, disabled, or busy MSD state rejects the operation;
- image selection requires the drive to be disconnected;
- setting the already-observed connection state is a no-op only when no
  indeterminate operation is pending;
- Pending is atomically persisted before a future backend call, survives
  restart, blocks every other mutation, and is never automatically retried;
- duplicate applied/rejected operations return the persisted outcome;
- conflicting operation-ID reuse is rejected;
- successful mutations impose a two-second global cooldown;
- reconciliation must use a later read-only MSD observation;
- unsafe, symlinked, malformed, or oversized journals are rejected.

## Fixed KVMD client

Only two internal KVMD route families exist:

- `/msd/set_params?image=<catalog-name>&cdrom=<0|1>&rw=<0|1>`;
- `/msd/set_connected?connected=<0|1>`.

The client has no upload, remote download, readback, remove, reset, generic
URI, raw path, or request body. Catalog resolution occurs before socket
connection, and the response must be bounded JSON with `ok=true`.

`bli-kvm-media-transition` combines the persisted policy with this client and
the catalog: a permit persists Pending, exactly one fixed route is called, and
the completion is persisted before the outcome; duplicates and conflicts replay
without another call, a no-op never touches the journal, and a busy or failed
backend response is persisted as rejected. The engine is still not connected to
the broker or server.

## Remaining gates

Before media mutation can be wired or advertised:

1. provide atomic, verified catalog provisioning and image storage quotas;
2. run an H616 disconnected-state no-op gate and a separately approved
   attach/eject test against a safe external target. The engine itself now
   passes an isolated H616 gate (mock media sockets and a temp catalog) for
   attach, eject, catalog selection, duplicate replay, backend failure with
   reconnect and restart with a Pending journal plus reconciliation;
3. add visible indeterminate/cooldown feedback;
4. verify upload interruption, read-only/writable modes, BIOS boot, reconnect,
   reboot, storage exhaustion, and recovery.

## Broker profile (Phase 5 slice)

A compile-time `media-transition-profile` build attaches the persisted media
engine to the control broker and routes the typed v2 `VirtualMedia` request to
it. Before each operation the broker observes the fixed read-only KVMD
`GET /msd` state and builds the media eligibility from it; the engine then
persists Pending, calls exactly one fixed route and persists the completion.
Shipped builds keep the profile compiled out and reject the request with
`backend_unavailable`. A dedicated `run_with_media_profile` entry point selects
the journal, the opaque-ID catalog and the KVMD socket from absolute paths, and
the KVMD credential arrives only through `LoadCredential`. The server exposes
`GET /api/virtual-media/catalog` (sanitized opaque IDs, names, sizes and allowed modes) and
`POST /api/virtual-media` (opaque image ID and a typed command) and advertises
`virtualMedia` `read_write` only when a `restricted-control-surface` build is
explicitly enabled with a configured control socket and the separate
`BLI_KVM_MEDIA_CONTROLS=1` media opt-in; the read-only catalog needs only the
`BLI_KVM_MEDIA_CATALOG` path. The storage/image GUI
uses opaque IDs. `tools/build_device_media_catalog.py` builds a catalog from
the board's real MSD images read-only; real-target attach/eject, upload,
interruption and BIOS boot remain the next gates.

## Backend seam

The media transition engine now executes through a backend seam
(`MediaExecutor`) so the KVMD executor used during migration can later be
replaced by the Rust configfs MSD worker without changing policy or broker
wiring. The KVMD constructor and all existing behavior are preserved, and a
test exercises the engine through an injected backend.

The backend seam now has a real Rust implementation: the feature-gated
`gadget-media-profile` adds `MediaExecutor::Gadget`, a configfs MSD executor
that resolves the opaque image ID through the catalog, selects the LUN file,
binds the UDC on connect and unbinds it on disconnect, and reports drive state
from `bli-kvm-usb-gadget::observe`. It is compiled out of shipped builds and
covered by host tests over a temporary configfs root.

The privileged broker exposes a feature-gated `run_with_gadget_media_profile`
that serves the media backend from the Rust configfs gadget (no KVMD): select
writes the LUN file, connect binds the UDC and disconnect unbinds it. A host
test drives it end-to-end through the control socket and asserts the gadget
state.
