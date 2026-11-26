"""Replay the exact paired live contracts using a loopback HTTP server and synthetic responses."""

import argparse
from collections import defaultdict
from contextlib import ExitStack, contextmanager
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Thread
from unittest.mock import patch
from urllib.parse import urlsplit

# Include sibling modules when the worker is launched with Python -I.
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from paired_live import PROVIDERS, PythonResults, cases, key_name  # noqa: E402
from replay_decisions import decision_response  # noqa: E402
from replay_generation import generation_response  # noqa: E402

ROOT = HERE.parent
KEY = "synthetic-replay"
LOOPBACK = "127.0.0.1"
HOSTS = {
    "openai": {"api.openai.com"},
    "openrouter": {"openrouter.ai"},
    "anthropic": {"api.anthropic.com"},
    "google": {"generativelanguage.googleapis.com"},
    "cohere": {"api.cohere.com", "api.cohere.ai"},
    "mistral": {"api.mistral.ai"},
    "typesafe": {"api.typesafe.ai"},
}
DECISION_PATHS = {
    "openai": "/v1/decisions",
    "openrouter": "/api/alpha/decisions",
    "typesafe": "/v1/systemone",
}
GOOGLE_RETRY_REVISION = "0ab894f0661a6968b17e67cf5b7657f1fcb3b9f7"
GOOGLE_RETRY_TEST = "google_genai::test_validation_with_retries"


def expected_attempts(case):
    return 2 if case.endswith("::test_validation_with_retries") else 1


class Replay(PythonResults):
    def __init__(self, pairs):
        super().__init__()
        self.selectors = {
            node: (provider, name) for (provider, name), node in pairs.items()
        }
        self.traces = defaultdict(list)
        self.errors = []
        self.active = None

    def pytest_runtest_setup(self, item):
        self.active = ("python", *self.selectors[item.nodeid])

    def response(self, path, body):
        _, provider, case = self.active
        trace = self.traces[self.active]
        attempt = len(trace) + 1
        if attempt > expected_attempts(case):
            raise ValueError("unexpected additional request")
        if "::" in case:
            model, prompt, data, reply = generation_response(
                provider, case, path, body, attempt
            )
        else:
            if path != DECISION_PATHS[provider]:
                raise ValueError(f"unexpected decision endpoint {path}")
            reply = data = decision_response(provider, body)
            model = body["model"]
            state = json.loads(body["input"]) if provider == "openai" else body["state"]
            prompt = state["ticket"]
        fingerprint = sha256(json.dumps(data, sort_keys=True).encode()).hexdigest()
        trace.append(
            {
                "path": path,
                "model": model,
                "prompt_sha256": sha256(prompt.encode()).hexdigest(),
                "result_sha256": fingerprint,
                "sse_events": len(reply) if isinstance(reply, list) else 0,
            }
        )
        return reply


def frame(event):
    name = (
        f"event: {event['type']}\n"
        if isinstance(event, dict) and "type" in event
        else ""
    )
    text = event if isinstance(event, str) else json.dumps(event)
    return f"{name}data: {text}\n\n".encode()


@contextmanager
def serve(replay):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def do_POST(self):
            self.close_connection = True
            try:
                keys = [
                    self.headers.get("X-API-Key"),
                    self.headers.get("x-goog-api-key"),
                    self.headers.get("Authorization", "").removeprefix("Bearer "),
                ]
                if KEY not in keys:
                    raise ValueError("expected synthetic authorization")
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                path = urlsplit(self.path).path
                reply = replay.response(path, body)
            except Exception as exc:
                replay.errors.append(f"{replay.active}: {exc}")
                self.send_error(400, "Fixture rejected the request")
                return

            self.send_response(200)
            self.send_header("Connection", "close")
            if isinstance(reply, dict):
                data = json.dumps(reply).encode()
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            else:
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                # Split SSE frames across HTTP chunks as well as JSON across events.
                data = b"".join(frame(e) for e in reply)
                for i in range(0, len(data), 31):
                    chunk = data[i : i + 31]
                    self.wfile.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")

    with ThreadingHTTPServer((LOOPBACK, 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            thread.join()


@contextmanager
def local_sockets(replay, port):
    def blocked(reason):
        replay.errors.append(f"{replay.active}: {reason}")
        raise RuntimeError(reason)

    def address(host, service):
        if host not in (LOOPBACK, LOOPBACK.encode()) or service not in (
            port,
            str(port),
        ):
            blocked("outbound connection outside fixture server")

    def connected(method):
        def connect(sock, addr):
            if not isinstance(addr, tuple):
                blocked("unexpected socket type")
            address(addr[0], addr[1])
            return method(sock, addr)

        return connect

    getaddrinfo = socket.getaddrinfo

    def lookup(host, service, *args, **kwargs):
        address(host, service)
        return getaddrinfo(host, service, *args, **kwargs)

    with ExitStack() as stack:
        stack.enter_context(patch.object(socket, "getaddrinfo", lookup))
        for name in ("connect", "connect_ex"):
            stack.enter_context(
                patch.object(
                    socket.socket, name, connected(getattr(socket.socket, name))
                )
            )
        yield


@contextmanager
def local_sdks(replay, port):
    def redirected(method):
        def send(client, request, *args, **kwargs):
            _, provider, _ = replay.active
            if request.url.host not in HOSTS[provider]:
                reason = f"unexpected {provider} API host {request.url.host}"
                replay.errors.append(f"{replay.active}: {reason}")
                raise RuntimeError(reason)
            request.url = request.url.copy_with(scheme="http", host=LOOPBACK, port=port)
            request.headers["Host"] = f"{LOOPBACK}:{port}"
            return method(client, request, *args, **kwargs)

        return send

    with local_sockets(replay, port), ExitStack() as stack:
        import httpx
        import httpx2
        from google.genai import _api_client

        for lib in (httpx, httpx2):
            for cls in (lib.Client, lib.AsyncClient):
                stack.enter_context(patch.object(cls, "send", redirected(cls.send)))
        # Keep Google's async transport on its supported httpx path for redirection.
        stack.enter_context(
            patch.object(_api_client.BaseApiClient, "_use_aiohttp", return_value=False)
        )
        yield


def test_environment():
    allowed = ("PATH", "SYSTEMROOT", "WINDIR", "LANG", "LC_ALL", "TMPDIR", "TEMP")
    return {
        **{k: os.environ[k] for k in allowed if k in os.environ},
        "PYTHON_DOTENV_DISABLED": "1",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1",
        "NETRC": os.devnull,
    }


def rust_tests(pairs, replay, binary, port):
    env = {**test_environment(), "INSTRUCTOR_REPLAY_PORT": str(port)}
    passed = set()
    for provider, name in pairs:
        replay.active = ("rust", provider, name)
        run = subprocess.run(
            [binary, "--ignored", "--exact", name],
            env=env,
            text=True,
            capture_output=True,
            timeout=60,
        )
        if run.returncode == 0 and "1 passed; 0 failed" in run.stdout:
            passed.add(name)
        else:
            print(run.stdout, run.stderr, file=sys.stderr)
    return passed


def python_tests(pairs, replay, reference, port):
    os.chdir(reference)
    sys.path.insert(0, str(reference))
    for provider in PROVIDERS:
        os.environ[key_name(provider)] = KEY
    os.environ["GOOGLE_GENAI_MODEL"] = "google/gemini-3.8-flash"
    with local_sdks(replay, port):
        import instructor
        import pytest

        if not Path(instructor.__file__).resolve().is_relative_to(reference):
            raise RuntimeError("Python imported a different Instructor checkout")
        args = [
            "-q",
            "--tb=short",
            "-p",
            "pytest_asyncio.plugin",
            "--asyncio-mode=auto",
            *pairs.values(),
        ]
        if any("::" not in name for _, name in pairs):
            args.append("--run-decisions-live")
        return pytest.main(args, plugins=[replay])


def revision(path):
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=path, text=True
    ).strip()


def compare_requests(reference, case, rust, python):
    if len(rust) != expected_attempts(case):
        return "different"
    if python == rust:
        return "matched"
    # google-genai 2.8.0 validates once before Instructor. This reference
    # counts validator calls, so its first response can satisfy both attempts.
    if (
        reference == GOOGLE_RETRY_REVISION
        and case == GOOGLE_RETRY_TEST
        and python == rust[:1]
    ):
        return "known difference"
    return "different"


def isolate_network():
    """Optionally put the worker and its Rust children on a Linux network with lo only."""
    if sys.platform != "linux":
        raise RuntimeError("--isolate-network requires Linux")
    import ctypes
    import fcntl
    import struct

    libc = ctypes.CDLL(None, use_errno=True)
    clone_newuser, clone_newnet = 0x10000000, 0x40000000
    if libc.unshare(clone_newuser | clone_newnet) != 0:
        raise OSError(
            ctypes.get_errno(), "could not create user and network namespaces"
        )
    siocsifflags, iff_up = 0x8914, 1
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        fcntl.ioctl(sock, siocsifflags, struct.pack("16sH", b"lo", iff_up) + bytes(22))
    lines = Path("/proc/net/dev").read_text().splitlines()[2:]
    if [line.split(":", 1)[0].strip() for line in lines] != ["lo"]:
        raise RuntimeError("expected loopback to be the only network interface")


def run(args):
    if args.isolate_network:
        isolate_network()
    selected = PROVIDERS if args.provider == "all" else [args.provider]
    pairs = {
        (provider, name): node
        for provider in selected
        for name, node in cases(provider).items()
    }
    replay = Replay(pairs)
    with serve(replay) as port:
        passed = rust_tests(pairs, replay, args.binary, port)
        status = python_tests(pairs, replay, args.python_repo, port)
    rows = []
    for (provider, name), selector in pairs.items():
        rust = replay.traces["rust", provider, name]
        python = replay.traces["python", provider, name]
        comparison = compare_requests(args.python_revision, name, rust, python)
        python_status = replay.outcomes.get(selector, "missing")
        assertions = name in passed and python_status == "passed"
        print(
            f"{name}: assertions {'passed' if assertions else 'failed'}, requests "
            f"{comparison} ({len(rust)} Rust / {len(python)} Python)"
        )
        rows.append(
            {
                "provider": provider,
                "rust": name,
                "python": selector,
                "rust_passed": name in passed,
                "python_status": python_status,
                "request_comparison": comparison,
                "rust_requests": rust,
                "python_requests": python,
            }
        )
    for message in replay.errors:
        print(message, file=sys.stderr)
    if args.output:
        report = {
            "execution": "loopback replay",
            "isolated_network": args.isolate_network,
            "rust_revision": revision(ROOT),
            "python_revision": args.python_revision,
            "pairs": rows,
            "errors": replay.errors,
        }
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    count = sum(row["rust_passed"] and row["python_status"] == "passed" for row in rows)
    matches = sum(row["request_comparison"] == "matched" for row in rows)
    known = sum(row["request_comparison"] == "known difference" for row in rows)
    print(
        f"{count}/{len(rows)} assertion pairs passed, {matches}/{len(rows)} request traces matched"
    )
    if known:
        print(
            "Known Gemini retry difference in the pinned test. See docs/ci-credentials.md."
        )
    accepted = matches if args.strict else matches + known
    return int(
        bool(
            status
            or not rows
            or count != len(rows)
            or accepted != len(rows)
            or replay.errors
        )
    )


def build():
    env = {
        **test_environment(),
        "INSTRUCTOR_BUILD_REPLAY": "1",
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for k in (
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_PROFILE_TEST_DEBUG",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_BUILD_JOBS",
    ):
        if k in os.environ:
            env[k] = os.environ[k]
    if "CARGO_HOME" not in env:
        env["CARGO_HOME"] = str(Path.home() / ".cargo")
    if "RUSTUP_HOME" not in env:
        env["RUSTUP_HOME"] = str(Path.home() / ".rustup")
    cmd = [
        "cargo",
        "test",
        "--offline",
        "--locked",
        "-p",
        "instructor-ai",
        "--test",
        "live",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]
    for line in subprocess.check_output(cmd, env=env, cwd=ROOT, text=True).splitlines():
        event = json.loads(line)
        if event.get("executable") and event["target"]["name"] == "live":
            return event["executable"]
    raise RuntimeError("Missing Rust replay binary")


def check_reference(reference, requested):
    if requested is None:
        workflow = (ROOT / ".github/workflows/paired-live.yml").read_text()
        pinned = re.search(
            r"^  PYTHON_REVISION: ([0-9a-f]{40})$", workflow, re.MULTILINE
        )
        if not pinned:
            raise ValueError("Missing Python reference pin in paired-live.yml")
        requested = pinned[1]
    if requested != revision(reference):
        raise ValueError(f"Expected the Python checkout at commit {requested}")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=reference):
        raise ValueError(
            "The Python checkout must be clean to identify the tested commit"
        )
    return requested


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-repo", required=True, type=Path)
    parser.add_argument(
        "--python-revision",
        help="Override the paired CI pin for this replay with a full commit SHA",
    )
    parser.add_argument("--provider", choices=["all", *PROVIDERS], default="all")
    parser.add_argument("--output", type=Path, help="JSON report for each named pair")
    parser.add_argument(
        "--strict", action="store_true", help="Fail on known request differences too"
    )
    parser.add_argument(
        "--isolate-network",
        action="store_true",
        help="Require a Linux network namespace with only loopback during replay",
    )
    parser.add_argument("--binary", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.python_repo = args.python_repo.resolve()
    if args.output:
        args.output = args.output.resolve()
    try:
        args.python_revision = check_reference(args.python_repo, args.python_revision)
    except ValueError as exc:
        parser.error(str(exc))
    if args.binary:
        return run(args)
    binary = build()
    with TemporaryDirectory() as config:
        env = {
            **test_environment(),
            "XDG_CONFIG_HOME": config,
            "XDG_CACHE_HOME": config,
        }
        return subprocess.call(
            [sys.executable, "-I", "-B", __file__, *sys.argv[1:], "--binary", binary],
            env=env,
        )


if __name__ == "__main__":
    sys.exit(main())
