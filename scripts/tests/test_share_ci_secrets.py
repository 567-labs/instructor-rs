from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
from subprocess import CompletedProcess
import unittest
from unittest.mock import patch

from scripts import share_ci_secrets as audit


KEY = "OPENAI_API_KEY"
PY, RS = audit.REPOS


class SecretSharingTests(unittest.TestCase):
    def setUp(self):
        self.org = {k: "selected" for k in audit.NAMES if k not in audit.OPTIONAL}
        self.recipients = {k: {PY, RS, "existing-app"} for k in self.org}
        self.own = {PY: set(), RS: set()}
        self.ids = {PY: 111, RS: 222}
        self.writes = []
        self.denied = None
        self.drop_page = False
        self.ignore_write = False
        proc = patch.object(audit.subprocess, "run", self.fake_gh)
        proc.start()
        self.addCleanup(proc.stop)

    def fake_gh(self, args, **kw):
        self.assertEqual(
            args[:5], ["gh", "api", "--hostname", "github.com", "--method"]
        )
        method, path = args[5], args[6].partition("?")[0]
        if (method, path) == self.denied:
            return CompletedProcess(args, 1, "", "untrusted response (HTTP 403)")
        if method == "PUT":
            self.assertEqual(len(args), 7)
            self.assertTrue(path.startswith(audit.SECRETS + "/"))
            name, resource, rid = path.removeprefix(audit.SECRETS + "/").split("/")
            self.assertEqual(resource, "repositories")
            self.assertIn(name, audit.NAMES)
            self.assertEqual(self.org[name], "selected")
            repo = {str(v): k for k, v in self.ids.items()}[rid]
            self.writes.append((name, repo))
            if not self.ignore_write:
                self.recipients[name].add(repo)
            return CompletedProcess(args, 0, "", "")
        self.assertEqual(method, "GET")
        self.assertIn("--jq", args)
        if path == audit.SECRETS:
            entries = list(self.org.items())
        else:
            prefix = f"repos/{audit.ORG}/"
            self.assertTrue(path.startswith(prefix))
            repo, _, suffix = path.removeprefix(prefix).partition("/")
            if not suffix:
                data = [f"{audit.ORG}/{repo}", self.ids[repo]]
                return CompletedProcess(args, 0, json.dumps(data), "")
            if suffix == "actions/organization-secrets":
                entries = [
                    (k, None) for k, repos in self.recipients.items() if repo in repos
                ]
            else:
                self.assertEqual(suffix, "actions/secrets")
                entries = [(k, None) for k in self.own[repo]]
        self.assertIn("--paginate", args)
        pages = [entries[:2]]
        if len(entries) > 2 and not self.drop_page:
            pages.append(entries[2:])
        stdout = "\n".join(
            json.dumps({"count": len(entries), "entries": p}) for p in pages
        )
        return CompletedProcess(args, 0, stdout, "")

    def invoke(self, *args):
        text = StringIO()
        with redirect_stdout(text), redirect_stderr(text):
            code = audit.main(args)
        return code, text.getvalue()

    def test_audit_then_grant_preserves_recipients_and_is_idempotent(self):
        self.recipients[KEY].remove(RS)
        code, text = self.invoke()
        self.assertEqual(code, 1)
        self.assertIn("grant needed for " + RS, text)
        self.assertEqual(self.writes, [])
        self.assertEqual(self.invoke("--apply")[0], 0)
        self.assertEqual(self.invoke("--apply")[0], 0)
        self.assertEqual(self.writes, [(KEY, RS)])
        self.assertEqual(self.recipients[KEY], {PY, RS, "existing-app"})

    def test_optional_and_missing(self):
        self.assertEqual(self.invoke()[0], 0)
        self.assertIn("optional, absent", self.invoke()[1])
        self.assertEqual(self.invoke("--secret", "GOOGLE_GENAI_MODEL")[0], 1)
        del self.org[KEY]
        self.assertEqual(self.invoke("--apply")[0], 1)
        self.assertEqual(self.writes, [])

    def test_explicit_scope_only(self):
        self.recipients[KEY] = {"existing-app"}
        self.org["OTHER_KEY"] = "selected"
        self.recipients["OTHER_KEY"] = {PY}
        self.assertEqual(self.invoke("--apply")[0], 1)
        self.assertEqual(self.writes, [])
        self.assertEqual(self.invoke("--secret", KEY, "--apply")[0], 0)
        self.assertEqual(self.writes, [(KEY, PY), (KEY, RS)])
        with self.assertRaises(SystemExit):
            self.invoke("--apply", "--secret", "OTHER_KEY")

    def test_visibility_uses_github_access_and_never_switches_policy(self):
        for visibility in ["private", "all"]:
            self.org[KEY] = visibility
            self.recipients[KEY] = {PY, RS}
            self.assertEqual(self.invoke("--apply")[0], 0)
            self.recipients[KEY] = {RS}
            self.assertEqual(self.invoke("--apply")[0], 1)
        self.org[KEY] = "unexpected"
        self.assertEqual(self.invoke("--apply")[0], 1)
        self.assertEqual(self.writes, [])

    def test_repository_override_blocks_verified_status(self):
        self.own[RS].add(KEY.lower())
        code, text = self.invoke("--secret", KEY, "--apply")
        self.assertEqual(code, 1)
        self.assertIn("repository override", text)
        self.assertNotIn("shared", text)
        self.assertEqual(self.writes, [])
        del self.org[KEY]
        self.assertIn("repository secret only", self.invoke("--secret", KEY)[1])

    def test_denied_preflight_prevents_mutations(self):
        self.denied = ("GET", f"repos/{audit.ORG}/{RS}/actions/secrets")
        self.recipients[KEY].remove(RS)
        code, text = self.invoke("--apply")
        self.assertEqual(code, 2)
        self.assertIn("HTTP 403", text)
        self.assertNotIn("untrusted", text)
        self.assertNotIn("shared", text)
        self.assertEqual(self.writes, [])

    def test_pagination_and_actions_limit(self):
        self.drop_page = True
        self.assertEqual(self.invoke("--apply")[0], 2)
        self.drop_page = False
        for i in range(100):
            self.recipients[f"A{i:03d}"] = {RS}
        code, text = self.invoke("--secret", KEY)
        self.assertEqual(code, 1)
        self.assertIn("outside the first 100", text)

    def test_verify_effect_of_apply(self):
        self.recipients[KEY].remove(PY)
        self.ignore_write = True
        code, text = self.invoke("--secret", KEY, "--apply")
        self.assertEqual(code, 1)
        self.assertNotIn("shared", text)
        self.assertEqual(self.writes, [(KEY, PY)])

    def test_grant_preserves_effective_access_at_limit(self):
        self.recipients[KEY].remove(RS)
        for i in range(100):
            self.recipients[f"Z{i:03d}"] = {RS}
        code, text = self.invoke("--apply", "--secret", KEY)
        self.assertEqual(code, 1)
        self.assertIn("review first-100 limit", text)
        self.assertEqual(self.writes, [])

    def test_failed_grant_can_resume(self):
        self.recipients[KEY] = {"existing-app"}
        self.denied = ("PUT", f"{audit.SECRETS}/{KEY}/repositories/{self.ids[RS]}")
        code, text = self.invoke("--apply", "--secret", KEY)
        self.assertEqual(code, 2)
        self.assertNotIn("shared", text)
        self.assertEqual(self.recipients[KEY], {"existing-app", PY})
        self.denied = None
        self.assertEqual(self.invoke("--apply", "--secret", KEY)[0], 0)
        self.assertEqual(self.writes, [(KEY, PY), (KEY, RS)])


if __name__ == "__main__":
    unittest.main()
