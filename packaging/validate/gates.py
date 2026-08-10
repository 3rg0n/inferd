#!/usr/bin/env python3
"""v0.8.0 native-wire feature gates for an install=work leg.

Run against an *installed* daemon (from a release archive, at real
default paths) once `inferdctl doctor` reports ready. Covers the baseline
install=work claim plus the three new v0.8.0 surfaces:

  - `tool_choice` enforced by grammar (ADR 0029, #38)
  - `tool_choice_unsatisfied` on the `done` frame (#62)
  - unpaired `tool_result` rejected instead of guessed at (breaking)

Every gate asserts, so the exit code means something: 0 = all green.
Prompt-dependent model behaviour is checked structurally (a call was or
was not made) rather than against exact prose, except where a leg's
recorded evidence is a verbatim string.

    python3 packaging/validate/gates.py

Results go in docs/vX.Y-validation.md as a new row + section.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from wire import embed, gen, terminal, text_of, tool_uses  # noqa: E402

WEATHER = [
    {
        "name": "get_weather",
        "description": "Get the current weather for a city.",
        "input_schema": {
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        },
    }
]

FAILURES = []


def user(text):
    return {"role": "user", "content": [{"type": "text", "text": text}]}


def check(label, cond, detail=""):
    ok = bool(cond)
    print(f"  {'PASS' if ok else 'FAIL'}  {label}" + (f" -- {detail}" if detail else ""))
    if not ok:
        FAILURES.append(label)
    return ok


def show(label, frames):
    t = terminal(frames)
    calls = tool_uses(frames)
    print(f"\n=== {label} ===")
    print(f"  text: {text_of(frames)!r}")
    print(f"  tool_use count: {len(calls)}")
    for c in calls:
        print("    " + json.dumps({k: c.get(k) for k in ("tool_call_id", "name", "input")}))
    print(f"  terminal: {json.dumps(t)}")
    return t, calls


def err_of(t):
    return (t or {}).get("code") if (t or {}).get("type") == "error" else None


def main():
    # ---- baseline: the install=work claim -------------------------------
    fr = gen({
        "messages": [user("Reply with exactly: INSTALLWORK080")],
        "max_tokens": 32,
        "temperature": 0,
    })
    t, _ = show("BASELINE generate", fr)
    check("baseline generate says INSTALLWORK080",
          "INSTALLWORK080" in text_of(fr), repr(text_of(fr)))
    check("baseline backend is not mock",
          (t or {}).get("backend") not in (None, "mock"), (t or {}).get("backend"))

    er = embed({"input": ["hello inferd"], "dimensions": 256})
    vec = er.get("embeddings", [[]])[0]
    l2 = sum(x * x for x in vec) ** 0.5
    print(f"\n=== BASELINE embed ===\n  dims: {len(vec)}  L2: {l2:.6f}"
          f"  backend: {er.get('backend')}")
    check("embed returns 256 dims (MRL truncation)", len(vec) == 256, len(vec))
    check("embed vector is L2-normalised", abs(l2 - 1.0) < 1e-3, f"{l2:.6f}")

    # ---- G-A: required produces a real call -----------------------------
    _, calls = show("G-A required -> real call", gen({
        "messages": [user("What is the weather in Dublin?")],
        "tools": WEATHER,
        "tool_choice": "required",
        "max_tokens": 128,
        "temperature": 0,
    }))
    check("G-A required produced >=1 tool_use", len(calls) >= 1)
    if calls:
        check("G-A call names the declared tool",
              calls[0].get("name") == "get_weather", calls[0].get("name"))

    # ---- G-B: tool_choice_unsatisfied, adversarially --------------------
    # The grammar makes *ending the turn* without a call unreachable, but
    # unlimited non-call text is legal first -- so a model prompted against
    # the constraint declines until the budget runs out, which alone is
    # indistinguishable from ordinary truncation. The flag disambiguates.
    t, calls = show("G-B adversarial -> tool_choice_unsatisfied", gen({
        "messages": [user(
            "Do not use any tools. Do not call any function. "
            "Just reply with the single word: hi"
        )],
        "tools": WEATHER,
        "tool_choice": "required",
        "max_tokens": 64,
        "temperature": 0,
    }))
    if calls:
        # The model complied instead of refusing: the flag must stay absent.
        check("G-B a satisfied turn does not set the flag",
              not (t or {}).get("tool_choice_unsatisfied"), json.dumps(t))
    else:
        check("G-B unsatisfied required sets the flag",
              (t or {}).get("tool_choice_unsatisfied") is True, json.dumps(t))

    # ---- G-C: the breaking change, live ---------------------------------
    t, _ = show("G-C unpaired tool_result -> invalid_request", gen({
        "messages": [
            user("What is the weather in Dublin?"),
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_call_id": "call-that-never-happened",
                    "content": [{"type": "text", "text": "sunny, 21C"}],
                }],
            },
        ],
        "tools": WEATHER,
        "max_tokens": 64,
    }))
    check("G-C unpaired tool_result rejected", err_of(t) == "invalid_request", err_of(t))
    check("G-C error names the offending id",
          "call-that-never-happened" in (t or {}).get("message", ""))

    # ---- G-D..G-F: the rejection surface --------------------------------
    t, _ = show("G-D tool_choice without tools", gen({
        "messages": [user("hello")],
        "tool_choice": "required",
        "max_tokens": 16,
    }))
    check("G-D tool_choice without tools rejected",
          err_of(t) == "invalid_request", err_of(t))

    t, _ = show("G-E tool_choice + response_format", gen({
        "messages": [user("What is the weather in Dublin?")],
        "tools": WEATHER,
        "tool_choice": "required",
        # ResponseFormat is internally tagged with `schema` at the TOP
        # level (crates/inferd-proto/src/v2/request.rs) -- not the
        # OpenAI-style nested `json_schema` wrapper, which the bridge
        # translates away. The nested shape earns a misleading
        # "missing field `schema`" instead of the mutual-exclusion error.
        "response_format": {"type": "json_schema", "schema": {"type": "object"}},
        "max_tokens": 64,
    }))
    check("G-E tool_choice + response_format rejected",
          err_of(t) == "invalid_request", err_of(t))

    t, _ = show("G-F unknown tool_choice string", gen({
        "messages": [user("hello")],
        "tools": WEATHER,
        "tool_choice": "mandatory",
        "max_tokens": 16,
    }))
    check("G-F unknown tool_choice rejected",
          err_of(t) == "invalid_request", err_of(t))

    # ---- G-G: none, and that text stays reachable -----------------------
    # `none` excludes the call opener as *text*, so the thing worth
    # checking is that ordinary prose is still reachable.
    _, calls = show("G-G none (weather prompt)", gen({
        "messages": [user("What is the weather in Dublin?")],
        "tools": WEATHER,
        "tool_choice": "none",
        "max_tokens": 96,
        "temperature": 0,
    }))
    check("G-G none produced 0 calls", len(calls) == 0, len(calls))

    fr = gen({
        "messages": [user("What is 2 + 2? Reply with just the number.")],
        "tools": WEATHER,
        "tool_choice": "none",
        "max_tokens": 32,
        "temperature": 0,
    })
    show("G-G none (ordinary question)", fr)
    check("G-G text stays reachable under none", "4" in text_of(fr), repr(text_of(fr)))

    _, calls = show("G-G control: no tool_choice at all", gen({
        "messages": [user("What is the weather in Dublin?")],
        "tools": WEATHER,
        "max_tokens": 128,
        "temperature": 0,
    }))
    # Proves G-G measured the field and not the prompt.
    check("G-G control (no tool_choice) still calls", len(calls) >= 1, len(calls))

    # ---- G-H: the paired path still works -------------------------------
    fr = gen({
        "messages": [
            user("What is the weather in Dublin?"),
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "tool_call_id": "tc-1",
                    "name": "get_weather",
                    "input": {"city": "Dublin"},
                }],
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_call_id": "tc-1",
                    "content": [{"type": "text", "text": "sunny, 21C"}],
                }],
            },
        ],
        "tools": WEATHER,
        "max_tokens": 96,
        "temperature": 0,
    })
    t, _ = show("G-H paired tool_result", fr)
    # G-C must not have over-rejected: the documented shape still works.
    check("G-H paired tool_result accepted", err_of(t) is None, err_of(t))
    check("G-H answered from the tool result", "21" in text_of(fr), repr(text_of(fr)))

    print(f"\n{'=' * 60}")
    if FAILURES:
        print(f"FAILED {len(FAILURES)} gate(s):")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    print("ALL GATES GREEN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
