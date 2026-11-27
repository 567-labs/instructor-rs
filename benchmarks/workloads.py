"""Shared inputs, result checks and deterministic HTTP responses."""

from collections import Counter
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import socket
from threading import Thread


def compact(value):
    return json.dumps(value, separators=(",", ":"))


def record(i):
    return {"id": i, "title": f"Ticket {i:03d}"}


def batch(n):
    return {"tasks": [record(i) for i in range(n)]}


def batch_cases():
    prompts = [f"Extract ticket {i:03d}" for i in range(16)]
    for provider in ("openai", "anthropic", "mistral"):
        yield {
            "name": f"batch-prepare/{provider}",
            "op": "batch_prepare",
            "provider": provider,
            "prompts": prompts,
        }
        lines = []
        for i in range(16):
            row = {"custom_id": f"request-{i}"}
            if provider == "anthropic":
                tool = {"type": "tool_use", "name": "extract_data", "input": record(i)}
                row["result"] = {
                    "type": "succeeded",
                    "message": {"stop_reason": "tool_use", "content": [tool]},
                }
            else:
                choice = {
                    "finish_reason": "stop",
                    "message": {"content": compact(record(i))},
                }
                row["response"] = {"status_code": 200, "body": {"choices": [choice]}}
            lines.append(compact(row))
        yield {
            "name": f"batch-parse/{provider}",
            "op": "batch_parse",
            "provider": provider,
            "text": "\n".join(lines),
        }


def decision_cases():
    context = {"team": "billing"}
    for provider, model, raw in [
        (
            "typesafe",
            "jev-latest",
            {
                "answers": {
                    "topic": {"type": "choice", "choice": "billing"},
                    "refund": {"type": "noul", "noul": 0.625},
                    "urgency": {"type": "score", "score": 1.5},
                }
            },
        ),
        (
            "openai",
            "gpt-6-luna",
            {
                "answers": [
                    {"name": "topic", "type": "choice", "choice": "billing"},
                    {"name": "refund", "type": "predicate", "probability": 0.625},
                    {"name": "urgency", "type": "score", "score": 1.5},
                ]
            },
        ),
    ]:
        for kind in ["questions", "answers", "reject"]:
            payload = json.loads(compact(raw))
            if kind == "reject":
                answers = payload["answers"]
                urgency = answers["urgency"] if provider == "typesafe" else answers[2]
                urgency["score"] = 3
            yield {
                "name": f"decision/{model}/{kind}",
                "op": kind,
                "model": model,
                "provider": provider,
                "context": context,
                "answer": payload,
            }


def cases():
    yield {"name": "schema/batch", "op": "schema"}
    for n in [1, 16, 128]:
        yield {"name": f"parse/{n}", "op": "parse", "text": compact(batch(n))}
    yield {
        "name": "parse/fenced",
        "op": "parse",
        "text": "```json\n" + compact(batch(16)) + "\n```",
    }
    yield {
        "name": "parse/invalid",
        "op": "invalid",
        "text": '{"tasks":[{"id":"wrong","title":"Ticket"}]}',
    }
    for op, limit in [("validate", 16), ("validate_error", 15)]:
        yield {
            "name": f"context/{op}",
            "op": op,
            "context": {"max_items": limit},
            "text": compact(batch(16)),
        }
    for op in [
        "partial",
        "iterable",
        "partial_first",
        "iterable_first",
        "partial_async",
        "iterable_async",
    ]:
        if op.endswith("_first"):
            sizes = [(16, 20)]
        elif op.endswith("_async"):
            sizes = [(16, 16)]
        else:
            sizes = [(1, 8), (16, 16), (16, 128), (128, 128)]
        for n, size in sizes:
            text = compact(batch(n))
            yield {
                "name": f"{op}/{n}x{size}",
                "op": op,
                "n": n,
                "chunks": [text[i : i + size] for i in range(0, len(text), size)],
            }
    yield from decision_cases()
    yield from batch_cases()
    yield {"name": "cache/hit", "op": "cache", "provider": "openai"}
    for provider in [
        "openai",
        "anthropic",
        "openrouter",
        "google",
        "cohere",
        "mistral",
        "retry",
    ]:
        yield {"name": f"http/{provider}", "op": "http", "provider": provider}
    for provider in ["openai", "anthropic", "openrouter"]:
        yield {
            "name": f"http-parallel/{provider}",
            "op": "http_parallel",
            "provider": provider,
        }
        yield {
            "name": f"http-stream/{provider}",
            "op": "http_stream",
            "provider": provider,
        }
    for route in [
        "typesafe/jev-latest",
        "openrouter/typesafe/jev-1.14",
        "openai/gpt-6-luna",
    ]:
        provider, model = route.split("/", 1)
        yield {
            "name": f"http-decision/{route}",
            "op": "http_decision",
            "provider": provider,
            "model": model,
            "context": {"team": "billing"},
        }
    for provider in ["openai", "openrouter", "mistral"]:
        for mode in ["text", "tool"]:
            for op in ["http", "http_stream"]:
                yield {
                    "name": f"{op.replace('_', '-')}/{provider}/responses-{mode}",
                    "op": op,
                    "provider": provider,
                    "responses": mode,
                    "model": "example/fixture"
                    if provider == "openrouter"
                    else "fixture",
                }


def check_schema(schema):
    """Compare object/array structure and required fields, allowing $ref or inline."""

    def deref(node):
        if "$ref" in node:
            ref = schema
            for key in node["$ref"].removeprefix("#/").split("/"):
                ref = ref[key]
            return ref
        return node

    tasks = schema["properties"]["tasks"]
    item = deref(tasks["items"])
    return [
        schema["type"],
        sorted(schema["required"]),
        tasks["type"],
        item["type"],
        sorted(item["required"]),
        item["properties"]["id"]["type"],
        item["properties"]["title"]["type"],
    ]


def batch_requests(case, value):
    requests = [json.loads(line) for line in value.splitlines()]
    assert len(requests) == len(case["prompts"])
    for i, row in enumerate(requests):
        assert row["custom_id"] == f"request-{i}"
        if case["provider"] == "anthropic":
            body = row["params"]
            tool = body["tools"][0]
            schema = tool["input_schema"]
        else:
            body = row["body"]
            constraint = body["response_format"]["json_schema"]
            assert constraint["strict"] is True
            schema = constraint["schema"]
        assert body["messages"] == [{"role": "user", "content": case["prompts"][i]}]
        assert schema["required"] == ["id", "title"]
        # Compare title-independent typing across Pydantic and Schemars.
        for prop in schema["properties"].values():
            prop.pop("title", None)
            prop.pop("format", None)
            prop.pop("description", None)
        schema.pop("$schema", None)
    return requests


def checked(case, value):
    """Verify independent expected results. The driver also compares full values."""
    op = case["op"]
    if op == "schema":
        value = check_schema(value)
        assert value == [
            "object",
            ["tasks"],
            "array",
            "object",
            ["id", "title"],
            "integer",
            "string",
        ]
    elif op in ("parse", "validate"):
        expected = json.loads(
            case["text"].removeprefix("```json\n").removesuffix("\n```")
        )
        assert value == expected
    elif op in ("invalid", "reject", "validate_error"):
        assert value is True
    elif op in ("partial", "partial_async"):
        assert len(value) == len(case["chunks"]) and value[-1] == batch(case["n"])
    elif op in ("iterable", "iterable_async"):
        assert value == batch(case["n"])["tasks"]
    elif op == "batch_prepare":
        value = batch_requests(case, value)
    elif op == "batch_parse":
        assert value == [
            {"custom_id": f"request-{i}", "result": record(i), "success": True}
            for i in range(16)
        ]
    elif op == "http_parallel":
        assert value == [record(0), batch(2)]
    elif op in ("iterable_first", "http", "cache"):
        assert value == record(0)
    elif op == "http_stream":
        assert len(value) == 3 and value[-1] == record(0)
    elif op == "partial_first":
        assert value == {"tasks": [{"id": 0, "title": None}]}
    elif op == "questions":
        assert value["model"] == case["model"]
        if case["provider"] == "openai":
            value["input"] = json.loads(value["input"])
            assert value["input"] == case["context"]
            topic, refund, urgency = value["questions"]
            assert (topic["type"], refund["type"], urgency["type"]) == (
                "choice",
                "predicate",
                "score",
            )
        else:
            assert value["state"] == case["context"]
            topic, refund, urgency = (
                value["questions"][k] for k in ("topic", "refund", "urgency")
            )
            assert (topic["type"], refund["type"], urgency["type"]) == (
                "choice",
                "noul",
                "score",
            )
        assert refund["instructions"] == "Refund for billing?"
    elif op in ("answers", "http_decision"):
        assert value == {"topic": "billing", "refund": 0.625, "urgency": 7.5}
    else:
        raise ValueError(op)
    return value


def response_item(name=None):
    text = compact(record(0))
    if name:
        item = {
            "id": "fc_bench",
            "type": "function_call",
            "call_id": "call_bench",
            "name": name,
            "arguments": text,
        }
    else:
        item = {
            "id": "msg_bench",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text, "annotations": []}],
        }
    return {
        "id": "resp_bench",
        "object": "response",
        "model": "fixture",
        "created_at": 0,
        "status": "completed",
        "output": [{**item, "status": "completed"}],
    }


def responses():
    item = record(0)
    text = compact(item)

    def chat(args):
        return {
            "id": "chatcmpl-fixture",
            "object": "chat.completion",
            "created": 0,
            "model": "fixture",
            "choices": [
                {
                    "index": 0,
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "benchmark",
                                "type": "function",
                                "function": {
                                    "name": "Record",
                                    "arguments": compact(args),
                                },
                            }
                        ],
                    },
                }
            ],
            "usage": {"completion_tokens": 10, "prompt_tokens": 10, "total_tokens": 20},
        }

    return {
        **{
            f"/{c['provider']}": c["answer"]
            for c in decision_cases()
            if c["op"] == "answers"
        },
        "/v1/chat/completions": chat(item),
        "responses-text": response_item(),
        "responses-tool": response_item("Record"),
        "invalid": chat({"id": "wrong", "title": "Ticket"}),
        "/v1/messages": {
            "id": "msg_bench",
            "type": "message",
            "role": "assistant",
            "model": "fixture",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_fixture",
                    "name": "Record",
                    "input": item,
                }
            ],
            "stop_reason": "tool_use",
            "stop_sequence": None,
            "usage": {"input_tokens": 10, "output_tokens": 10},
        },
        "/v1/interactions": {
            "status": "completed",
            "steps": [
                {"type": "model_output", "content": [{"type": "text", "text": text}]}
            ],
        },
        "/v1beta/models/fixture:generateContent": {
            "candidates": [
                {
                    "content": {"role": "model", "parts": [{"text": text}]},
                    "finishReason": "STOP",
                }
            ]
        },
        "/v2/chat": {
            "id": "bench",
            "finish_reason": "COMPLETE",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
            },
            "usage": {"billed_units": {"input_tokens": 10, "output_tokens": 10}},
        },
    }


def named_events(events):
    return "".join(
        f"event: {e['type']}\ndata: {compact(e)}\n\n" for e in events
    ).encode()


def stream_response(path, req):
    chunks = ['{"id":0,', '"title":"Tick', 'et 000"}']
    if path in ("responses-text", "responses-tool"):
        tool = path == "responses-tool"
        response = response_item(req["tools"][0]["name"] if tool else None)
        item = response["output"][0]
        initial = {"arguments": ""} if tool else {"content": []}
        events = [
            {
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {**item, "status": "in_progress", **initial},
            }
        ]
        kind = "function_call_arguments" if tool else "output_text"
        for delta in chunks:
            event = {
                "type": f"response.{kind}.delta",
                "output_index": 0,
                "item_id": item["id"],
                "delta": delta,
            }
            if not tool:
                event["content_index"] = 0
            events.append(event)
        events.append({"type": "response.completed", "response": response})
        for sequence, event in enumerate(events):
            event["sequence_number"] = sequence
        return named_events(events)
    if path == "/v1/messages":
        events = [
            {
                "type": "message_start",
                "message": {
                    "type": "message",
                    "role": "assistant",
                    "model": "fixture",
                    "id": "msg_bench",
                    "content": [],
                    "stop_reason": None,
                    "usage": {"input_tokens": 10, "output_tokens": 0},
                },
            },
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_fixture",
                    "name": req["tools"][0]["name"],
                    "input": {},
                },
            },
        ]
        for chunk in chunks:
            events.append(
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": chunk},
                }
            )
        events.extend(
            [
                {"type": "content_block_stop", "index": 0},
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"},
                    "usage": {"output_tokens": 10},
                },
                {"type": "message_stop"},
            ]
        )
        return named_events(events)

    lines = []
    for i, chunk in enumerate([*chunks, None]):
        delta = {}
        if chunk is not None:
            tool = {"index": 0, "function": {"arguments": chunk}}
            if i == 0:
                tool["function"]["name"] = req["tools"][0]["function"]["name"]
                tool.update(id="benchmark", type="function")
            delta["tool_calls"] = [tool]
        payload = {
            "id": "chatcmpl-fixture",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "fixture",
            "choices": [
                {
                    "index": 0,
                    "delta": delta,
                    "finish_reason": None if chunk else "tool_calls",
                }
            ],
        }
        lines.append(f"data: {compact(payload)}\n\n")
    return ("".join(lines) + "data: [DONE]\n\n").encode()


@contextmanager
def fixture_server():
    bodies = {k: compact(v).encode() for k, v in responses().items()}
    parallel = responses()
    parallel["/v1/messages"]["content"].append(
        {"type": "tool_use", "id": "toolu_second", "name": "Batch", "input": batch(2)}
    )
    calls = parallel["/v1/chat/completions"]["choices"][0]["message"]["tool_calls"]
    calls.append(
        {
            "id": "second",
            "type": "function",
            "function": {"name": "Batch", "arguments": compact(batch(2))},
        }
    )
    for key in ["/v1/messages", "/v1/chat/completions"]:
        bodies[key + "/parallel"] = compact(parallel[key]).encode()
    counts = Counter()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def setup(self):
            super().setup()
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        def log_message(self, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            key = self.path.split("?")[0]
            if key == "/v1/responses":
                if not request.get("input") or "messages" in request:
                    return self.send_error(400, "Expected input")
                if request.get("tools") and request.get("tool_choice") != "none":
                    schema = request["tools"][0]["parameters"]
                    key = "responses-tool"
                else:
                    schema = request["text"]["format"]["schema"]
                    key = "responses-text"
                if set(schema["properties"]) != {"id", "title"}:
                    return self.send_error(400, "Expected record schema")
                counts[key] += 1
            elif key == "/v1/interactions":
                counts["interactions"] += 1
            elif key == "/v1beta/models/fixture:generateContent":
                counts["generateContent"] += 1
            elif key == "/v1/messages":
                counts[f"anthropic-{self.headers.get('anthropic-version')}"] += 1
            if len(request.get("tools", [])) == 2:
                key += "/parallel"
            if key == "/openrouter":
                key = "/typesafe"
            if request.get("model") == "retry" and len(request["messages"]) == 1:
                key = "invalid"
            if key not in bodies:
                self.send_error(404, self.path)
                return
            counts["total"] += 1
            if key == "invalid":
                counts["invalid"] += 1
            content_type = "application/json"
            if request.get("stream"):
                counts["stream"] += 1
                content_type = "text/event-stream"
                body = stream_response(key, request)
            else:
                body = bodies[key]
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}", counts
        finally:
            server.shutdown()
            thread.join()
