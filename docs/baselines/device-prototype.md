# Temporary H616 device prototype checkpoint

On 2026-08-30, the commit-bound development bundle
`78dec2c8d3a3ac3a9b9c1de79cad2e7c5a5dbfd5f19bb26182aa23b7a9ce640b`
passed a bounded temporary run on the designated test-only AArch64 H616
device. The bundle source commit was
`83ae0da7a43775d947d927a24999898e7c4a444b`.

The runner used a strict enrolled SSH host key and a one-time encrypted test
credential delivered on stdin. It wrote only to a random marker-protected
directory below `/tmp`, invoked neither root-filesystem mode helper, installed
no service, started no platform broker, and removed the staging directory at
the end.

Verified results:

- architecture was AArch64 and `/tmp` was executable with sufficient space;
- existing KVMD and KVMD Nginx services were active before and after the run;
- the Rust server ran with a non-root UID and GID, cleared supplementary
  groups, zero effective and bounding capabilities, and `NoNewPrivs`;
- exactly one listener existed on the selected IPv4 loopback address and a
  non-loopback bind was rejected;
- health, capabilities, first-account state, and static GUI requests returned
  HTTP 200; unauthenticated inventory returned HTTP 401;
- twenty consecutive health requests succeeded;
- security headers, bundle SHA-256, payload checksums, and binary modes were
  verified;
- the process stopped cleanly, the temporary port was free, and the staging
  path was absent in the final check;
- the root filesystem remained in its pre-existing read-write state.

This checkpoint accepts only execution and HTTP policy of the unprivileged
server slice. It does not accept platform-broker credentials, authenticated
inventory on device, TLS/reverse-proxy integration, installation, update or
recovery behavior, video, HID, virtual media, GPIO, ATX, or physical buttons.
Physical power and reset tests were explicitly excluded from this session.
