# Share provider credentials across CI

An organization Actions secret lets `567-labs/instructor` and `567-labs/instructor-rs` use the same credential. Rotation then updates one secret for both workflows. The administrator audit checks the organization policy, the grants visible to each repository and any repository secrets that take precedence. Its output contains names and statuses. GitHub exposes secret metadata and restricts access to stored values. [GitHub Actions secrets API](https://docs.github.com/en/rest/actions/secrets)

| Provider | Shared secret |
| --- | --- |
| OpenAI, including native decisions | `OPENAI_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenRouter, including routed Jev | `OPENROUTER_API_KEY` |
| Google Gemini | `GOOGLE_API_KEY` |
| Cohere | `COHERE_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| Direct Jev | `TYPESAFE_API_KEY` |

The audit also checks the optional `GOOGLE_GENAI_MODEL` setting when present. The seven keys together cover all provider routes. Use `--secret NAME` to audit an individual route.

## Check access

Run the helper from the Rust checkout with Python 3 and an authenticated GitHub CLI. For a fine-grained credential, choose `567-labs` as the resource owner and select both repositories. The audit needs organization **Secrets** read access and repository **Secrets** and **Metadata** read access. Applying grants requires organization **Secrets** write access. Classic tokens require `admin:org` and `repo` scopes for the combined audit. [Required permissions](https://docs.github.com/en/rest/actions/secrets#add-selected-repository-to-an-organization-secret)

```sh
python scripts/share_ci_secrets.py
python scripts/share_ci_secrets.py --secret OPENAI_API_KEY
```

Exit 0 means the requested names resolve to organization secrets in both repositories, allowing an absent optional setting in the default audit. Exit 1 identifies a missing grant, override or policy requiring review. Exit 2 means the audit could not complete. During implementation GitHub returned HTTP 403, `Resource not accessible by integration`, for the organization and both repositories' secret lists. Current sharing remains unverified until an authorized administrator runs the audit.

## Grant a selected secret

Review the reported names and their intended use. For an existing **Selected repositories** secret, run the following to add the missing Instructor repository. Repeat `--secret` to include other names. Naming a secret explicitly also authorizes adding both repositories when neither currently has access.

```sh
python scripts/share_ci_secrets.py --apply --secret OPENAI_API_KEY
python scripts/share_ci_secrets.py --secret OPENAI_API_KEY
```

With `--apply` alone the helper considers only the listed secrets already assigned to at least one Instructor repository. It adds individual repository IDs, preserving existing recipients, and re-reads the metadata. Re-running after an interrupted grant picks up the missing recipients. Existing **All repositories** and **Private repositories** policies are audited according to GitHub's effective grants. The helper flags the Actions limit of 100 accessible organization secrets when a grant could displace a secret.

## Resolve repository overrides

1. For a repository-only secret, retrieve the authoritative value from the original vault. GitHub's API cannot copy a stored secret value between repositories. In the organization's Actions settings, create the secret with the exact name above and choose **Selected repositories**. Select both Instructor repositories. When editing an existing grant, retain its other approved recipients. [Organization Actions settings](https://github.com/organizations/567-labs/settings/secrets/actions)
2. A repository secret overrides an organization secret of the same name. A job environment secret takes precedence over both. The helper reports repository overrides and defers changes for those names. An administrator should review the affected workflows, confirm the organization grant and credential, then resolve the override in repository settings. Check any named job environments separately. [Secret precedence and limits](https://docs.github.com/en/actions/reference/security/secrets)
3. Re-run the audit, then run the trusted live provider checks in both repositories. Inspect which routes actually executed. The metadata audit establishes credential scope, and the live results establish provider access and model availability.

## Test the helper

The tests use a mocked `gh api` transport and require only the Python standard library.

```sh
python -m unittest discover -s scripts/tests
```
