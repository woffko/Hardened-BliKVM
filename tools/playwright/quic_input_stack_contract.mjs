// Input over QUIC through everything that is the project's own on the way: a
// page of the server's origin, the real gateway, the real web server, and a
// stand-in for the board owner (quic_stack_rig.mjs). The requests of the HID
// lease API reach the owner in the order the page sent them, no keyboard
// report is lost, pointer positions that wait are merged, a session that
// ends gives its lease back, a request without a confirmation of the target
// is refused in the words HTTP gives, and the standard path works as before.
import assert from "node:assert/strict";
import { launch, until } from "./quic_fixture.mjs";
import { installClient, startStack } from "./quic_stack_rig.mjs";

const browser = await launch();
// The owner takes 20 ms over a report, so requests wait behind one another.
const stack = await startStack({ hidMs: 20 });
const origin = `http://127.0.0.1:${stack.port}`;
const LEASE = "55".repeat(16);

async function openPage() {
  const context = await browser.newContext();
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  await page.goto(`${origin}/`, { waitUntil: "load" });
  await page.evaluate(installClient);
  assert.equal(await page.evaluate(() => window.kvm.login()), 200);
  return { context, page };
}
const leases = () => stack.owner.leases();
const reports = (from = 0) =>
  leases()
    .slice(from)
    .filter((request) => request.op === "report")
    .map((request) => request.report);

try {
  // The lease over a session: acquire, key reports one by one, release.
  {
    const { context, page } = await openPage();
    assert.equal(await page.evaluate(() => window.kvm.confirm()), true);
    const answers = await page.evaluate(async () => {
      const kvm = window.kvm;
      const session = await kvm.open();
      window.session = session;
      const acquired = await session.ask({
        action: "acquire",
        operationId: kvm.operation(),
        ttlSeconds: 30,
      });
      const reported = [];
      for (let code = 4; code < 24; code += 1) {
        reported.push(
          await session.ask({
            action: "report",
            operationId: kvm.operation(),
            leaseId: acquired.body.data.leaseId,
            report: { mode: "keyboard", data: [0, 0, code, 0, 0, 0, 0, 0] },
          }),
        );
      }
      return { acquired, reported };
    });
    assert.deepEqual(answers.acquired, {
      status: 200,
      body: {
        code: 0,
        msg: "success",
        data: { applied: true, leaseId: LEASE, ttlSeconds: 30 },
      },
    });
    assert.ok(
      answers.reported.every(
        (answer) => answer.status === 200 && answer.body.data.applied === true,
      ),
    );
    assert.equal(leases()[0].op, "acquire");
    assert.deepEqual(
      reports().map((report) => report.data[2]),
      Array.from({ length: 20 }, (_, index) => 4 + index),
    );

    // Sixty key reports written at once: every one arrives, in order.
    const before = leases().length;
    const burst = await page.evaluate(async (lease) => {
      const kvm = window.kvm;
      const answers = await window.session.askAll(
        Array.from({ length: 60 }, (_, index) => ({
          action: "report",
          operationId: kvm.operation(),
          leaseId: lease,
          report: {
            mode: "keyboard",
            data: [0, 0, 30 + (index % 9), 0, 0, 0, 0, 0],
          },
        })),
      );
      return answers.map((answer) => answer.body?.data?.applied === true);
    }, LEASE);
    assert.deepEqual(burst, Array(60).fill(true));
    assert.deepEqual(
      reports(before).map((report) => report.data[2]),
      Array.from({ length: 60 }, (_, index) => 30 + (index % 9)),
    );

    // A key, twenty positions and a key at once: the keys keep their place,
    // the positions that waited give way to the newest, and every request is
    // answered.
    const mixedFrom = leases().length;
    const mixed = await page.evaluate(async (lease) => {
      const kvm = window.kvm;
      const key = (code) => ({
        action: "report",
        operationId: kvm.operation(),
        leaseId: lease,
        report: { mode: "keyboard", data: [0, 0, code, 0, 0, 0, 0, 0] },
      });
      const position = (x) => ({
        action: "report",
        operationId: kvm.operation(),
        leaseId: lease,
        report: {
          mode: "absolute_mouse",
          data: [0, x & 0xff, x >> 8, 9, 0, 0],
        },
      });
      const answers = await window.session.askAll([
        key(4),
        ...Array.from({ length: 20 }, (_, index) => position(1000 + index)),
        key(5),
      ]);
      return answers.map((answer) => answer.body?.data?.applied === true);
    }, LEASE);
    assert.deepEqual(mixed, Array(22).fill(true));
    const seen = reports(mixedFrom);
    assert.deepEqual(seen[0], {
      mode: "keyboard",
      data: [0, 0, 4, 0, 0, 0, 0, 0],
    });
    assert.deepEqual(seen.at(-1), {
      mode: "keyboard",
      data: [0, 0, 5, 0, 0, 0, 0, 0],
    });
    const positions = seen
      .slice(1, -1)
      .map((report) => report.data[1] | (report.data[2] << 8));
    assert.ok(
      positions.length >= 1 && positions.length < 20,
      `${positions.length} positions reached the owner`,
    );
    assert.equal(positions.at(-1), 1019, "the newest position was not sent");
    assert.deepEqual(
      positions,
      [...positions].sort((a, b) => a - b),
    );

    // The session ends without a release: the lease is given back for it.
    const releases = () =>
      leases().filter((request) => request.op === "release").length;
    assert.equal(releases(), 0);
    await page.evaluate(() => window.session.close());
    await until("the lease is given back", () => releases() === 1);

    // The standard path, for the same login: as before. A lease that was
    // given back for a session that vanished took the confirmation of the
    // target with it, so the operator confirms again, as the page does.
    const unconfirmed = await page.evaluate(() =>
      window.kvm.https({
        action: "acquire",
        operationId: window.kvm.operation(),
        ttlSeconds: 30,
      }),
    );
    assert.equal(
      unconfirmed.body.msg,
      "a current focus confirmation is required",
    );
    assert.equal(await page.evaluate(() => window.kvm.confirm()), true);
    const standard = await page.evaluate(async () => {
      const kvm = window.kvm;
      const acquired = await kvm.https({
        action: "acquire",
        operationId: kvm.operation(),
        ttlSeconds: 30,
      });
      const lease = acquired.body.data.leaseId;
      const reported = await kvm.https({
        action: "report",
        operationId: kvm.operation(),
        leaseId: lease,
        report: { mode: "keyboard", data: [0, 0, 7, 0, 0, 0, 0, 0] },
      });
      const released = await kvm.https({
        action: "release",
        operationId: kvm.operation(),
        leaseId: lease,
      });
      return { acquired, reported, released };
    });
    assert.equal(standard.acquired.body.data.leaseId, LEASE);
    assert.equal(standard.reported.body.data.applied, true);
    assert.equal(standard.released.body.data.applied, true);
    assert.equal(releases(), 2);
    await context.close();
  }

  // Without a confirmation of the target: the words HTTP gives, over both,
  // and nothing reaches the owner.
  {
    const before = leases().length;
    const { context, page } = await openPage();
    const refused = await page.evaluate(async () => {
      const kvm = window.kvm;
      const session = await kvm.open();
      const requests = [
        { action: "acquire", operationId: kvm.operation(), ttlSeconds: 30 },
        {
          action: "report",
          operationId: kvm.operation(),
          leaseId: "55".repeat(16),
          report: { mode: "keyboard", data: [0, 0, 4, 0, 0, 0, 0, 0] },
        },
      ];
      const answers = [];
      for (const request of requests) {
        answers.push({
          quic: await session.ask(request),
          https: await kvm.https(request),
        });
      }
      session.close();
      return answers;
    });
    for (const { quic, https } of refused) {
      assert.deepEqual(quic, https);
      assert.deepEqual(quic, {
        status: 200,
        body: { code: -1, msg: "a current focus confirmation is required" },
      });
    }
    assert.equal(leases().length, before, "the owner was asked");
    await context.close();
  }

  // The gateway stops while a session holds the lease: it is given back.
  {
    const { context, page } = await openPage();
    assert.equal(await page.evaluate(() => window.kvm.confirm()), true);
    const before = leases().length;
    await page.evaluate(async () => {
      const kvm = window.kvm;
      const session = await kvm.open();
      await session.ask({
        action: "acquire",
        operationId: kvm.operation(),
        ttlSeconds: 30,
      });
    });
    assert.deepEqual(
      leases()
        .slice(before)
        .map((request) => request.op),
      ["acquire"],
    );
    await stack.stopGateway();
    await until(
      "the lease is given back",
      () => leases().at(-1).op === "release",
    );
    await context.close();
  }

  console.log("quic input stack contract: ok");
} catch (error) {
  console.error(stack.logs());
  throw error;
} finally {
  await browser.close();
  await stack.close();
}
