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
