# Temporary H616 broker and inventory checkpoint

On 2026-08-30, the commit-bound development bundle
`b3428d947c021d0e391330a872a00d9cb2f7fb467e061d3de14ed09dd129f181`
passed a bounded authenticated inventory run on the designated test-only H616
device. The bundle source commit was
`c3c8e8bafaec423af2c0af8cb16e5ebc0b68fc46`.

The transition broker and network server ran under distinct non-root UIDs.
Only the broker had the existing `kvmd` supplementary group. Both processes
had empty effective and bounding capability sets and `NoNewPrivs`; the server
had no supplementary groups. The broker socket was mode 0660 and shared with
the server through a non-privileged group.

The current test-only KVMD administrator credential was delivered through a
one-time secret stdin handle into a temporary mode-0700 directory with
mode-0600 files. The server could not read that directory and did not receive a
credential path or KVMD environment value. This verified process-level
credential isolation, but it is not acceptance of production account
provisioning: a distinct least-privilege KVMD account and systemd
`LoadCredential` remain separate gates.

Verified results:

- the bundle and every payload checksum and executable mode matched;
- temporary first-account setup and login returned HTTP 200;
- twenty authenticated `GET /api/inventory` requests returned HTTP 200,
  schema v1, platform `bli-h616-pikvm`, and backend `kvmd_transition`;
- the public inventory contained none of the forbidden credential, account,
  endpoint, socket, serial, token, network, or media identifier fields;
- the broker used only KVMD's five fixed read-only Unix-socket routes:
  `/info`, `/atx`, `/hid`, `/msd`, and `/streamer`;
- KVMD and KVMD Nginx remained active before and after the run;
- server and broker stopped cleanly, the loopback port was free, and the
  marker-protected `/tmp` and `/run` staging paths were absent afterward;
- the root filesystem remained in its pre-existing read-write state and no
  root-filesystem mode helper was invoked.

No HID event, ATX action, GPIO operation, media mutation, update, reboot,
physical power, or physical reset command was invoked. This checkpoint does
not accept installation, systemd sandbox execution, TLS proxying, dedicated
KVMD account provisioning, or any physical hardware behavior.
