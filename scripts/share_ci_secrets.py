"""Audit shared GitHub Actions provider secrets for the two Instructor repositories."""

import argparse
from dataclasses import dataclass
import json
import re
import subprocess
import sys

ORG = "567-labs"
REPOS = ("instructor", "instructor-rs")
SECRETS = f"orgs/{ORG}/actions/secrets"
NAMES = (
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_MODEL",
    "COHERE_API_KEY",
    "MISTRAL_API_KEY",
    "TYPESAFE_API_KEY",
)
OPTIONAL = {"GOOGLE_GENAI_MODEL"}


def api(method, path, *flags):
    cmd = ["gh", "api", "--hostname", "github.com", "--method", method, path, *flags]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    except OSError as exc:
        raise RuntimeError("Could not run gh") from exc
    if result.returncode:
        status = re.search(r"HTTP \d{3}", result.stderr)
        reason = status.group() if status else f"gh exit {result.returncode}"
        raise RuntimeError(f"{method} {path} failed ({reason})")
    return result.stdout


def list_secrets(path):
    # gh applies the projection to each page and emits one line per page.
    pages = api(
        "GET",
        path + "?per_page=100",
        "--paginate",
        "--jq",
        "{count: .total_count, entries: [.secrets[] | [.name, .visibility]]} | @json",
    )
    counts = set()
    secrets = {}
    for line in pages.splitlines():
        data = json.loads(line)
        counts.add(data["count"])
        for name, visibility in data["entries"]:
            if not isinstance(name, str) or name.upper() in secrets:
                raise RuntimeError(f"Inconsistent secret list at {path}")
            secrets[name.upper()] = visibility
    if counts != {len(secrets)}:
        raise RuntimeError(f"Incomplete secret list at {path}")
    return secrets


@dataclass
class Repository:
    id: int
    own: set
    granted: set
    available: set


def snapshot():
    org = list_secrets(SECRETS)
    repos = {}
    for name in REPOS:
        full = f"{ORG}/{name}"
        path = f"repos/{full}"
        actual, repo_id = json.loads(
            api("GET", path, "--jq", "[.full_name, .id] | @json")
        )
        if actual != full or type(repo_id) is not int or repo_id <= 0:
            raise RuntimeError(f"Repository identity mismatch for {full}")
        own = set(list_secrets(f"{path}/actions/secrets"))
        # GitHub computes access for all, private and selected policies.
        granted = set(list_secrets(f"{path}/actions/organization-secrets"))
        # Actions reads the first 100 available organization secrets alphabetically.
        repos[name] = Repository(repo_id, own, granted, set(sorted(granted)[:100]))
    return org, repos


def check(name, explicit, org, repos):
    """Return the status, whether it passes the audit, and eligible grant targets."""
    shadows = [r for r in REPOS if name in repos[r].own]
    if shadows:
        kind = "repository override" if name in org else "repository secret only"
        return f"{kind} in {', '.join(shadows)}", False, []
    if name not in org:
        if name in OPTIONAL and not explicit:
            return "optional, absent", True, []
        return "missing organization secret", False, []
    visibility = org[name]
    if visibility not in ("selected", "all", "private"):
        return "unknown visibility", False, []
    if any(name in repo.granted - repo.available for repo in repos.values()):
        return "outside the first 100 organization secrets", False, []
    missing = [r for r in REPOS if name not in repos[r].granted]
    if not missing:
        return f"shared ({visibility})", True, []
    if visibility != "selected":
        return f"policy {visibility} excludes {', '.join(missing)}", False, []
    if len(missing) == len(REPOS) and not explicit:
        return "selected for neither repository, review scope", False, []
    full = [r for r in missing if len(repos[r].granted) >= 100]
    if full:
        return f"review first-100 limit in {', '.join(full)}", False, []
    return f"grant needed for {', '.join(missing)}", False, missing


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--apply", action="store_true", help="Add missing selected-repository grants"
    )
    parser.add_argument(
        "--secret",
        action="append",
        choices=NAMES,
        metavar="NAME",
        help="Limit scope to NAME. Repeat for more names. With --apply, authorize NAME even if neither repo has access.",
    )
    args = parser.parse_args(argv)
    names = tuple(dict.fromkeys(args.secret or NAMES))
    explicit = bool(args.secret)
    try:
        org, repos = snapshot()
        if args.apply:
            changed = False
            for name in names:
                _, _, targets = check(name, explicit, org, repos)
                for repo in targets:
                    api("PUT", f"{SECRETS}/{name}/repositories/{repos[repo].id}")
                    repos[repo].granted.add(name)
                    print(f"Granted {name} to {ORG}/{repo}")
                    changed = True
            if changed:
                org, repos = snapshot()
        ok = True
        for name in names:
            status, passed, _ = check(name, explicit, org, repos)
            print(f"{name}  {status}")
            ok = ok and passed
        return 0 if ok else 1
    except RuntimeError as exc:
        print(f"Audit incomplete. {exc}", file=sys.stderr)
    except (ValueError, KeyError, TypeError):
        print("Audit incomplete. Unexpected GitHub metadata", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
