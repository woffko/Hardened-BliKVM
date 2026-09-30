#!/usr/bin/env python3
"""A scripted OpenAI-compatible chat model for testing the PicoClaw runtime.

Serves `POST /v1/chat/completions` (plain JSON or server-sent events) and
answers every conversation the same way: first a call to the KVM screenshot
tool, then a call to the KVM actions tool that moves the mouse to the centre,
then a final text that names what the tools returned. `GET /log` returns what
each request carried (tool names, message roles, shortened tool results).

Usage: fake_openai_model.py BIND_ADDRESS PORT
It holds no credentials and accepts any API key.
"""

from __future__ import annotations

import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG: list[dict] = []
LOCK = threading.Lock()
MAX_BODY = 32 * 1024 * 1024


def tool_named(tools: list, fragment: str) -> str | None:
    for tool in tools:
        name = (tool.get("function") or {}).get("name") or tool.get("name") or ""
        if fragment in name:
            return name
    return None


def text_of(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return " ".join(
            part.get("text", "") if isinstance(part, dict) else str(part)
            for part in content
        )
    return ""


def plan(body: dict) -> dict:
    """The next assistant message for this conversation."""
    tools = body.get("tools") or []
    messages = body.get("messages") or []
    screenshot = tool_named(tools, "kvm_screenshot")
    actions = tool_named(tools, "kvm_actions")
    results = [text_of(m.get("content")) for m in messages if m.get("role") == "tool"]
    called = [
        call.get("function", {}).get("name", "")
        for m in messages
        if m.get("role") == "assistant"
        for call in (m.get("tool_calls") or [])
    ]
    if screenshot and screenshot not in called:
        return {"tool": screenshot, "arguments": {}}
    if actions and actions not in called:
        return {
            "tool": actions,
            "arguments": {"actions": [{"action": "move", "x": 0.5, "y": 0.5}]},
        }
    summary = " | ".join(result[:160] for result in results)
    return {"text": f"FAKE-DONE tools={len(results)} {summary}"}


def completion(body: dict, step: dict) -> tuple[dict, list[dict]]:
    created = int(time.time())
    model = body.get("model", "fake-model")
    if "tool" in step:
        call = {
            "id": f"call_{created}_{len(LOG)}",
            "type": "function",
            "function": {
                "name": step["tool"],
                "arguments": json.dumps(step["arguments"]),
            },
        }
        message = {"role": "assistant", "content": None, "tool_calls": [call]}
        finish = "tool_calls"
        delta = {"role": "assistant", "tool_calls": [dict(call, index=0)]}
    else:
        message = {"role": "assistant", "content": step["text"]}
        finish = "stop"
        delta = {"role": "assistant", "content": step["text"]}
    whole = {
        "id": f"chatcmpl-{created}",
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }
    chunks = [
        {
            "id": f"chatcmpl-{created}",
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": None}],
        },
        {
            "id": f"chatcmpl-{created}",
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
        },
    ]
    return whole, chunks


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args) -> None:
        return

    def send_json(self, status: int, value) -> None:
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        if self.path == "/log":
            with LOCK:
                self.send_json(200, LOG)
        elif self.path.rstrip("/").endswith("/models"):
            self.send_json(200, {"object": "list", "data": [{"id": "fake-model", "object": "model"}]})
        else:
            self.send_json(404, {"error": "not found"})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        if not self.path.rstrip("/").endswith("/chat/completions") or length > MAX_BODY:
            self.send_json(404, {"error": "not found"})
            return
        try:
            body = json.loads(self.rfile.read(length))
        except ValueError:
            self.send_json(400, {"error": "invalid JSON"})
            return
        step = plan(body)
        with LOCK:
            LOG.append(
                {
                    "at": time.time(),
                    "stream": bool(body.get("stream")),
                    "tools": [
                        (tool.get("function") or {}).get("name") or tool.get("name")
                        for tool in body.get("tools") or []
                    ],
                    "roles": [message.get("role") for message in body.get("messages") or []],
                    "tool_results": [
                        text_of(message.get("content"))[:400]
                        for message in body.get("messages") or []
                        if message.get("role") == "tool"
                    ],
                    "answer": step.get("tool") or step.get("text"),
                }
            )
        whole, chunks = completion(body, step)
        if not body.get("stream"):
            self.send_json(200, whole)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        for chunk in chunks:
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    server = ThreadingHTTPServer((sys.argv[1], int(sys.argv[2])), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
