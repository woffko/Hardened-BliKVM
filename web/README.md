# Hardened BliKVM web interface

This React and TypeScript interface is being ported from the pinned Hardened
NanoKVM baseline documented in [`UPSTREAM.md`](UPSTREAM.md). It is not a
drop-in frontend for either upstream device.

The UI consumes `GET /api/capabilities` and fails closed when the Rust backend
does not expose an adapter. Hardware evidence alone never enables a control.

## Development

From the repository root:

```sh
corepack pnpm --dir web install --frozen-lockfile
corepack pnpm --dir web build
./scripts/check.sh
```

The Rust development server listens on `127.0.0.1:18080` by default and serves
`web/dist`. A public bind is intentionally rejected; production access belongs
behind the future hardened TLS reverse proxy.

Do not copy this development build into a NanoKVM or BliKVM field image. Device
deployment starts only after the matching platform adapter, recovery path, and
physical test gate are approved.
