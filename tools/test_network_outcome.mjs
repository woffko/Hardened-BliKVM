import assert from "node:assert/strict";
import test from "node:test";
import {
  dnsWriteVerified,
  ipv6WriteVerified,
  resolverServers,
} from "../web/src/lib/network-outcome.ts";

test("DNS requires exact ordered read-back and explicit application", () => {
  const servers = ["1.1.1.1", "2606:4700:4700::1111"];
  const result = { applied: true, readBack: { servers } };
  assert.equal(dnsWriteVerified(result, servers), true);
  assert.equal(dnsWriteVerified(result, [...servers].reverse()), false);
  assert.equal(dnsWriteVerified(result, ["1.1.1.1"]), false);
  assert.equal(dnsWriteVerified({ ...result, applied: false }, servers), false);
  assert.equal(
    dnsWriteVerified({ ...result, rejection: "refused" }, servers),
    false,
  );
  assert.equal(dnsWriteVerified({ applied: true }, servers), false);
});

test("DNS resolver decoding rejects malformed and oversized responses", () => {
  assert.deepEqual(resolverServers({ servers: [] }), []);
  for (const value of [
    null,
    [],
    {},
    { servers: "1.1.1.1" },
    { servers: [null] },
    { servers: [123] },
    { servers: ["x".repeat(46)] },
    { servers: Array(9).fill("1.1.1.1") },
  ]) {
    assert.throws(() => resolverServers(value));
    assert.equal(
      dnsWriteVerified({ applied: true, readBack: value }, ["1.1.1.1"]),
      false,
    );
  }
});

test("IPv6 success requires explicit application and target read-back", () => {
  const outcome = {
    applied: true,
    ipv6State: { interfaces: { enp1s0: "enabled" } },
  };
  assert.equal(ipv6WriteVerified(outcome, "enp1s0", false), true);
  assert.equal(ipv6WriteVerified(outcome, "eth0", false), false);
  assert.equal(ipv6WriteVerified(outcome, "enp1s0", true), false);
  assert.equal(
    ipv6WriteVerified({ ...outcome, applied: false }, "enp1s0", false),
    false,
  );
  assert.equal(
    ipv6WriteVerified({ ...outcome, rejection: "refused" }, "enp1s0", false),
    false,
  );
});

test("IPv6 malformed or missing read-back fails closed", () => {
  for (const value of [
    null,
    [],
    {},
    true,
    { applied: true },
    { applied: "true", ipv6State: { interfaces: { eth0: "enabled" } } },
    { applied: true, ipv6State: { interfaces: null } },
    { applied: true, ipv6State: { interfaces: { eth0: "unknown" } } },
  ]) {
    assert.equal(ipv6WriteVerified(value, "eth0", false), false);
  }
});
