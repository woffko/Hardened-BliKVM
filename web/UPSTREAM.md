# Hardened NanoKVM web provenance

The initial `web/` tree was imported from:

- repository: `https://github.com/woffko/Hardened_NanoKVM`;
- commit: `387793ab944437bbedc534d2ba231dd88d1fd2ca`;
- source path: `web/`;
- import date: 2026-08-30;
- license: GPL-3.0-only at the source-repository level.

Before BLi-KVM changes, all 261 tracked files matched the source bytes. The
path-and-content aggregate SHA-256 on both sides was:

```text
e2e4ec413e21f04548114439aee4649ad2cab3f27a020c1fbf6be655bd3595e0
```

The unmodified imported baseline produced a successful TypeScript/Vite
production build and a production dependency audit with no known
vulnerabilities. ESLint reported 29 warnings and zero errors. Prettier reported
37 pre-existing formatting differences plus options no longer recognized by
the installed import-sorting plugin.

BLi-KVM retains upstream copyright and license notices. Subsequent changes
must not be represented as originating from the upstream project.
