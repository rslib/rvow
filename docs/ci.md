# How to carry accepted checks through CI

Use this flow for a home-mode project whose store is not available in CI, such as a GitHub Actions job.
Use the same rvow version on both machines.

1. On the accepted branch, export its checks outside the checkout.

   ```sh
   rvow bundle --out /tmp/rvow-bundle.json
   ```

   Export refuses pending decisions, blocking plan drift and invalid required signatures.
   Keep credentials in the CI environment, not in literal commands or `rerun_env` values.

2. Transfer the bundle to the matching CI checkout.
   Fetch complete Git history and retain the same origin identity.
   Treat the bundle as trusted input because it executes the stored commands.

3. Run the checks and save their results outside the checkout.

   ```sh
   rvow gate --bundle /tmp/rvow-bundle.json --out /tmp/rvow-runs.jsonl
   ```

   The command needs no home store and creates no accepted local plan.
   It runs configured setup and checks and records the environment with rerun overrides applied.
   A failed check exits nonzero and still writes its failed task results.
   Configure CI to retain the result artifact even when the job fails.

4. Download the trusted result file where the home store is available.

   ```sh
   rvow ingest /tmp/rvow-runs.jsonl
   rvow status
   ```

   Results retain their task, requirement, input and environment identities.
   Different local inputs or environments leave the imported proof stale.
   Saved test objects travel with successful results and are checked before import.
   A result file can record proofs, not accept or confirm plan changes.

GitHub Actions, GitLab CI, CircleCI, Buildkite, Azure Pipelines and Jenkins retain provider run identities.
Other runners that set `CI=true` are recorded as generic CI.
The result format is not a cryptographic CI attestation, so do not ingest files from an untrusted source.
