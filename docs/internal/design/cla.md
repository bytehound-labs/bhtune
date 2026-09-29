# Contributor License Agreement

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Contributor License Agreement (`cla-tooling`)

`CLA.md` is version 1.0 and in force; the earlier DRAFT banner is gone. Four things shaped it:

- **The commercial disclosure is the point, not a footnote.** BHTune stays AGPL, but ByteHound
  Corp. may also sell a commercial/enterprise license, and a contribution may end up inside what
  is sold. Section 3's copyright grant is what makes that legally possible, so the agreement says
  so in plain language up front ("Commercial licensing — please read before signing"), together
  with the limits that run the other way: contributors are not paid, gain no revenue claim, keep
  their own copyright, and cannot have the AGPL retracted from already-released code. A
  contributor who objects is told to walk away rather than discovering the term later. By
  explicit decision this disclosure lives **only** in `CLA.md` — `README.md` and
  `CONTRIBUTING.md` link to it without restating commercial terms — and the failing check's job
  summary links straight to it, which is the moment a contributor is actually about to sign.
- **Clauses added beyond the old draft**, closing gaps from the earlier pre-legal punch-list:
  patent-litigation termination (5), third-party material disclosure (7), employer/corporate
  authorization (8), no-obligation-to-use (10), trademark disclaimer (11), Alberta governing law
  (13), and a severability/non-retroactivity clause (14). Still outstanding: a real legal review.
- **Enforcement is `.github/workflows/cla.yml`, written in-house with no third-party action.**
  An earlier draft used `contributor-assistant/github-action`, which required `pull_request_target`
  and was archived upstream in March 2026. The repo's own `security-lint.yml` gate rejected both:
  zizmor raised `dangerous-triggers` (medium) for the trigger and `archived-uses` (**high**) for
  the action. Rather than suppress a high-confidence finding from the project's own security
  audit, both causes were removed. The workflow now splits into two jobs so no privileged trigger
  is needed:
  - `check` runs on plain `pull_request`. A fork PR's read-only `GITHUB_TOKEN` is _sufficient_
    here, because the job only needs to read the signature file and fail; a failing required
    check is what blocks the merge, so it needs no write access at all. Instructions and the
    exact signing phrase are written to `$GITHUB_STEP_SUMMARY`, replacing the bot comment.
  - `sign` runs on `issue_comment`, which always executes the workflow from the default branch
    and never checks out pull request code — so holding `contents: write` there is safe. It
    records the signature through the Contents API, reacts to the comment, and re-runs the failed
    `check` run so the contributor does not have to push again.

  Logic is plain `bash` + `jq` + the preinstalled `gh` CLI, so the supply chain for a job holding
  `contents: write` is GitHub's own tooling and nothing else. There is no `actions/checkout`
  anywhere in the file, and every value taken from the event payload is passed through `env:`
  rather than interpolated into a `run:` script, which is what keeps zizmor's `template-injection`
  audit quiet. Signature storage cannot live on `main`, which is protected with
  `enforce_admins: true` and requires pull requests — a bot push would simply be rejected — so
  signatures go to a dedicated orphan `cla-signatures` branch. `concurrency` uses
  `cancel-in-progress: false`, since cancelling mid-run can lose a signature a contributor has
  already posted. Maintainers (`mikeboiko`, `Copilot`) and any account of type `Bot` are exempt.
  Verified locally against the same tool versions CI pins: `actionlint` v1.7.7 clean, and
  `zizmor` 1.29.0 reporting **no findings** both offline and online, restoring the repo's clean
  baseline.
