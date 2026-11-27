"""Guard against CI reporting discovery or skips as successful live calls."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts import paired_live


class PairedLiveTests(unittest.TestCase):
    def test_plan_distinguishes_configured_providers_from_collection(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            flags = {"HAS_OPENAI_API_KEY": "true", "GITHUB_OUTPUT": str(output)}
            with (
                patch.dict(os.environ, flags, clear=True),
                patch.object(paired_live, "summarize"),
            ):
                paired_live.plan("all")
            values = dict(row.split("=", 1) for row in output.read_text().splitlines())
            configured, discovery = json.loads(values["matrix"])
            self.assertEqual(configured, {"provider": "openai"})
            self.assertEqual(discovery["provider"], "collection")
            others = set(paired_live.PROVIDERS) - {"openai"}
            self.assertEqual(set(discovery["missing"].split()), others)
            self.assertEqual(
                set(values["missing"].split(", ")),
                {paired_live.key_name(provider) for provider in others},
            )

    def test_rust_requires_registered_tests_and_runs_exact_names(self):
        with (
            patch("subprocess.check_output", return_value="a::b: test\n"),
            patch(
                "subprocess.run", return_value=SimpleNamespace(returncode=1)
            ) as execute,
        ):
            results = paired_live.rust_results("google", "live", ["a", "a::b"], False)
        execute.assert_called_once_with(
            ["live", "--ignored", "--exact", "a::b"], check=False, env=os.environ
        )
        self.assertEqual(results, {"a": "missing", "a::b": "failed"})

    def test_generation_pins_are_local_to_each_subprocess(self):
        env = {"GOOGLE_GENAI_MODEL": " gemini/custom "}

        def pin(provider, case):
            route = paired_live.GENERATION[provider][0]
            child = paired_live.rust_environment(provider, route + "::" + case, env)
            return child[f"INSTRUCTOR_LIVE_{provider.upper()}_MODEL"]

        docs = "test_current_documented_model"
        self.assertEqual(pin("mistral", "test_int"), "ministral-8b-latest")
        self.assertEqual(pin("mistral", docs), "mistral-small-latest")
        self.assertEqual(pin("google", "test_int"), "custom")
        self.assertEqual(pin("google", docs), "gemini-3.8-flash")
        self.assertEqual(pin("openrouter", "test_int"), "google/gemini-3.8-flash")
        self.assertEqual(pin("cohere", "test_int"), "command-a-03-2025")
        self.assertEqual(
            paired_live.rust_environment("openrouter", "openrouter_jev", env), env
        )
        self.assertEqual(env, {"GOOGLE_GENAI_MODEL": " gemini/custom "})

    def test_pytest_success_requires_every_call_to_pass(self):
        pairs = paired_live.cases("typesafe")
        env = {"TYPESAFE_API_KEY": "test", "OPENAI_API_KEY": "other"}
        for outcome in ("missing", "collected", "skipped", "passed"):
            with self.subTest(outcome=outcome):

                def main(args, plugins):
                    self.assertIn("--run-decisions-live", args)
                    self.assertNotIn("OPENAI_API_KEY", os.environ)
                    self.assertEqual(os.environ["TYPESAFE_API_KEY"], "test")
                    items = [SimpleNamespace(nodeid=test) for test in pairs.values()]
                    if outcome == "missing":
                        items.pop()
                    results = plugins[0]
                    results.pytest_collection_finish(SimpleNamespace(items=items))
                    for item in items:
                        if outcome != "collected":
                            skipped = outcome == "skipped"
                            results.pytest_runtest_logreport(
                                SimpleNamespace(
                                    nodeid=item.nodeid,
                                    failed=False,
                                    skipped=skipped,
                                    when="setup" if skipped else "call",
                                    outcome="skipped" if skipped else "passed",
                                )
                            )
                    return 0

                with (
                    patch.dict(os.environ, env, clear=True),
                    patch.dict(sys.modules, pytest=SimpleNamespace(main=main)),
                    patch.object(sys, "path", sys.path.copy()),
                    patch.object(
                        paired_live,
                        "rust_results",
                        return_value=dict.fromkeys(pairs, "passed"),
                    ),
                    patch.object(paired_live, "summarize"),
                ):
                    code = paired_live.run("typesafe", "live", Path.cwd(), False)
                self.assertEqual(code, 0 if outcome == "passed" else 1)
