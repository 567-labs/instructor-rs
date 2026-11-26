"""Checks for the local transport and the replay report's acceptance criteria."""

import json
import os
import socket
import unittest
from unittest.mock import patch

from scripts import paired_replay as replay


class ReplayTests(unittest.TestCase):
    def test_child_environment_drops_credentials_and_overrides(self):
        overrides = {
            "OPENAI_API_KEY": "inherited",
            "TYPESAFE_API_KEY": "inherited",
            "HTTPS_PROXY": "inherited",
            "OPENAI_BASE_URL": "inherited",
            "PYTEST_ADDOPTS": "inherited",
        }
        with patch.dict(os.environ, overrides):
            env = replay.test_environment()
        self.assertFalse(env.keys() & overrides.keys())
        self.assertEqual(env["PYTHON_DOTENV_DISABLED"], "1")

    def test_blocks_resolution_and_connections_outside_fixture(self):
        recorder = replay.Replay({})
        with replay.local_sockets(recorder, 10000), socket.socket() as client:
            with self.assertRaises(RuntimeError):
                socket.getaddrinfo("example.invalid", 10000)
            with self.assertRaises(RuntimeError):
                client.connect(("127.0.0.1", 10001))
            with self.assertRaises(RuntimeError):
                client.connect_ex(("192.0.2.1", 10000))
        self.assertEqual(len(recorder.errors), 3)

    def test_reference_requires_full_sha_and_clean_checkout(self):
        sha = "a" * 40
        with patch.object(replay, "revision", return_value=sha):
            with self.assertRaises(ValueError):
                replay.check_reference(".", sha[:7])
            with patch.object(
                replay.subprocess, "check_output", return_value=b"M file"
            ):
                with self.assertRaises(ValueError):
                    replay.check_reference(".", sha)
            with patch.object(replay.subprocess, "check_output", return_value=b""):
                self.assertEqual(replay.check_reference(".", sha), sha)

    def test_only_pinned_google_retry_gets_a_known_difference(self):
        rev = replay.GOOGLE_RETRY_REVISION
        case = replay.GOOGLE_RETRY_TEST
        first, both = ["a"], ["a", "a"]
        self.assertEqual(
            replay.compare_requests(rev, case, both, first), "known difference"
        )
        self.assertEqual(replay.compare_requests(rev, case, both, both), "matched")
        for args in (
            (rev, case, [], []),
            (rev, case, first, first),
            (rev, case, both, []),
            (rev, case, both, ["b"]),
            ("other", case, both, first),
            (rev, "openai_chat::test_validation_with_retries", both, first),
        ):
            with self.subTest(args=args):
                self.assertEqual(replay.compare_requests(*args), "different")

    def test_retry_requires_the_feedback_in_second_request(self):
        recorder = replay.Replay({})
        recorder.active = ("rust", "google", replay.GOOGLE_RETRY_TEST)
        path = "/v1beta/models/gemini-3.8-flash:generateContent"
        body = {
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Extract: Sarah Johnson is 25 years old"}],
                }
            ]
        }
        recorder.response(path, body)
        with self.assertRaisesRegex(AssertionError, "feedback"):
            recorder.response(path, body)
        body["contents"].append(
            {
                "role": "user",
                "parts": [{"text": "Check both fields against the original text"}],
            }
        )
        recorder.response(path, body)
        with self.assertRaisesRegex(ValueError, "additional"):
            recorder.response(path, body)

    def test_jev_rejects_mismatched_model_or_questions(self):
        body = {
            "model": "wrong",
            "state": {
                "ticket": "Please refund the duplicate charge on my invoice. There is no rush."
            },
            "questions": {},
        }
        with self.assertRaisesRegex(ValueError, "model"):
            replay.decision_response("openrouter", body)
        body["model"] = "typesafe/jev-1.13"
        with self.assertRaisesRegex(ValueError, "questions"):
            replay.decision_response("openrouter", body)

    def test_stream_requires_prompt_and_spans_multiple_frames(self):
        recorder = replay.Replay({})
        recorder.active = ("rust", "openai", "openai_chat::test_partial_streaming")
        path = "/v1/chat/completions"
        body = {
            "model": "gpt-6-luna",
            "stream": True,
            "tools": [{"function": {"name": "User"}}],
        }
        with self.assertRaisesRegex(AssertionError, "prompt"):
            recorder.response(path, body)
        body["messages"] = [{"role": "user", "content": "Jason Liu is 30 years old"}]
        events = recorder.response(path, body)
        self.assertEqual(replay.frame(events[-1]), b"data: [DONE]\n\n")
        pieces = [
            e["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            for e in events[:-1]
        ]
        self.assertGreater(len(pieces), 1)
        self.assertEqual(json.loads("".join(pieces)), {"name": "Jason Liu", "age": 30})
