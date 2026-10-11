import assert from "node:assert/strict";
import test from "node:test";

import {
  BoardClock,
  MAX_POINTS,
  PageClock,
} from "../web/src/lib/quic/board-clock.ts";
import { generator } from "./net/udp_impair_proxy.mjs";

test("before a ping there is no board time; one gives the offset, two enclose a span", () => {
  const clock = new BoardClock();
  assert.equal(clock.at(1000), null);
  assert.equal(clock.rate(), null);
  assert.equal(clock.span(), null);
  clock.sample(1000, 500_000, 2);
  assert.equal(clock.at(1000), 500_000);
  assert.equal(clock.at(1250), 500_250);
  assert.equal(clock.rate(), 1);
  // One point encloses nothing.
  assert.equal(clock.span(), null);
  assert.equal(clock.between(1000), null);
  // What is not a time is not taken.
  clock.sample(Number.NaN, 1, 1);
  clock.sample(1, Number.POSITIVE_INFINITY, 1);
  clock.sample(1, 1, -1);
  assert.equal(clock.at(1250), 500_250);

  clock.sample(1500, 500_600, 2);
  assert.deepEqual(clock.span(), { from: 1000, to: 1500 });
  assert.equal(clock.between(1000), 500_000);
  assert.equal(clock.between(1250), 500_300);
  assert.equal(clock.between(1500), 500_600);
  assert.equal(clock.between(999), null);
  assert.equal(clock.between(1501), null);
  // An answer that would turn a clock back is not one.
  clock.sample(1400, 500_700, 2);
  clock.sample(1600, 500_500, 2);
  assert.deepEqual(clock.span(), { from: 1000, to: 1500 });
});

test("the pings with the shortest round trips decide", () => {
  for (const interval of [250, 2000]) {
    const clock = new BoardClock();
    // The board is 7000 ms ahead. Slow round trips are late by half their
    // excess; the fast ones are right.
    let page = 1000;
    for (let index = 0; index < 40; index += 1) {
      page = 1000 + index * interval;
      const slow = index % 3 !== 0;
      clock.sample(page, page + 7000 + (slow ? 15 : 0), slow ? 40 : 2);
    }
    const inside = page - interval * 1.5;
    assert.ok(
      Math.abs(clock.between(inside) - (inside + 7000)) < 1e-6,
      `${clock.between(inside)} with a ping every ${interval} ms`,
    );
    const later = page + interval;
    assert.ok(Math.abs(clock.at(later) - (later + 7000)) < 1e-6);
    assert.ok(Math.abs(clock.rate() - 1) < 1e-9);
  }
  // A path that became slower for good is believed once the fast pings are
  // twenty pings old.
  const clock = new BoardClock();
  for (let index = 0; index < 5; index += 1)
    clock.sample(index * 250, index * 250, 2);
  for (let index = 5; index < 60; index += 1)
    clock.sample(index * 250, index * 250, 30);
  assert.equal(clock.span().to, 59 * 250);
});

test("a host whose clock runs slow does not skew the board's time", () => {
  // The page's clock counts 0.9 ms for every millisecond of the board.
  const board = (page) => 1_000_000 + page / 0.9;
  for (const interval of [250, 2000]) {
    const clock = new BoardClock();
    for (let page = 0; page <= 60_000; page += interval)
      clock.sample(page, board(page), 3);
    const inside = 60_000 - interval * 2.3;
    assert.ok(Math.abs(clock.between(inside) - board(inside)) < 1e-6);
    assert.ok(Math.abs(clock.rate() - 1 / 0.9) < 1e-9, `${clock.rate()}`);
    // Two seconds after the last ping the line is still right; without the
    // rate it would be off by more than 200 ms.
    assert.ok(Math.abs(clock.at(62_000) - board(62_000)) < 1e-6);
  }
  // Points within a second claim no rate.
  const dense = new BoardClock();
  for (let page = 0; page <= 750; page += 250)
    dense.sample(page, board(page), 3);
  assert.equal(dense.rate(), 1);
});

test("a host whose clock changes its rate is followed", () => {
  // A virtual machine's clock: it runs at 0.978 of the true rate, then
  // 0.970, then 0.976, changing every ten seconds. The board's clock is the
  // true one. A ping four times a second, about 3 ms there and back, the
  // two ways not equally long.
  const rates = [0.978, 0.97, 0.976, 0.972, 0.978, 0.969];
  const random = generator(11);
  // The page's clock as a function of the true time.
  const pageAt = (truth) => {
    let page = 0;
    let left = truth;
    for (const rate of rates) {
      const piece = Math.min(left, 10_000);
      page += piece * rate;
      left -= piece;
      if (left <= 0) break;
    }
    return page;
  };
  const clock = new BoardClock();
  const BOARD = 5_000_000;
  // Moments whose board time is asked for once the pings enclose them, as
  // the age of a frame is.
  const asked = [];
  let worst = 0;
  let total = 0;
  let checks = 0;
  let worstAhead = 0;
  for (let truth = 0; truth < 60_000; truth += 50) {
    if (truth % 250 === 0) {
      const out = 1.5 + random() * 0.5;
      const back = 1.5 + random() * 0.5;
      // The board answers `out` after the ping left; the page takes the
      // middle of the round trip for that moment.
      const sent = pageAt(truth);
      const heard = pageAt(truth + out + back);
      clock.sample((sent + heard) / 2, BOARD + truth + out, heard - sent);
      while (asked.length > 0) {
        const board = clock.between(pageAt(asked[0]));
        if (board === null) break;
        const error = Math.abs(board - (BOARD + asked[0]));
        worst = Math.max(worst, error);
        total += error;
        checks += 1;
        asked.shift();
      }
    }
    if (truth < 1000) continue;
    asked.push(truth + 20);
    // The line through the newest points, for a moment after them.
    worstAhead = Math.max(
      worstAhead,
      Math.abs(clock.at(pageAt(truth + 20)) - (BOARD + truth + 20)),
    );
  }
  assert.ok(checks > 1000 && asked.length < 10);
  // Between two pings the error is that of the pings themselves, a quarter
  // of a millisecond here, and of a change of rate between them. A line
  // through two minutes of pings, right for a steady clock, would be off by
  // tens of milliseconds.
  assert.ok(worst < 1, `off by ${worst.toFixed(2)} ms between pings`);
  assert.ok(total / checks < 0.25, `off by ${(total / checks).toFixed(3)} ms`);
  assert.ok(worstAhead < 5, `off by ${worstAhead.toFixed(2)} ms ahead`);
});

test("old points leave, a rate that cannot be is not believed, and reset forgets", () => {
  const clock = new BoardClock();
  for (let index = 0; index < MAX_POINTS * 2; index += 1)
    clock.sample(index * 2000, 5000 + index * 2000, 2);
  assert.deepEqual(clock.span(), {
    from: MAX_POINTS * 2000,
    to: (MAX_POINTS * 2 - 1) * 2000,
  });
  assert.equal(clock.between((MAX_POINTS - 1) * 2000), null);

  const wild = new BoardClock();
  for (let index = 0; index < 10; index += 1)
    wild.sample(index * 2000, index * 4000, 2);
  assert.equal(wild.rate(), 1);
  wild.reset();
  assert.equal(wild.at(0), null);
  assert.equal(wild.span(), null);
});

test("the page's clock is placed on the worker's by the quickest message, whatever the origins", () => {
  const clock = new PageClock();
  assert.equal(clock.toWorker(1000), null);
  // The worker's clock reads 4000 ms more than the page's at one moment
  // (their origins apart); messages take 0.3 to 5 ms.
  for (const [sent, took] of [
    [1000, 5],
    [2000, 0.3],
    [3000, 2],
  ]) {
    clock.sample(sent, sent + 4000 + took);
  }
  assert.equal(clock.toWorker(5000), 9000.3);
  // What is not a time is not taken.
  clock.sample(Number.NaN, 1);
  clock.sample(1, Number.POSITIVE_INFINITY);
  assert.equal(clock.toWorker(5000), 9000.3);
});
