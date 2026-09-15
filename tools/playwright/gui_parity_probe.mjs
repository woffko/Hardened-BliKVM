import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, targetUrl, outputValue, usernameFile] =
  process.argv.slice(2);
assert.ok(
  modulePath && executablePath && targetUrl && outputValue,
  "missing arguments",
);

const url = new URL(targetUrl);
assert.ok(
  ["http:", "https:", "file:"].includes(url.protocol),
  "unsupported URL scheme",
);

const outputPath = path.resolve(outputValue);
assert.equal(
  path.dirname(outputPath),
  path.resolve("evidence/private"),
  "output must be under evidence/private",
);

const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({ executablePath, headless: true });

try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  let consoleErrors = 0;
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors += 1;
    }
  });

  await page.goto(targetUrl, { waitUntil: "load", timeout: 20_000 });
  await page.waitForTimeout(500);

  if (usernameFile) {
    const username = fs.readFileSync(usernameFile, "utf8").trim();
    let password = "";
    for await (const chunk of process.stdin) {
      password += chunk.toString("utf8");
      if (password.length > 4096) {
        throw new Error("password input exceeded the limit");
      }
    }
    password = password.replace(/[\r\n]+$/, "");
    const userInput = await page.$(
      'input[type="text"], input[type="email"], input[name*="user" i]',
    );
    const passInput = await page.$('input[type="password"]');
    if (userInput && passInput && password) {
      await userInput.fill(username);
      await passInput.fill(password);
      const submit = await page.$('button[type="submit"], button');
      if (submit) {
        await submit.click();
      } else {
        await passInput.press("Enter");
      }
      await page
        .waitForLoadState("networkidle", { timeout: 20_000 })
        .catch(() => {});
      await page.waitForTimeout(800);
    }
  }

  const dom = await page.evaluate(() => {
    const labels = [];
    const seen = new Set();
    const nodes = document.querySelectorAll(
      '[role="menuitem"], [role="button"], nav a, nav button, header button, ' +
        ".ant-menu-item, .ant-dropdown-menu-item, [aria-label], [title]",
    );
    for (const node of nodes) {
      const text = (
        node.textContent ||
        node.getAttribute("aria-label") ||
        node.getAttribute("title") ||
        ""
      )
        .trim()
        .replace(/\s+/g, " ");
      if (text && text.length <= 80 && !seen.has(text)) {
        seen.add(text);
        labels.push(text);
      }
    }

    const tagCounts = {};
    for (const element of document.querySelectorAll("*")) {
      const tag = element.tagName.toLowerCase();
      tagCounts[tag] = (tagCounts[tag] || 0) + 1;
    }
    const bounded = Object.fromEntries(
      Object.entries(tagCounts)
        .sort(([left], [right]) => (left < right ? -1 : 1))
        .slice(0, 40),
    );

    return {
      title: document.title || "",
      route: `${location.pathname}${location.hash}`,
      menuLabels: labels.slice(0, 100),
      tagCounts: bounded,
    };
  });

  const receipt = {
    schemaVersion: 1,
    probe: "hardened-blikvm-gui-parity",
    target: { scheme: url.protocol.replace(":", ""), route: dom.route },
    title: dom.title,
    menuLabels: dom.menuLabels,
    tagCounts: dom.tagCounts,
    consoleErrors,
    cookiesRead: false,
    storageRead: false,
    mutationsPerformed: false,
  };
  fs.writeFileSync(outputPath, `${JSON.stringify(receipt, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  console.log(
    `gui-parity target=${url.protocol}//${url.host} route=${dom.route} labels=${receipt.menuLabels.length} consoleErrors=${consoleErrors}`,
  );
} finally {
  await browser.close();
}
