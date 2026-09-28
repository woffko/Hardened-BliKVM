// Probe the device WebRTC video path end to end in headless Chromium: log in
// through the API, negotiate through /api/stream/webrtc and count decoded
// video frames. WEBRTC_CODEC=h264 asks for H.264 (x264) instead of VP8. The
// GUI password is read from stdin.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, seconds = "15"] = process.argv.slice(2);
const codec = process.env.WEBRTC_CODEC ?? "vp8";
assert.ok(["vp8", "h264"].includes(codec));
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
});
const receipt = {
  schemaVersion: 1,
  probe: `${codec}-webrtc-device`,
  result: "failed",
};
try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const login = await context.request.post(`${url.origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: url.origin },
  });
  password = "";
  receipt.login = login.status();
  const page = await context.newPage();
  await page.goto(`${url.origin}/`);
  const capabilities = await page.evaluate(
    async () => (await (await fetch("/api/capabilities")).json()).data.video,
  );
  receipt.videoTransports = capabilities.transports;
  const result = await page.evaluate(
    async ([duration, codec]) => {
      const events = [];
      const query = codec === "h264" ? "?codec=h264" : "";
      const socket = new WebSocket(
        `wss://${location.host}/api/stream/webrtc${query}`,
      );
      const peer = new RTCPeerConnection({ iceServers: [] });
      const video = document.createElement("video");
      video.muted = true;
      video.autoplay = true;
      video.playsInline = true;
      document.body.append(video);
      let answered = false;
      let frames = 0;
      peer.ontrack = (event) => {
        video.srcObject = new MediaStream([event.track]);
        const count = () => {
          frames += 1;
          video.requestVideoFrameCallback(count);
        };
        video.requestVideoFrameCallback(count);
      };
      peer.onicecandidate = (event) => {
        if (!answered) return;
        socket.send(
          JSON.stringify({
            type: "candidate",
            candidate: event.candidate
              ? {
                  candidate: event.candidate.candidate,
                  sdpMid: event.candidate.sdpMid,
                  sdpMLineIndex: event.candidate.sdpMLineIndex,
                }
              : null,
          }),
        );
      };
      socket.onmessage = async (event) => {
        const message = JSON.parse(event.data);
        events.push(
          message.type === "offer" ? "offer" : JSON.stringify(message),
        );
        if (message.type !== "offer") return;
        await peer.setRemoteDescription({ type: "offer", sdp: message.sdp });
        const answer = await peer.createAnswer();
        await peer.setLocalDescription(answer);
        answered = true;
        socket.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));
      };
      await new Promise((resolve) => setTimeout(resolve, duration * 1000));
      const stats = {};
      (await peer.getStats()).forEach((report) => {
        if (report.type === "inbound-rtp" && report.kind === "video") {
          Object.assign(stats, {
            codec: report.codecId,
            framesDecoded: report.framesDecoded,
            bytesReceived: report.bytesReceived,
            frameWidth: report.frameWidth,
            frameHeight: report.frameHeight,
            keyFramesDecoded: report.keyFramesDecoded,
          });
        }
        if (report.type === "codec" && report.mimeType)
          stats.mimeType = report.mimeType;
      });
      const state = {
        ice: peer.iceConnectionState,
        connection: peer.connectionState,
      };
      socket.close();
      peer.close();
      return {
        events,
        frames,
        stats,
        state,
        width: video.videoWidth,
        height: video.videoHeight,
      };
    },
    [Number(seconds), codec],
  );
  Object.assign(receipt, result);
  receipt.result = result.stats.framesDecoded > 0 ? "passed" : "no_frames";
} finally {
  await browser.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
