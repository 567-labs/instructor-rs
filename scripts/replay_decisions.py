"""Native request/response samples for the ticket in tests/llm/test_decisions.

The ordered questions and input come from the paired Python test. The
synthetic responses exercise score scaling, probability vectors and usage.
"""

import json
from pathlib import Path


def decision_response(provider, body):
    if provider not in ("openai", "openrouter", "typesafe"):
        raise ValueError("unsupported Decisions route")
    protocol = "openai_decisions" if provider == "openai" else "jev"
    path = Path(__file__).parent / "fixtures" / "replay" / (protocol + ".json")
    fixture = json.loads(path.read_text())
    expected, answer = fixture["request"], fixture["response"]

    if provider == "openrouter":
        expected["model"] = answer["model"] = "typesafe/jev-1.14"
        answer["usage"]["cost"] = 0.001
    if not isinstance(body, dict) or expected.keys() != body.keys():
        raise ValueError(f"{provider} Decisions requires {', '.join(expected)}")
    if provider == "openai":
        if not isinstance(body["input"], str):
            raise ValueError(
                "OpenAI input must contain the ticket encoded as JSON text"
            )
        body = dict(body, input=json.loads(body["input"]))
        expected["input"] = json.loads(expected["input"])
    for name in expected:
        if expected[name] != body[name]:
            raise ValueError(
                f"{provider} Decisions {name} differs from the ticket contract"
            )
    return answer
