import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { statusRoute, verifyStatusPanel } from "./status_api_acceptance.mjs";
import { runStatusSoak } from "./status_soak.mjs";
import { observeMjpegProgress } from "./mjpeg_progress.mjs";
import {
  createStatusOutageWindow,
  validateOutageResult,
} from "./status_outage_audit.mjs";

export async function runExistingAccountBrowser({
  browserArguments = process.argv.slice(2),
  statusOutageScenario,
  videoContinuityScenario,
} = {}) {
  assert.ok(
    statusOutageScenario === undefined ||
      typeof statusOutageScenario === "function",
  );
  assert.ok(
    videoContinuityScenario === undefined ||
      typeof videoContinuityScenario === "function",
  );
  assert.ok(!(statusOutageScenario && videoContinuityScenario));

  const [
    modulePath,
    executablePath,
    baseUrl,
    username,
    outputValue,
    runTag,
    statusMode,
    soakMode,
  ] = browserArguments;
  assert.ok(
    modulePath && executablePath && baseUrl && username && outputValue,
    "missing browser arguments",
  );
  assert.equal(new URL(baseUrl).protocol, "https:");
  assert.match(username, /^[a-z_][a-z0-9_-]{0,31}$/);
  assert.ok(
    statusMode === undefined || ["enabled", "restored"].includes(statusMode),
  );
  assert.ok(
    soakMode === undefined ||
      (soakMode === "soak-10m" && statusMode === "enabled"),
  );
  if (statusOutageScenario) {
    assert.equal(statusMode, "enabled");
    assert.equal(soakMode, undefined, "outage and soak are separate probes");
  }
  if (videoContinuityScenario) {
    assert.equal(statusMode, "enabled");
    assert.equal(soakMode, undefined);
  }
  assert.ok(browserArguments.length <= 8);
  if (runTag)
    assert.match(
      runTag,
      /^(?:hardened-blikvm-slot-serving-[a-f0-9]{24}|hardened-blikvm-application-update-[a-f0-9]{24}-(?:rehearsal|confirmed)|hardened-blikvm-status-(?:api|retain)-[a-f0-9]{24}-(?:enabled|restored))$/,
    );

  const outputPath = path.resolve(outputValue);
  const privateRoot = path.resolve("evidence/private");
  assert.equal(path.dirname(outputPath), privateRoot);

  let password = "";
  for await (const chunk of process.stdin) {
    password += chunk.toString("utf8");
    assert.ok(password.length <= 4096, "password input exceeded limit");
  }
  password = password.replace(/[\r\n]+$/, "");
  assert.ok(
    password.length >= 8 && password.length <= 256,
    "invalid password input",
  );

  function writeReceipt(value) {
    fs.mkdirSync(privateRoot, { recursive: true, mode: 0o700 });
    const temporary = `${outputPath}.part-${process.pid}`;
    fs.writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
      encoding: "utf8",
      flag: "wx",
      mode: 0o600,
    });
    fs.renameSync(temporary, outputPath);
    fs.chmodSync(outputPath, 0o600);
  }

  let browser = null;
  let phase = "launch";
  const requests = [];
  const responses = [];
  const outageWindow = createStatusOutageWindow();
  let statusOutageAcceptance = null;
  let videoContinuityAcceptance = null;
  let videoContinuityStep = null;
  let videoContinuityObservation = null;
  const videoFacts = [];
  let webSockets = 0;
  let streamResponses = 0;
  let activeStreamResponse = null;
  let inventoryVideoOnline = null;
  let inventoryVideoBusy = null;
  let logoutStatus = null;
  let statusAcceptance = null;
  let statusAfterSoak = null;
  let statusSoak = null;
  let statusPanelStep = null;
  const statusProgress = (step) => {
    statusPanelStep = step;
  };
  let pageErrorCount = 0;
  const pageErrorFacts = [];

  try {
    const { chromium } = await import(pathToFileURL(modulePath).href);
    browser = await chromium.launch({ executablePath, headless: true });
    const context = await browser.newContext({
      ignoreHTTPSErrors: true,
      locale: "en-US",
    });
    const page = await context.newPage();
    const videoProgress = await observeMjpegProgress(page);
    const pageErrors = [];
    const streamByPage = new WeakMap();
    const requestWindows = new WeakMap();
    let extraPageCreated = false;
    let extraPage = null;
    function watchPage(observedPage) {
      observedPage.on("pageerror", (error) => {
        pageErrorCount += 1;
        pageErrors.push(error.name);
        pageErrorFacts.push({
          phase,
          kind: /ResizeObserver loop/.test(error.message)
            ? "resize-observer-loop"
            : /Cannot read properties of (?:null|undefined)/.test(error.message)
              ? "null-property-access"
              : "unclassified",
          name: [
            "Error",
            "TypeError",
            "ReferenceError",
            "SyntaxError",
          ].includes(error.name)
            ? error.name
            : "other",
          locations: [
            ...(error.stack || "").matchAll(
              /\/assets\/([A-Za-z0-9_-]+\.js):(\d+):(\d+)/g,
            ),
          ]
            .slice(0, 4)
            .map((match) => ({
              asset: match[1],
              line: match[2],
              column: match[3],
            })),
        });
      });
      observedPage.on("websocket", () => {
        webSockets += 1;
      });
      observedPage.on("request", (request) => {
        requestWindows.set(request, outageWindow.captureRequest());
        const route = new URL(request.url()).pathname;
        if (route.startsWith("/api/")) {
          requests.push({ method: request.method(), route });
        }
      });
      observedPage.on("response", (response) => {
        const route = new URL(response.url()).pathname;
        if (route === "/api/stream/mjpeg" && videoFacts.length < 32) {
          videoFacts.push({
            page: observedPage === page ? "primary" : "additional",
            status: response.status(),
            step: videoContinuityStep,
          });
        }
        if (route.startsWith("/api/")) {
          const observation = {
            method: response.request().method(),
            route,
            status: response.status(),
          };
          responses.push({
            ...observation,
            expectedStatusOutage: outageWindow.allows(
              observation,
              requestWindows.get(response.request()),
            ),
          });
        }
        if (route === "/api/stream/mjpeg" && response.status() === 200) {
          streamResponses += 1;
          streamByPage.set(observedPage, response);
          if (observedPage === page) activeStreamResponse = response;
        }
      });
    }
    watchPage(page);

    phase = "login-document";
    if (statusMode) {
      assert.equal(
        (await context.request.get(`${baseUrl}${statusRoute}`)).status(),
        401,
      );
    }
    const documentResponse = await page.goto(`${baseUrl}/#/auth/login`, {
      waitUntil: "networkidle",
    });
    assert.equal(documentResponse?.status(), 200);
    assert.equal(
      documentResponse?.headers()["x-content-type-options"],
      "nosniff",
    );
    assert.equal(documentResponse?.headers()["x-frame-options"], "DENY");
    assert.equal(documentResponse?.headers()["referrer-policy"], "no-referrer");
    await page.waitForFunction(
      () => document.title === "Login - Hardened BliKVM",
    );
    assert.equal(await page.locator("input").count(), 2);

    phase = "setup-closed";
    const setup = await page.evaluate(async () => {
      const result = await fetch("/api/auth/setup", { credentials: "include" });
      return { status: result.status, body: await result.json() };
    });
    assert.equal(setup.status, 200);
    assert.equal(setup.body.code, 0);
    assert.equal(setup.body.data.required, false);

    phase = "login";
    const inputs = page.locator("input");
    await inputs.nth(0).fill(username);
    await inputs.nth(1).fill(password);
    const loginResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/auth/login" &&
        response.request().method() === "POST",
    );
    const initialStreamResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/stream/mjpeg" &&
        response.request().method() === "GET",
    );
    await page.getByRole("button", { name: "Login" }).click();
    assert.equal((await loginResponse).status(), 200);
    await page.waitForURL(`${baseUrl}/#/`);
    await page.waitForFunction(
      () => document.title === "Remote Desktop - Hardened BliKVM",
    );
    phase = "inventory-before-video";
    const inventoryBeforeVideo = await page.evaluate(async () => {
      const result = await fetch("/api/inventory", { credentials: "include" });
      return { status: result.status, body: await result.json() };
    });
    assert.equal(inventoryBeforeVideo.status, 200);
    assert.equal(inventoryBeforeVideo.body.code, 0);
    assert.equal(inventoryBeforeVideo.body.data.schemaVersion, 3);
    inventoryVideoOnline = inventoryBeforeVideo.body.data.video?.online ?? null;
    inventoryVideoBusy = inventoryBeforeVideo.body.data.video?.busy ?? null;

    phase = "video-initial-response";
    const initialStream = await initialStreamResponse;
    assert.equal(initialStream.status(), 200);
    assert.match(
      initialStream.headers()["content-type"] || "",
      /^multipart\/x-mixed-replace\s*;/i,
    );
    phase = "video-initial-decode";
    await page.waitForFunction(() => {
      const screen = document.querySelector("img#screen");
      return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
    });
    assert.ok(
      activeStreamResponse,
      "missing active MJPEG response before reload",
    );
    const initialStreamCheckpoint = videoProgress.checkpoint();
    await videoProgress.progress(initialStreamCheckpoint);

    phase = "video-reconnect-response";
    const reconnectedStreamResponse = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/stream/mjpeg" &&
        response.request().method() === "GET",
    );
    await page.reload({ waitUntil: "domcontentloaded" });
    const reconnectedStream = await reconnectedStreamResponse;
    assert.equal(reconnectedStream.status(), 200);
    assert.match(
      reconnectedStream.headers()["content-type"] || "",
      /^multipart\/x-mixed-replace\s*;/i,
    );
    phase = "video-reconnect-decode";
    await page.waitForFunction(() => {
      const screen = document.querySelector("img#screen");
      return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
    });
    phase = "video-reconnect-old-stream-release";
    await videoProgress.disconnected(initialStreamCheckpoint);

    phase = "authenticated";
    const inventory = await page.evaluate(async () => {
      const result = await fetch("/api/inventory", { credentials: "include" });
      return { status: result.status, body: await result.json() };
    });
    assert.equal(inventory.status, 200);
    assert.equal(inventory.body.code, 0);
    assert.equal(inventory.body.data.schemaVersion, 3);
    assert.equal(inventory.body.data.backend, "kvmd_transition");
    assert.doesNotMatch(
      JSON.stringify(inventory.body.data),
      /password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress/i,
    );
    const diagnostics = await page.evaluate(async () => {
      const result = await fetch("/api/diagnostics", {
        credentials: "include",
      });
      return { status: result.status, body: await result.json() };
    });
    assert.equal(diagnostics.status, 200);
    assert.equal(diagnostics.body.code, 0);
    assert.equal(diagnostics.body.data.schemaVersion, 1);
    if (statusMode) {
      phase = "status-api-desktop-mobile";
      statusAcceptance = await verifyStatusPanel(
        page,
        statusMode === "enabled",
        statusProgress,
      );
    }
    if (soakMode) {
      phase = "status-api-soak";
      statusSoak = await runStatusSoak(
        () =>
          page.evaluate(async (route) => {
            const response = await fetch(route, {
              credentials: "include",
              signal: AbortSignal.timeout(5000),
            });
            return { status: response.status, body: await response.json() };
          }, statusRoute),
        {
          progress: (value) => {
            statusSoak = value;
          },
        },
      );
      phase = "status-api-after-soak";
      statusAfterSoak = await verifyStatusPanel(page, true, statusProgress);
    }

    if (statusOutageScenario) {
      phase = "status-api-outage";
      const outcome = await statusOutageScenario({
        page,
        context,
        beginOutage: () => outageWindow.begin(),
        endOutage: () => outageWindow.end(),
        activeStream: () => activeStreamResponse,
        verifyVideoProgress: () =>
          videoProgress.progress(videoProgress.checkpoint()),
        streamFor: (observedPage) => streamByPage.get(observedPage),
        createObservedPage: async () => {
          assert.equal(
            extraPageCreated,
            false,
            "only one additional probe page is allowed",
          );
          extraPageCreated = true;
          const extra = await context.newPage();
          extraPage = extra;
          watchPage(extra);
          return extra;
        },
      });
      validateOutageResult(outcome);
      if (extraPage) {
        assert.equal(
          extraPage.isClosed(),
          true,
          "outage probe must close its extra page before returning",
        );
      }
      outageWindow.assertCompleted();
      statusOutageAcceptance = outcome;
    }

    if (videoContinuityScenario) {
      phase = "video-continuity";
      videoContinuityAcceptance = await videoContinuityScenario({
        page,
        activeStream: () => activeStreamResponse,
        verifyVideoProgress: () =>
          videoProgress.progress(videoProgress.checkpoint()),
        progress: (step) => {
          assert.ok(
            [
              "warm-viewports",
              "cold-document",
              "cold-viewports",
              "cold-close",
              "warm-return",
              "complete",
            ].includes(step),
          );
          videoContinuityStep = step;
        },
        observe: (value) => {
          assert.deepEqual(Object.keys(value).sort(), [
            "finished",
            "sameResponse",
          ]);
          assert.equal(typeof value.finished, "boolean");
          assert.equal(typeof value.sameResponse, "boolean");
          videoContinuityObservation = value;
        },
        createObservedPage: async () => {
          assert.equal(extraPageCreated, false);
          extraPageCreated = true;
          extraPage = await context.newPage();
          watchPage(extraPage);
          return extraPage;
        },
      });
      assert.deepEqual(videoContinuityAcceptance, { videoPreserved: true });
      assert.ok(extraPage?.isClosed());
    }

    phase = "cookies";
    let cookies = await context.cookies(baseUrl);
    const session = cookies.find(
      (cookie) => cookie.name === "hardened-blikvm-session",
    );
    const csrf = cookies.find(
      (cookie) => cookie.name === "hardened-blikvm-csrf",
    );
    assert.ok(session && csrf);
    assert.equal(session.httpOnly, true);
    assert.equal(session.secure, true);
    assert.equal(session.sameSite, "Strict");
    assert.equal(csrf.httpOnly, false);
    assert.equal(csrf.secure, true);
    assert.equal(csrf.sameSite, "Strict");

    phase = "logout-request";
    assert.ok(activeStreamResponse, "missing active MJPEG response");
    const activeStreamCheckpoint = videoProgress.checkpoint();
    await videoProgress.progress(activeStreamCheckpoint);
    const logout = await context.request.post(`${baseUrl}/api/auth/logout`, {
      headers: { Origin: baseUrl, "x-csrf-token": csrf.value },
    });
    logoutStatus = logout.status();
    phase = "logout-status";
    assert.equal(logoutStatus, 200);
    phase = "logout-stream-revocation";
    await videoProgress.disconnected(activeStreamCheckpoint);
    phase = "logout-cookies";
    if (statusMode) {
      assert.equal(
        (await context.request.get(`${baseUrl}${statusRoute}`)).status(),
        401,
      );
    }
    cookies = await context.cookies(baseUrl);
    assert.equal(
      cookies.some((cookie) => cookie.name === "hardened-blikvm-session"),
      false,
    );

    phase = "audit";
    assert.deepEqual(pageErrors, []);
    assert.equal(webSockets, 0);
    assert.ok(streamResponses >= 2, "MJPEG reconnect was not observed");
    assert.equal(
      requests.some((request) =>
        /^\/api\/(?:control|hid|atx|vm|msd|ws)(?:\/|$)/.test(request.route),
      ),
      false,
    );
    assert.equal(
      responses.some(
        (response) => response.status >= 500 && !response.expectedStatusOutage,
      ),
      false,
    );
    writeReceipt({
      schemaVersion: 1,
      probe: "hardened-blikvm-existing-account-browser",
      runTag: runTag || null,
      result: "passed",
      ...(statusOutageScenario ? { statusOutageAcceptance } : {}),
      ...(videoContinuityScenario
        ? {
            videoContinuityAcceptance,
            videoContinuityStep,
            videoContinuityObservation,
            videoFacts,
          }
        : {}),
      statusAcceptance,
      statusAfterSoak,
      statusSoak,
      statusPanelStep,
      transport: "direct-lan-https",
      title: "Remote Desktop - Hardened BliKVM",
      setupRequired: false,
      loginStatus: 200,
      inventoryStatus: inventory.status,
      diagnosticsStatus: diagnostics.status,
      videoStatus: initialStream.status(),
      videoDecoded: true,
      videoReconnects: streamResponses,
      streamRevoked: true,
      secureSessionCookie: true,
      secureCsrfCookie: true,
      logoutStatus: logout.status(),
      sessionCleared: true,
      apiRequests: requests.length,
      apiResponses: responses.length,
      controlRequests: 0,
      webSockets,
      videoResponses: streamResponses,
      inventoryVideoOnline,
      inventoryVideoBusy,
      legacyBrandingAbsent: true,
      secretEmitted: false,
    });
  } catch (error) {
    writeReceipt({
      schemaVersion: 1,
      probe: "hardened-blikvm-existing-account-browser",
      runTag: runTag || null,
      result: "failed",
      ...(statusOutageScenario ? { statusOutageAcceptance } : {}),
      ...(videoContinuityScenario
        ? {
            videoContinuityAcceptance,
            videoContinuityStep,
            videoContinuityObservation,
            videoFacts,
          }
        : {}),
      phase,
      statusAcceptance,
      statusAfterSoak,
      statusSoak,
      statusPanelStep,
      pageErrorCount,
      pageErrorFacts,
      serverErrors: responses
        .filter((response) => response.status >= 500)
        .map((response) => ({
          status: response.status,
          route: [
            statusRoute,
            "/api/inventory",
            "/api/diagnostics",
            "/api/stream/mjpeg",
          ].includes(response.route)
            ? response.route
            : "other-api",
        })),
      errorType: error?.name || "Error",
      apiRequests: requests.length,
      apiResponses: responses.length,
      controlRequests: requests.filter((request) =>
        request.route.startsWith("/api/control"),
      ).length,
      webSockets,
      videoResponses: streamResponses,
      inventoryVideoOnline,
      inventoryVideoBusy,
      logoutStatus,
      secretEmitted: false,
    });
    process.exitCode = 1;
  } finally {
    password = "";
    if (browser) await browser.close();
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  await runExistingAccountBrowser();
}
