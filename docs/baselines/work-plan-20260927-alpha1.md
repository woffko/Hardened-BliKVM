# Work plan: audit fixes and release 0.1.0-alpha.1 (2026-09-27)

Follows `docs/baselines/audit-20260927.md`; item numbers (H1, M2, ...) refer
to its findings. Rules as in the earlier plans: capability-gated surfaces
enforced by the server and the board owner, privileged work only in the owner
or a dedicated helper, English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts),
`scripts/check.sh` green before every commit and release (a failure under
host load is rerun and the flaky test named in the commit that fixes it),
every change shipped as a signed slot installed through the API and verified
on the test board, audit and design documents updated with evidence under
`evidence/private`.

## Phase A: image store and virtual media

A1. (H1) The SD image builder gives p3's root `0770 root:hardened-blikvm`,
    and `/etc/fstab` no longer orders the MSD mount before
    `systemd-tmpfiles-setup.service`, so the tmpfiles rule also corrects a
    partition mounted on an existing card. A packaging test pins both.
A2. (H2) The server reconciles the catalog with the image store when it
    serves the catalog and before a mount: an entry whose file is gone is
    dropped (and logged); the builder resets the catalog to an empty one.
A3. (M2) The owner answers `image_missing` and `image_too_small` instead of
    `backend_rejected`; the server reports an unwritable image store as such;
    the image dialog waits out the cooldown between media commands instead
    of showing `cooldown_active`, and shows the reasons in English and
    Russian.
A4. (M3) An upload whose name matches the attached image is refused (409)
    until the drive is detached.
A5. Board: `media_gui_device_test.mjs` desktop and mobile; an upload over the
    attached image is refused; a stale catalog entry disappears; the remote
    download (switched on for the test, then back off) stores an ISO from a
    fixture URL served on the board and deleted afterwards.

## Phase B: owner responsiveness

B1. (H3) Network information (the DNS, IPv6 and About reads) runs on its own
    lock and a blocking worker, like the firewall and Wi-Fi, with one
    `nmcli device show` and one `connection show` per device and a result
    shared for 2 seconds; IPv4/IPv6/DNS writes take the same lock.
B2. (M4) Every `nmcli` call in the owner is bounded (killed after 20 s; 40 s
    for an activation that waits 30 s).
B3. Board: a request on the owner's shared lock stays under 100 ms at the
    90th percentile while DNS, IPv6 and About load in a loop (the probe from
    the audit, committed as a tool); the GUI walk passes on desktop and
    mobile with no API error.

## Phase C: small fixes

C1. (M1) The TLS fingerprint parser accepts OpenSSL 3's lowercase prefix.
C2. (M5) The manifest's `signedSystemUpdates` follows the RAW switch.
C3. (L1) The Wi-Fi rescan moves to `POST /api/network/wifi/scan`.
C4. (M6) The GUI parity self-test gets the same room on a loaded host as the
    other timing tests.

## Phase D: root password follows the web account (H4)

D1. Restore the parked draft (`git stash` entry `root-password-parity-draft`)
    and finish it as the reference does it: the administration helper sets
    root's password with `chpasswd` from standard input (not a switchable
    feature); First Setup and a password change call it before the account
    is stored and store nothing when it fails; passwords with line breaks are
    refused; the password page shows the reference's warning (web password
    and system root password) and the login tips name the image's default SSH
    account; English and Russian.
D2. Tests: the helper's input handling, the server sending the request
    before it stores the account and storing nothing on a refusal, the web
    texts.
D3. Board (operator): after the slot is installed, the operator changes the
    web password once in Settings > Account (root follows) and confirms that
    the agent may use the GUI record for SSH; the agent then checks root's
    SSH login and that the expiry is gone.

## Phase E: a clean SD image (H5, H2)

E1. Before fetching, the board is brought to defaults: every administration
    switch off, firewall Baseline, no attached media, the last system action
    accepted.
E2. The builder resets in the staged root: the administration switches, the
    confirmed firewall mode, the owner's gadget, media, ATX and HID journals
    and the system-action journal (each only if the services start without
    it; otherwise replaced by an empty initial state), and removes kvmd unit
    backups, old web copies and configuration backups. Assembly stops if any
    of them remains.
E3. The image records its release (`BLI_KVM_IMAGE_RELEASE=0.1.0-alpha.1`).
E4. The builder replaces the board's `resolv.conf` (the source network's name
    servers and search domain) with an empty NetworkManager file; found in the
    staged root of the first alpha build.

## Phase F: guard against unrouted web calls (M7)

F1. A test lists every `/api` call in `web/src/api` and fails on one the
    server does not route, except an explicit list of reference functions
    kept for gated controls, each with the gate that hides it.

## Phase G: GUI regression on the board

G1. `gui_walk_device.mjs` desktop and mobile with no API or page error.
G2. Media (A5), firewall page (`firewall_page_device_test.mjs status`),
    Wi-Fi (`wifi_via_api.py status scan`), VP8 (`vp8_webrtc_probe.mjs`).
G3. Safe writes set back to their values: session lock, mDNS, hostname,
    video FPS and quality, system-log test message, SSH switch off and on,
    scripts (switch on, upload, run, delete, switch off), terminal (open and
    close).

## Phase H: release 0.1.0-alpha.1

H1. Slot `0.1.0-alpha.1` built with `check.sh` green, installed through the
    API and confirmed.
H2. SD image built from the board after E1, compressed (the previous image
    was 1.16 GB compressed, under GitHub's 2 GiB asset limit), its root
    checked for the items of E2 and for Wi-Fi passphrases and private keys.
H3. GitHub: PR #1 updated; tag `v0.1.0-alpha.1`; a pre-release with the
    compressed image, `SHA256SUMS`, `IMAGE-INFO.txt`, the signed application
    bundle and English release notes (how to write the card, First Setup sets
    the web and root password, known limitations from the audit's low
    findings).
H4. Operator (recommended): write the image to a spare card and boot it once
    (First Setup, an upload and a mount) before sharing it; the collaborator
    accepts the repository invitation.

## Order and operator steps

A, B, C, D, E, F, G, H. Operator: D3 (change the web password once and allow
the GUI record for SSH), H4 (optional first boot of the image), the
collaborator's invitation.

## Phase I: image store size (H6, found after the first publish)

I1. The SD image's data partition is 8 GiB; the image store (p3) takes the
    rest of the image and the first boot grows it to the end of the card
    (`tools/image/hardened-blikvm-grow-image-store.sh`, run after the store's
    mount job, online `resize2fs`; a failure is logged and the first boot
    completes).
I2. One image may have 64 GiB: the server's upload cap, nginx's body limit
    for the upload and the remote download's limit (update runtime
    `0.1.0~dev32`).
I3. The server reports the store's free space and the limit
    (`GET /api/download/image/enabled`) and refuses a declared length beyond
    the free space and a write that runs out of it ("the image store does not
    have enough free space"); the Download dialog shows the free space,
    refuses a file that does not fit, and asks
    `POST /api/download/file/check` (name, size, free space, attached image)
    before sending. Names may have 128 characters, as in the catalog. The
    remote download's status no longer ends an upload; Cancel takes the
    upload button's place while a file is sent.
I4. (L6) The read-only autostart entry leaves Settings > Device, as in the
    reference. The IPv6 address placeholder (inherited from the reference, an address of
    a private test network) becomes a documentation address
    (`2001:db8::132`).
I5. Board: the test board's p3 grown in place; the growth script run on the
    board against a shrunken p3; an upload above 4 GiB with a long name,
    mounted as a CD-ROM and deleted; a file larger than the free space and a
    badly named file refused in the dialog without sending; Cancel during an
    upload; the GUI walk desktop and mobile.
I6. The release is rebuilt: a new slot and SD image, published again as
    `v0.1.0-alpha.1` after the first publish is deleted.
I7. (M8) At most one mouse report is in flight; movement meanwhile merges
    into one report (`web/src/lib/hid-report-queue.ts`, tested in
    `tools/test_hid_report_queue.mjs`).

I8. (L7) A session lives as long as Settings > Device > Session lock after
    (5, 15, 30 or 60 minutes), as in the reference: login issues the session
    and its cookies for that duration, and a change of the setting retimes
    the current session. Today the server issues 15-minute sessions whatever
    the setting says.

## Phase J: video smoothness (M9)

J0. The VP8 encoder runs only while a WebRTC viewer is connected (the
    signaling socket holds it; the last viewer leaving stops it). Today it
    decodes and encodes every changed frame for nobody: 2.3 of the four
    cores while the operator watched MJPEG, taken from the MJPEG relay, TLS
    and HID.
J1. MJPEG: the relay forwards only the newest frame when the viewer falls
    behind, and the send queues on the way stay small, so the picture trails
    the capture by a frame or two on any link; the default frame rate
    follows the reference (30).
J2. Frame preparation for every encoder: the capture's JPEG decoded straight
    to I420 (no RGB step), at half scale in the DCT when the encoder runs at
    half resolution; decoding, conversion and encoding on separate cores;
    VP8 with token partitions and four threads.
J3. H.264 through x264 (GPL-2.0-or-later, compatible with the project's
    GPL-3.0-only), built from a pinned source release like libvpx: preset
    `superfast` and `ultrafast`, tune `zerolatency`, constrained baseline
    profile for WebRTC, four threads, into Janus over loopback RTP
    (RFC 6184 packetization) as an "H.264 (WebRTC)" video mode beside VP8.
    Browsers decode it in hardware.
J4. Benchmark on the board, on the same recorded capture frames: VP8 and
    x264 `superfast` and `ultrafast` at 1920x1080 and 1280x720, frames per
    second, per-stage times, CPU and bitrate. The default WebRTC mode and
    resolution follow the numbers; the goal is 20 frames/s or more at
    1920x1080. If the H616 cannot, the reached rates are recorded and the
    video menu offers the lower resolution.
J5. Board: latency measured from the capture timestamp (`X-Timestamp`) on a
    full-speed viewer and one throttled to 20 Mbit/s; frame rate while the
    screen moves and CPU with and without a viewer, for MJPEG, VP8 and
    H.264, desktop and mobile.
