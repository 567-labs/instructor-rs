"""Fixed result data and native JSON / SSE envelopes for paired generation."""

import json
from pathlib import Path

from paired_live import DOCUMENTED_MODELS


FIXTURES = Path(__file__).parent / "fixtures" / "replay"
# Shared with instructor/tests/providers/complexity.rs.
CASES = json.loads(
    (
        Path(__file__).resolve().parents[1] / "instructor/tests/support/generation.json"
    ).read_text()
)


def first_user_text(provider, body):
    messages = body.get("messages") or body.get("contents") or body.get("input")
    if isinstance(messages, str):
        return messages
    if provider == "cohere" and not body.get("tools"):
        # Python's Cohere handler prepends its schema in a separate user message.
        assert "json schema below" in messages[0]["content"], (
            "missing schema instruction"
        )
        messages = messages[1:]
    for item in messages or []:
        if item.get("role") == "user":
            content = item.get("content", item.get("parts"))
            if isinstance(content, str):
                return content
            return "".join(block["text"] for block in content)
    raise AssertionError("expected a user prompt")


def generation_response(provider, contract, path, body, attempt):
    route, name = contract.split("::")
    case = CASES[name]
    data = case["answer"]
    model = DOCUMENTED_MODELS[provider]
    if provider == "mistral" and name != "test_current_documented_model":
        model = "ministral-8b-latest"
    stream = name.endswith("_streaming")
    expected_path = {
        "openai_chat": "/v1/chat/completions",
        "openai_responses": "/v1/responses",
        "openrouter_chat": "/api/v1/chat/completions",
        "mistral_chat": "/v1/chat/completions",
        "anthropic_messages": "/v1/messages",
        "google_genai": f"/v1beta/models/{model}:"
        + ("streamGenerateContent" if stream else "generateContent"),
        "cohere_chat": "/v2/chat",
    }[route]
    assert path == expected_path, f"expected {expected_path}, got {path}"
    request = json.dumps(body)
    prompt = first_user_text(provider, body)
    assert prompt == case["prompt"], f"unexpected user prompt {prompt!r}"
    if provider != "google":
        assert body["model"] == model, "unexpected model"
        assert (body.get("stream") is True) == stream, "unexpected stream flag"
    if name in {"test_enum_extraction", "test_nested_enum_collection"}:
        assert all(s in request for s in ("packed", "in_transit", "delivered")), (
            "missing enum schema labels"
        )
    if attempt == 2:
        assert "Check both fields against the original text" in request, (
            "missing reask feedback"
        )

    text = json.dumps(data)
    fragments = []
    if stream:
        fragments = [text[:14], text[14:21], text[21:]]
        if name == "test_iterable_streaming" and provider == "mistral":
            # Mistral serves a complete tasks envelope per item in tools mode.
            fragments = [json.dumps({"tasks": [item]}) for item in data["tasks"]]

    protocol, tool = wire_protocol(provider, route, body)
    fields = {"$model": model, "$name": tool, "$object": data, "$text": text}
    if stream:
        fields.update(zip(("$first", "$second", "$third"), fragments, strict=True))
    reply = wire_reply(protocol, fields, stream)
    return model, prompt, data, reply


def wire_protocol(provider, route, body):
    tools = body.get("tools")
    if provider in ("cohere", "google") and not tools:
        return provider + "_text", None
    tool = tools[0]
    if route == "openai_responses":
        return route, tool["name"]
    if provider == "anthropic":
        return provider, tool["name"]
    if provider == "google":
        declarations = tool.get("functionDeclarations") or tool["function_declarations"]
        return "google_tool", declarations[0]["name"]
    protocol = "cohere_tool" if provider == "cohere" else "chat"
    return protocol, tool["function"]["name"]


def wire_reply(protocol, fields, stream):
    """Each JSONL file holds one completion followed by its ordered SSE data frames.

    Only entire field values are substituted. JSON encoding and SSE framing
    happen afterward, so quotes and newlines in the payload stay literal.
    """

    def fill(record):
        return {
            key: fields[value]
            if isinstance(value, str) and value.startswith("$")
            else value
            for key, value in record.items()
        }

    path = FIXTURES / (protocol + ".jsonl")
    completion, *events = path.read_text().splitlines()
    if stream:
        if not events:
            raise ValueError(f"no streaming fixture for {protocol}")
        return [json.loads(line, object_hook=fill) for line in events]
    return json.loads(completion, object_hook=fill)
