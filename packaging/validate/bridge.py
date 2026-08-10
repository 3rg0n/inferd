#!/usr/bin/env python3
"""v0.8.0 `inferd-http` bridge gates for an install=work leg.

Run the bridge from the *archive* binary against the installed daemon:

    inferd-http --listen 127.0.0.1:8080 &
    python3 packaging/validate/bridge.py

Stdlib only -- no OpenAI SDK required, so this runs on a bare validation
host. Note the trade-off: the SDK caught two bugs (stream default,
base64) that curl-shaped checks missed, so an SDK pass is still worth
doing where the host has one.

Covers G7 on **both** `stream: false` and `stream: true`: those are
separate code paths, and the non-streaming one silently dropped
`tool_calls` from the bridge's first release until 3b75846.

Every gate asserts; exit 0 means all green.
"""
import json
import math
import os
import sys
import urllib.error
import urllib.request

BASE = os.environ.get("INFERD_HTTP_BASE", "http://127.0.0.1:8080")

WEATHER = [{
    "type": "function",
    "function": {
        "name": "get_weather",
        "description": "Get the current weather for a city.",
        "parameters": {
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        },
    },
}]

FAILURES = []


def check(label, cond, detail=""):
    ok = bool(cond)
    print(f"  {'PASS' if ok else 'FAIL'}  {label}" + (f" -- {detail}" if detail else ""))
    if not ok:
        FAILURES.append(label)
    return ok


def get(path):
    with urllib.request.urlopen(BASE + path, timeout=30) as r:
        return r.status, json.loads(r.read())


def post(path, body, timeout=180, raw=False):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
            return r.status, (data.decode() if raw else json.loads(data))
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def main():
    st, body = get("/health")
    print(f"=== /health === {st} {json.dumps(body)}")
    check("/health ok", st == 200 and body.get("status") == "ok")

    st, body = get("/v1/models")
    print(f"=== /v1/models === {st} {json.dumps(body)[:200]}")
    check("/v1/models lists a model", st == 200 and body.get("data"))

    st, body = post("/v1/chat/completions", {
        "model": "inferd",
        "messages": [{"role": "user", "content": "Reply with exactly: BRIDGE080"}],
        "max_tokens": 32,
        "temperature": 0,
    })
    print(f"\n=== chat non-stream === {st}\n{json.dumps(body)[:400]}")
    content = ""
    if st == 200:
        content = body["choices"][0]["message"].get("content") or ""
    check("non-stream chat returns the text", "BRIDGE080" in content, repr(content))
    check("non-stream finish_reason is stop",
          st == 200 and body["choices"][0].get("finish_reason") == "stop")

    st, body = post("/v1/chat/completions", {
        "model": "inferd",
        "messages": [{"role": "user", "content": "Count: 1 2 3"}],
        "max_tokens": 32,
        "stream": True,
    }, raw=True)
    lines = [line for line in body.splitlines() if line.startswith("data:")]
    print(f"\n=== chat stream === {st}  {len(lines)} data lines, "
          f"last={lines[-1][:40] if lines else 'none'}")
    check("stream chat produced chunks", len(lines) >= 2, len(lines))
    check("stream terminates with [DONE]",
          any("[DONE]" in line for line in lines))

    st, body = post("/v1/embeddings", {
        "model": "inferd",
        "input": "hello inferd",
        "dimensions": 256,
    })
    vec = body["data"][0]["embedding"] if st == 200 else []
    l2 = math.sqrt(sum(x * x for x in vec)) if vec else 0.0
    print(f"\n=== embeddings === {st}  dims={len(vec)}  L2={l2:.6f}")
    check("embeddings return 256 dims", len(vec) == 256, len(vec))
    check("embeddings are L2-normalised", abs(l2 - 1.0) < 1e-3, f"{l2:.6f}")

    # ---- G7: tool calls on BOTH paths (the 3b75846 fix) ------------------
    tool_req = {
        "model": "inferd",
        "messages": [{"role": "user", "content": "What is the weather in Dublin?"}],
        "tools": WEATHER,
        "tool_choice": "required",
        "max_tokens": 128,
        "temperature": 0,
    }

    st, body = post("/v1/chat/completions", dict(tool_req, stream=False))
    print(f"\n=== G7 tool call, stream=false === {st}\n{json.dumps(body)[:600]}")
    msg = None
    if st == 200:
        choice = body["choices"][0]
        msg = choice["message"]
        calls = msg.get("tool_calls") or []
        check("G7 non-stream carries tool_calls", len(calls) >= 1, len(calls))
        # OpenAI sends an explicit null on a tool-call-only turn and
        # clients branch on it; "" is not equivalent.
        check("G7 non-stream content is null", msg.get("content") is None,
              repr(msg.get("content")))
        check("G7 non-stream finish_reason is tool_calls",
              choice.get("finish_reason") == "tool_calls",
              choice.get("finish_reason"))
    else:
        check("G7 non-stream request succeeded", False, str(body)[:200])

    st, body = post("/v1/chat/completions", dict(tool_req, stream=True), raw=True)
    carrying = [line for line in body.splitlines()
                if line.startswith("data:") and "tool_calls" in line]
    print(f"\n=== G7 tool call, stream=true === {st}  "
          f"{len(carrying)} chunk(s) carrying tool_calls")
    if carrying:
        print(f"  {carrying[0][:300]}")
    check("G7 stream carries tool_calls", len(carrying) >= 1, len(carrying))

    # ---- G7 round trip: echo the returned message back as history -------
    # The claim that matters to a consumer: a returned choice can be
    # echoed into the next request unchanged, which is what every OpenAI
    # SDK's tool loop does.
    if msg and msg.get("tool_calls"):
        tc = msg["tool_calls"][0]
        st, body = post("/v1/chat/completions", {
            "model": "inferd",
            "messages": [
                {"role": "user", "content": "What is the weather in Dublin?"},
                msg,
                {"role": "tool", "tool_call_id": tc["id"], "content": "sunny, 21C"},
            ],
            "tools": WEATHER,
            "max_tokens": 96,
            "temperature": 0,
        })
        answer = body["choices"][0]["message"].get("content") if st == 200 else str(body)
        print(f"\n=== G7 round trip === {st}\n  {answer!r}")
        check("G7 round trip answered from the tool result",
              st == 200 and "21" in (answer or ""), repr(answer))
    else:
        check("G7 round trip ran", False, "no tool_calls to echo back")

    # ---- named-function tool_choice is rejected -------------------------
    st, body = post("/v1/chat/completions", dict(
        tool_req,
        tool_choice={"type": "function", "function": {"name": "get_weather"}},
    ))
    print(f"\n=== named-function tool_choice === {st}\n  {str(body)[:300]}")
    check("named-function tool_choice rejected with 400", st == 400, st)

    print(f"\n{'=' * 60}")
    if FAILURES:
        print(f"FAILED {len(FAILURES)} gate(s):")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    print("ALL BRIDGE GATES GREEN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
