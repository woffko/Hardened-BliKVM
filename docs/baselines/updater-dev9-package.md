# Updater dev9 reproducible host package checkpoint

Source and builder `77192d35a137e8d2c9658cb24a5b637d8915736d` include
the separate upload/activation workers and inert systemd socket units.
The application source includes the authenticated activation API and explicit
GUI confirmation/reconciliation flow. The advertised write capability remains
disabled pending installed end-to-end acceptance.

The first shared-target build `23f4533dbc9544b5b5a80de05ac813c8` failed
the ELF host-path check. Its binary was not accepted or packaged. Fresh-target
builds `a870c036c3b64e05bba69e8dd09026b8` and
`8b382bfa8a1b47c89fb7e206bc46054d` passed all build and ELF checks.
Their complete archives compared byte-identical:
`4097793ec08d946071dfd871e343fd3ab66d67877017e1bf1f3e143892c467d3`.
The previous shared target was preserved.

Two separate Debian builds also compared byte-identical:
`15f744ed21f4029cc5d913fd66a98d3e029ba19be0b3c60a6fe62fb2b0bd556d`.
The AArch64 supervisor hash is
`2a524958c5e20f859213935553b5b0c02e9dc6b33e88fef066666153684e3ac0`.

This is host artifact evidence only. No dev9 package was installed by these
jobs. The native upgrade gate must bind dev7/dev9 and preserve the retained
sequence-six application, its journal, slots, configuration and persistent
read-only status wiring. Device preflight, rollback rehearsal, retention,
reboot and installed HTTP/GUI upload/activation acceptance remain required.

The profile-bound native gate now selects dev7/dev9 explicitly while keeping
legacy dev6/dev7 observers unchanged. It pins sequence six, floor six and
generation 31, preserves the retained server PID and checks that both write
sockets and their workers remain inactive. Both exact packages passed the
host driver validator. Rehearsal receipts and persistent rollback plans must
match the selected profile.

Authorized external review `7fd702504c8249e8849db29f670972a2` identified a
legacy infrastructure rollback CLI incompatibility. The shared unit generator
now adds the profile argument only for the new profile, preserving the legacy
controller command. Regression tests cover this and profile/receipt mismatch.
Full host check `ae7000f64ec04e7a9f2cb6f26d3e81ec` passed, including 539 Python
tests, Rust checks, static analysis, UI contracts, web build, lint and format
checks. This does not establish device upgrade acceptance.

Native preflight `d1209b25a3f049eebba25e034cd3372b` passed on the retained
sequence-six device. Rehearsal `ee2592c1b9f84c21a05adc65c6bfeb3c` installed
dev9, independently verified it, rolled back to dev7 and removed its guard.
Retention `fe9f25727acd4da7b49ce7d85de372df` then installed and committed dev9.
Both receipts verify unchanged serving/state and removal of guard and temporary
controller files. The device now retains updater dev9 and application sequence
six; write sockets remain inert. Fresh reboot and installed write-flow/browser
acceptance are still outstanding.
