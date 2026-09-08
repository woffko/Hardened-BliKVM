import assert from "node:assert/strict";
import net from "node:net";
import { runExistingAccountBrowser } from "./device_existing_account_smoke.mjs";
import { createStatusOutageScenario } from "./status_outage_scenario.mjs";
import { StatusOutageControl } from "./status_outage_control.mjs";

const [fdValue, ...browserArguments] = process.argv.slice(2);
assert.match(fdValue || "", /^[0-9]{1,6}$/);
const fd = Number(fdValue);
assert.ok(fd >= 3);
const control = new StatusOutageControl(
  new net.Socket({ fd, readable: true, writable: true }),
);
try {
  await runExistingAccountBrowser({
    browserArguments,
    statusOutageScenario: createStatusOutageScenario((phase) =>
      control.request(phase),
    ),
  });
} finally {
  control.close();
}
