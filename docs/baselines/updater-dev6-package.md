# Updater dev6 host package checkpoint

Source and builder `f22615d98ec69ff61fb49def0847df36c0ebb690` add the
read-only status socket and worker without enabling either. Full host job
`67f1bbdde50540f18db9ae57bb389951` passed.

Independent bundle jobs `7a65bbb16aa54fb0a455b2f6ee45ef24` and
`11c637dcd45741c4bbc534c7fdd77a08` passed; `cmp` verified identical archives:
`21436994ad8f57db2ddf97e37fd241288cf56bc700efb8c7339ae7c5dd8e9d1c`.

The two independently packaged `hardened-blikvm-update-runtime_0.1.0~dev6_arm64.deb`
files also compared byte-identical:
`1634b7e42ff7ef8f10894b212bf635c6e23851d58bab5f69096705aed52cb939`.
The first package job was `a0bfafb4a70c4100980d26fd79bc878b`; the second
short build ran directly with the same reviewed builder and the second archive.

The host package validator accepted both this package and the retained dev5
rollback package, including their exact file maps, manifests, modes and absence
of maintainer scripts. The dev5 package hash remains
`da88afb8c4b97d1f71318f04afa44445b1f64d76c6bf6580ecfa4200e67099f4`.

Device gate source `3a4e4bc` passed full host job
`f382df12bd454b0e9c145e9c1a4a8f88`, preflight
`7570f776df904c8dbcb43d5d21c6eab7`, guarded dev5-to-dev6-to-dev5 rollback
rehearsal `4923b65373bf40db8cab49eb13204f5e`, and retaining upgrade
`b74f97d2b6144608aadef5b170349de7`. The independent final session verified
dev6, preserved operational sequence two, unchanged serving/configuration/TLS
and application state, and removed guards/controller staging. Status transport
remains disabled without active workers. HTTP, GUI and status-socket enablement
remain subsequent gates; the signed slot-two server was not replaced.
