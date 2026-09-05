# Notification lifecycle fix: sequence-five host checkpoint

Source and package builder `11df6b8b925605dd818ff61c401482fe494d87fb` contain
the password-reminder rejection/unmount fix. Full host job
`641c1559255c4b6793c9ac4118336a4d` passed, including six lifecycle scenarios.

Independent bundle jobs `c3bbfb9b5f1a49dabc9f11f9fb51555d` and
`924f1dc41d4047bcadc0d10131adf5c8` produced byte-identical archives:
`ddc294caf1023aa0ccd06a42ad6c19d93d0a26bfd82e03dc8fbe1f109227e6ce`.
Two short direct package builds also compared equal:
`f6fcef6a03ac014a75f4f03afa57c9fb3609c61dae59aadbe696629b49b0ced2`.

Two signing runs under the already enrolled key produced identical
8,591,360-byte `0.1.0-dev4`, sequence-five archives:
`618b1068086a04aa75f44804b58bcf0dac05d62c11553af74ebfe5cfdf893093`.
The manifest hash is
`41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc`.
Host verifier job `db06ef83298e4360ba88bc7c7247bdd3` passed with supervisor
`b067ff2a0b2dc3cb59ac1c3bd7e8c01c1c6c86dbc81ca335575e7abd35c3ad4c`,
covering signature/payload validation, all 27 immutable files, idempotent
staging, initialization, negative checks and disposable-root cleanup without
executing payload code or contacting the device.

The named transition profile binds the exact sequence-four source and
sequence-five target. Live rehearsal, activation and the previously failing
desktop/mobile/logout browser sequence remain required. The previous status
test was safely restored; failed-test cleanup
`bcf543f4dac04e2eb3370067d1d7b7d2` removed only its controller/guard files and
preserved operational state. Its browser failure remains recorded, not accepted.
