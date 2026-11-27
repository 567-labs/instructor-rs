"""Select and check the same live contracts in the Rust and Python repositories."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


PROVIDERS = (
    "openai",
    "anthropic",
    "openrouter",
    "google",
    "cohere",
    "mistral",
    "typesafe",
)
GENERATION = {
    "openai": ("openai_structured", "openai_responses"),
    "anthropic": ("anthropic_messages",),
    "openrouter": ("openrouter_responses",),
    "google": ("google_interactions",),
    "cohere": ("cohere_chat",),
    "mistral": ("mistral_responses",),
}
DECISIONS = {
    "openai": "openai_decisions",
    "openrouter": "openrouter_jev",
    "typesafe": "typesafe_jev",
}
ROUTED_JEV = "typesafe/jev-1.14"
DOCUMENTED_MODELS = {
    "openai": "gpt-6-luna",
    "anthropic": "claude-haiku-5-5",
    "openrouter": "google/gemini-3.8-flash",
    "google": "gemini-3.8-flash",
    "cohere": "command-a-03-2025",
    "mistral": "mistral-small-latest",
}
CORE_TESTS = {
    "test_simple_types": ("test_int",),
    "test_basic_extraction": (
        "test_simple_extraction",
        "test_list_extraction",
        "test_nested_model_extraction",
    ),
    "test_model_complexity": ("test_enum_extraction", "test_nested_enum_collection"),
    "test_retries": ("test_validation_with_retries",),
    "test_streaming": ("test_partial_streaming", "test_iterable_streaming"),
}


def key_name(provider):
    return f"{provider.upper()}_API_KEY"


def summarize(text):
    print(text)
    if path := os.getenv("GITHUB_STEP_SUMMARY"):
        with open(path, "a", encoding="utf-8") as summary:
            print(text, file=summary)


def cases(provider):
    """Map stable Rust test names to exact pytest node IDs in the pinned checkout."""
    pairs = {}
    for route in GENERATION.get(provider, ()):
        parameter = "openai-responses" if route == "openai_responses" else provider
        for module, names in CORE_TESTS.items():
            for name in names:
                pairs[f"{route}::{name}"] = (
                    f"tests/llm/test_core_providers/{module}.py::{name}[{parameter}]"
                )
        if route != "openai_responses":
            name = "test_current_documented_model"
            model = f"{provider}/{DOCUMENTED_MODELS[provider]}"
            pairs[f"{route}::{name}"] = (
                f"tests/llm/test_current_doc_models.py::{name}[{model}-{key_name(provider)}]"
            )
    if provider in DECISIONS:
        path = "tests/llm/test_decisions/test_live_decisions.py"
        for kind, suffix in (("async", ""), ("sync", "_blocking")):
            pairs[DECISIONS[provider] + suffix] = (
                f"{path}::test_live_{kind}_decisions_contract[{provider}]"
            )
    return pairs


def plan(selected):
    matrix, missing = [], []
    rows = ["| Provider | Pairs | Credential |", "| --- | --- | --- |"]
    for provider in PROVIDERS if selected == "all" else (selected,):
        key = key_name(provider)
        if os.getenv(f"HAS_{key}") == "true":
            matrix.append({"provider": provider})
            status = "Configured"
        else:
            missing.append(provider)
            status = f"Live tests skipped, missing `{key}`"
        rows.append(f"| {provider} | {len(cases(provider))} | {status} |")
    summarize("\n".join(rows))
    if missing:
        matrix.append({"provider": "collection", "missing": " ".join(missing)})
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print("matrix=" + json.dumps(matrix), file=output)
        print("missing=" + ", ".join(key_name(name) for name in missing), file=output)


def generation_model(provider, test, google_model=""):
    """Mirror the pinned Python suite's core and documentation models."""
    if test != "test_current_documented_model":
        if provider == "mistral":
            return "ministral-8b-latest"
        if provider == "google" and google_model.strip():
            return google_model.strip().removeprefix("google/").removeprefix("gemini/")
    return DOCUMENTED_MODELS[provider]


def rust_environment(provider, name, environment):
    route, _, test = name.partition("::")
    if route not in GENERATION.get(provider, ()):
        return environment
    model = generation_model(provider, test, environment.get("GOOGLE_GENAI_MODEL", ""))
    return {**environment, f"INSTRUCTOR_LIVE_{provider.upper()}_MODEL": model}


def rust_results(provider, binary, tests, collect_only):
    listed = subprocess.check_output([binary, "--list", "--ignored"], text=True)
    names = {
        line.removesuffix(": test")
        for line in listed.splitlines()
        if line.endswith(": test")
    }
    results = {}
    for name in tests:
        if name not in names:
            results[name] = "missing"
        elif collect_only:
            results[name] = "collected"
        else:
            result = subprocess.run(
                [binary, "--ignored", "--exact", name],
                env=rust_environment(provider, name, os.environ),
                check=False,
            )
            results[name] = "passed" if result.returncode == 0 else "failed"
    return results


class PythonResults:
    def __init__(self):
        self.outcomes = {}

    def pytest_collection_finish(self, session):
        self.outcomes = {item.nodeid: "collected" for item in session.items}

    def pytest_runtest_logreport(self, report):
        if report.failed or report.skipped or report.when == "call":
            self.outcomes[report.nodeid] = report.outcome


def run(provider, binary, reference, collect_only):
    key = key_name(provider)
    credential = os.environ.get(key)
    if collect_only:
        credential = "collection-only"
    if not credential:
        raise SystemExit(f"Missing {key}")
    for other in PROVIDERS:
        os.environ.pop(key_name(other), None)
    os.environ[key] = credential
    if provider == "google":
        if os.getenv("GOOGLE_GENAI_MODEL", "").strip():
            summarize(
                "Google core contracts use the configured `GOOGLE_GENAI_MODEL`.\n"
            )
        else:
            summarize(
                "Google core contracts use the documented `google/gemini-3.8-flash` default.\n"
            )
    for route in DECISIONS:
        variable = f"{route.upper()}_DECISIONS_MODEL"
        if not os.getenv(variable):
            os.environ.pop(variable, None)
    if provider == "openrouter":
        legacy = os.getenv("INSTRUCTOR_LIVE_OPENROUTER_JEV_MODEL", "").strip()
        os.environ.setdefault("OPENROUTER_DECISIONS_MODEL", legacy or ROUTED_JEV)

    pairs = cases(provider)
    rust = rust_results(provider, binary, pairs, collect_only)
    os.chdir(reference)
    sys.path.insert(0, str(reference))
    import pytest

    args = ["-q", "-rs", "--tb=short", "--asyncio-mode=auto", *pairs.values()]
    if provider in DECISIONS:
        args.append("--run-decisions-live")
    if collect_only:
        args.append("--collect-only")
    python = PythonResults()
    status = pytest.main(args, plugins=[python])
    expected = "collected" if collect_only else "passed"
    matched = 0
    rows = ["| Contract | Rust | Python |", "| --- | --- | --- |"]
    for name, selector in pairs.items():
        result = python.outcomes.get(selector, "missing")
        matched += rust[name] == result == expected
        rows.append(f"| `{name}` | {rust[name]} | {result} |")
    summarize(
        f"### {provider} · {matched}/{len(pairs)} pairs {expected}\n\n"
        + "\n".join(rows)
    )
    return 0 if status == 0 and matched == len(pairs) > 0 else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    planner = commands.add_parser(
        "plan", help="Plan CI jobs from credential presence flags"
    )
    planner.add_argument("provider", choices=("all", *PROVIDERS))
    runner = commands.add_parser("run", help="Check paired contracts for one provider")
    runner.add_argument("provider", choices=PROVIDERS)
    runner.add_argument("--rust-binary", type=Path, required=True)
    runner.add_argument("--python-repo", type=Path, required=True)
    runner.add_argument("--collect-only", action="store_true")
    args = parser.parse_args()
    if args.command == "plan":
        plan(args.provider)
        return 0
    return run(
        args.provider,
        args.rust_binary.resolve(),
        args.python_repo.resolve(),
        args.collect_only,
    )


if __name__ == "__main__":
    sys.exit(main())
