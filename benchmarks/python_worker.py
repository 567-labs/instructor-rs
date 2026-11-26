"""Exercise the reference checkout in a separate process from the fixture server."""

import asyncio
from collections.abc import Iterable
from contextlib import redirect_stdout
from io import BytesIO
import json
import os
from pathlib import Path
import sys
from time import perf_counter_ns
from typing import Annotated

import instructor
from instructor import Mode, Partial
from instructor.decisions import Choice, Choices, Noul, Score
from instructor.decisions.client import _parse, _request
from instructor.v2.core.function_calls import _validate_model_from_json
from instructor.v2.dsl.iterable import IterableModel
from pydantic import BaseModel, ValidationInfo, model_validator


class Record(BaseModel):
    id: int
    title: str


class Batch(BaseModel):
    tasks: list[Record]


class LimitedBatch(Batch):
    @model_validator(mode="after")
    def check_length(self, info: ValidationInfo):
        if len(self.tasks) > info.context["max_items"]:
            raise ValueError("too many items")
        return self


class Topic(Choices):
    """Which topic?"""

    BILLING = Choice()
    OTHER = Choice()


class Ticket(BaseModel):
    topic: Topic
    refund: Annotated[float, Noul("Refund for {{ team }}?")]
    urgency: Annotated[
        float, Score("Urgent?", levels=["low", "normal", "high"], scale=(0, 10))
    ]


def http_operation(case, endpoint):
    provider = case["provider"]
    key = "local"
    model = "retry" if provider == "retry" else "fixture"
    options = {}
    parallel = case["op"] == "http_parallel"
    tool_mode = Mode.PARALLEL_TOOLS if parallel else Mode.TOOLS
    response_model = Iterable[Record | Batch] if parallel else Record
    if case["op"] == "cache":
        from instructor.cache import AutoCache

        options["cache"] = AutoCache(maxsize=10)
    if provider in ("openai", "retry"):
        from openai import OpenAI

        client = instructor.from_openai(
            OpenAI(api_key=key, base_url=endpoint + "/v1"), mode=tool_mode
        )
    elif provider == "openrouter":
        client = instructor.from_provider(
            "openrouter/fixture",
            api_key=key,
            base_url=endpoint + "/v1",
            mode=tool_mode,
        )
    elif provider == "anthropic":
        from anthropic import Anthropic

        client = instructor.from_anthropic(
            Anthropic(api_key=key, base_url=endpoint), mode=tool_mode
        )
        options["max_tokens"] = 1024
    elif provider == "mistral":
        from mistralai.client import Mistral

        client = instructor.from_mistral(
            Mistral(api_key=key, server_url=endpoint), mode=Mode.TOOLS
        )
    elif provider == "cohere":
        from cohere import ClientV2

        client = instructor.from_cohere(
            ClientV2(
                api_key=key, base_url=endpoint, log_warning_experimental_features=False
            ),
            mode=Mode.JSON_SCHEMA,
        )
    elif provider == "google":
        from google import genai

        sdk = genai.Client(
            api_key=key,
            vertexai=False,
            http_options={"base_url": endpoint, "api_version": "v1beta"},
        )
        client = instructor.from_genai(sdk, mode=Mode.JSON)
    else:
        raise ValueError(provider)

    streaming = case["op"] == "http_stream"
    method = client.create_partial if streaming else client.create

    def create():
        result = method(
            model=model,
            messages=[{"role": "user", "content": "Extract ticket"}],
            response_model=response_model,
            max_retries=2,
            strict=True,
            **options,
        )
        return list(result) if streaming or parallel else result

    return create


def rejection(call):
    def op():
        try:
            call()
        except ValueError:
            return True
        raise AssertionError("Invalid input accepted")

    return op


def operation(case, endpoint, loop):
    op = case["op"]
    if op in ("batch_prepare", "batch_parse"):
        from instructor.batch import BatchProcessor

        processor = BatchProcessor(f"{case['provider']}/fixture", Record)
        if op == "batch_prepare":

            def prepare_messages():
                conversations = [
                    [{"role": "user", "content": p}] for p in case["prompts"]
                ]
                return processor.create_batch_from_messages(conversations)

            return prepare_messages
        text = case["text"]
        return lambda: processor.parse_results(text)
    if op == "http_decision":
        provider, context = case["provider"], case["context"]
        client = instructor.from_provider(
            f"{provider}/{case['model']}",
            mode=Mode.DECISIONS,
            api_key="local",
            endpoint=endpoint + "/" + provider,
        )
        return lambda: client.create(response_model=Ticket, context=context)
    if op in ("http", "http_stream", "http_parallel", "cache"):
        return http_operation(case, endpoint)
    if op == "schema":
        return Batch.model_json_schema
    if op in ("validate", "validate_error"):
        text = case["text"]
        context = case["context"]

        def validate():
            return _validate_model_from_json(LimitedBatch, text, context, strict=True)

        return rejection(validate) if op == "validate_error" else validate
    if op in ("parse", "invalid"):
        text = case["text"]

        def parse():
            return _validate_model_from_json(Batch, text, strict=True)

        return rejection(parse) if op == "invalid" else parse
    if op in ("partial_async", "iterable_async"):
        stream = (
            Partial[Batch].model_from_chunks_async
            if op == "partial_async"
            else IterableModel(Record).tasks_from_chunks_async
        )
        chunks = case["chunks"]

        async def source():
            for fragment in chunks:
                yield fragment

        async def fetch():
            return [item async for item in stream(source())]

        return lambda: loop.run_until_complete(fetch())
    if op in ("partial", "iterable", "partial_first", "iterable_first"):
        stream = (
            Partial[Batch].model_from_chunks
            if op.startswith("partial")
            else IterableModel(Record).tasks_from_chunks
        )
        chunks = case["chunks"]
        consume = next if op.endswith("_first") else list
        return lambda: consume(stream(chunks))

    if op in ("questions", "answers", "reject"):
        model, provider = case["model"], case["provider"]
        context, raw = case["context"], case["answer"]

        def prepare():
            return _request(model, Ticket, context, provider, None)

        if op == "questions":
            return lambda: prepare()[1]
        fields, _, _ = prepare()

        def decode():
            return _parse(Ticket, fields, raw, context, True, provider)

        return rejection(decode) if op == "reject" else decode
    raise ValueError(op)


def measure(call, target_ns):
    def batch(count):
        start = perf_counter_ns()
        for _ in range(count):
            call()
        return perf_counter_ns() - start

    iterations = 1
    while batch(iterations) < target_ns:
        iterations *= 2
    return {"iterations": iterations, "elapsed_ns": batch(iterations)}


def serialized(obj):
    if isinstance(obj, BytesIO):
        return obj.getvalue().decode()
    return obj.model_dump(mode="json")


def run():
    request = json.load(sys.stdin)
    source = Path(request["python_repo"]).resolve()
    if not Path(instructor.__file__).resolve().is_relative_to(source):
        raise RuntimeError(f"Unexpected instructor checkout: {instructor.__file__}")
    results = {}
    loop = asyncio.new_event_loop()
    for case in request["cases"]:
        call = operation(case, request["endpoint"], loop)
        # Serialize the witness before timing and release the model instances.
        witness = json.loads(json.dumps(call(), default=serialized))
        results[case["name"]] = {
            "witness": witness,
            **measure(call, request["target_ns"]),
        }
    loop.close()
    return results


if __name__ == "__main__":
    # BatchProcessor prints a status line for every prepared buffer.
    with open(os.devnull, "w") as quiet, redirect_stdout(quiet):
        report = run()
    print(json.dumps(report))
