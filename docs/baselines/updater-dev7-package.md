# Updater dev7 host package checkpoint

Source and builder `4d47974a13eea1e563107c6611a52d520d90ccf0` order the
periodic watchdog after mandatory initial recovery and preserve specific
manager-open exit codes. Host job `4dfcaff839114509b1b4c6509be909fe` passed.

Independent bundle jobs `58edfdded9e744319f565dc975380850` and
`1fc73a7d87c54d658de4f2cd97aab079` passed. The complete archives compared equal:
`a8b1d9ec9150baf8f6f67d57a3d268fa5854c8415b13ccce58a53091077d91cc`.
Two short direct Debian package builds also compared byte-identical:
`41cef0e7c5d8839a343ba2bf6a16f255e250659510f3cdb89b7f38870202e2a1`.
The new AArch64 supervisor hash is
`120add7f7086164709d5b1f9c5be7280b060a7c86e537741aa060bc85052471b`.

The host package validator accepted this exact dev7 package and dev6 rollback
package `1634b7e42ff7ef8f10894b212bf635c6e23851d58bab5f69096705aed52cb939`.
The upgrade gate now binds the retained sequence-five application, generation
twenty-two and persistent read-only status wiring. It must not replace the
application or demand that an intentionally retained status socket be inactive.
Both package variants are checked against their own version and supervisor hash;
the application journal, immutable slots, account/TLS/trust/peer state and
persistent drop-in hashes must remain unchanged.

Device preflight, dev7-to-dev6 rollback rehearsal, retention and fresh
boot/browser acceptance remain required. The original startup failure and
manual recovery are retained in `retained-status-startup-failure.md`.
