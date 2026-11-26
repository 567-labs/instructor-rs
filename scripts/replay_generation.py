"""Fixed result data and native JSON / SSE envelopes for paired generation."""

import json

from paired_live import DOCUMENTED_MODELS


def user(name, age):
    return {"name": name, "age": age}


PROMPTS = {
    "test_current_documented_model": "Extract the integer 42.",
    "test_int": "Return the number 42",
    "test_enum_extraction": "The package is in transit. Extract its shipment state.",
    "test_simple_extraction": "Extract: Jason is 25 years old",
    "test_nested_model_extraction": (
        "Extract: John Doe, 28 years old, lives at 123 Main St, Springfield, USA"
    ),
    "test_nested_enum_collection": (
        "Extract the manifest for the North depot in the given order. "
        "Package A1 is packed, carrier unknown. Package B2 is in transit "
        "with carrier Swift. Use null for an unknown carrier."
    ),
    "test_list_extraction": "Extract: Alice is 30, Bob is 25, Charlie is 35",
    "test_validation_with_retries": "Extract: Sarah Johnson is 25 years old",
    "test_partial_streaming": "Jason Liu is 30 years old",
    "test_iterable_streaming": "Create 3 users: Alice (25), Bob (30), Carol (35)",
}

PAYLOAD = {
    "test_current_documented_model": {"value": 42},
    "test_int": {"content": 42},
    "test_enum_extraction": {"content": "in_transit"},
    "test_simple_extraction": user("Jason", 25),
    "test_nested_model_extraction": {
        **user("John Doe", 28),
        "address": {"street": "123 Main St", "city": "Springfield", "country": "USA"},
    },
    "test_nested_enum_collection": {
        "depot": "North",
        "packages": [
            {"tracking_id": "A1", "state": "packed", "carrier": None},
            {"tracking_id": "B2", "state": "in_transit", "carrier": "Swift"},
        ],
    },
    "test_list_extraction": {
        "tasks": [user("Alice", 30), user("Bob", 25), user("Charlie", 35)]
    },
    "test_validation_with_retries": user(" Sarah Johnson ", 25),
    "test_partial_streaming": user("Jason Liu", 30),
    "test_iterable_streaming": {
        "tasks": [user("Alice", 25), user("Bob", 30), user("Carol", 35)]
    },
}


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
    data = PAYLOAD[name]
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
    assert prompt == PROMPTS[name], f"unexpected user prompt {prompt!r}"
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

    reply, events = envelopes(provider, route, body, data, fragments)
    return model, prompt, data, events if stream else reply


def envelopes(provider, route, body, data, fragments):
    text = json.dumps(data)
    if provider == "google":
        part = {"text": text}
        if "tools" in body:
            tools = body["tools"][0]
            declarations = (
                tools.get("functionDeclarations") or tools["function_declarations"]
            )
            part = {"functionCall": {"name": declarations[0]["name"], "args": data}}

        def google(part, last):
            candidate = {"content": {"role": "model", "parts": [part]}}
            if last:
                candidate["finishReason"] = "STOP"
            return {"candidates": [candidate]}

        events = [
            google({"text": s}, i == len(fragments) - 1)
            for i, s in enumerate(fragments)
        ]
        return google(part, True), events

    model = body["model"]
    usage = {"input_tokens": 1, "output_tokens": 1}
    if provider == "anthropic":
        name = body["tools"][0]["name"]
        tool = {"type": "tool_use", "id": "toolu_test", "name": name, "input": data}
        result = {
            "type": "message",
            "id": "msg_test",
            "role": "assistant",
            "content": [tool],
            "model": model,
            "stop_reason": "tool_use",
            "stop_sequence": None,
            "usage": usage,
        }
        events = [
            {
                "type": "message_start",
                "message": dict(result, content=[], stop_reason=None),
            },
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": dict(tool, input={}),
            },
        ]
        for fragment in fragments:
            delta = {"type": "input_json_delta", "partial_json": fragment}
            events.append({"type": "content_block_delta", "index": 0, "delta": delta})
        events.extend(
            [
                {"type": "content_block_stop", "index": 0},
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use", "stop_sequence": None},
                    "usage": usage,
                },
                {"type": "message_stop"},
            ]
        )
        return result, events

    if route == "openai_responses":
        name = body["tools"][0]["name"]
        item = {
            "type": "function_call",
            "id": "fc_test",
            "call_id": "call_test",
            "name": name,
            "arguments": text,
            "status": "completed",
        }
        result = {
            "id": "resp_test",
            "object": "response",
            "status": "completed",
            "output": [item],
            "model": model,
        }
        events = [
            {
                "type": "response.created",
                "response": dict(result, status="in_progress", output=[]),
            },
            {
                "type": "response.output_item.added",
                "output_index": 0,
                "item": dict(item, arguments="", status="in_progress"),
            },
        ]
        events.extend(
            {
                "type": "response.function_call_arguments.delta",
                "item_id": item["id"],
                "output_index": 0,
                "delta": s,
            }
            for s in fragments
        )
        events.extend(
            [
                {"type": "response.output_item.done", "output_index": 0, "item": item},
                {"type": "response.completed", "response": result},
            ]
        )
        return result, [dict(e, sequence_number=i) for i, e in enumerate(events)]

    def call(arguments):
        name = body["tools"][0]["function"]["name"]
        return {
            "index": 0,
            "id": "call_test",
            "type": "function",
            "function": {"name": name, "arguments": arguments},
        }

    if provider == "cohere":
        # The Python handler requests JSON text. Rust uses native function calls.
        message = {"role": "assistant"}
        if body.get("tools"):
            message["tool_calls"] = [call(text)]
            finish = "TOOL_CALL"
            events = [
                {
                    "type": "tool-call-delta",
                    "index": 0,
                    "delta": {"message": {"tool_calls": call(s)}},
                }
                for s in fragments
            ]
        else:
            message["content"] = [{"type": "text", "text": text}]
            finish = "COMPLETE"
            events = [
                {
                    "type": "content-delta",
                    "index": 0,
                    "delta": {"message": {"content": {"text": s}}},
                }
                for s in fragments
            ]
        metadata = {
            "finish_reason": finish,
            "usage": {"billed_units": usage, "tokens": usage},
        }
        events.append({"type": "message-end", "delta": metadata})
        return {"id": "chat_test", "message": message, **metadata}, events

    meta = {"id": "chatcmpl_test", "created": 0, "model": model}
    message = {"role": "assistant", "content": None, "tool_calls": [call(text)]}
    result = {
        **meta,
        "object": "chat.completion",
        "choices": [{"index": 0, "message": message, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }
    events = []
    for i, s in enumerate(fragments):
        choice = {
            "index": 0,
            "delta": {"tool_calls": [call(s)]},
            "finish_reason": "tool_calls" if i == len(fragments) - 1 else None,
        }
        events.append({**meta, "object": "chat.completion.chunk", "choices": [choice]})
    events.append("[DONE]")
    return result, events
