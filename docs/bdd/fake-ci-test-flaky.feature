Feature: Run a fake flaky CI command that exercises the Test Engine dogfood loop

  The Test Engine closed loop (docs/design/1.0-test-engine.md §5) dogfooded on
  the slash repo itself: a fake *flaky* test is ingested, auto-quarantined by
  the flaky detector, and then skipped/soft-failed by the disposal hook so it
  stops flipping a PR's required check. "Ready to wire": needs a Test Engine
  deployment (upload + quarantined endpoints, per-suite token) — when absent
  the workflow degrades to running the fake test without uploading.

  Scenario: A deterministic flaky test is quarantined then stops failing PRs
    Given a GitHub repository has Slash installed
    And the command "fake-ci-test-flaky" is configured in its ".slash" directory
    And a Test Engine deployment is wired (upload + quarantined endpoints + suite token)
    When a user opens a pull request
    And comments "/fake-ci-test-flaky" on the pull request
    Then the Slash server receives the GitHub webhook
    And Slash triggers the configured GitHub Actions workflow
    And the workflow runs the deterministic fake flaky test
    When the test fails this run (odd run number)
    Then the workflow uploads the failed execution to the Test Engine upload endpoint
    And the check concludes with "failure"

    # The upload trains the flaky detector; after >=3 executions with a
    # fail->pass recovery it mutes the test (default disposition per §8 Q1).
    When enough runs have produced a fail->pass recovery
    Then the flaky detector marks "tests::demo_flaky" as muted

    # On the next invocation the workflow consults the quarantined endpoint and
    # soft-fails or skips the muted test instead of blocking the PR (bktec
    # "skip/mute flaky" behavior, server-side).
    When the user re-runs the command
    Then the workflow reads the quarantined list from the disposal endpoint
    And "tests::demo_flaky" is reported as quarantined
    And the workflow does not fail the pull request on it (soft-fail)
    And the check concludes with "success" (or a non-blocking neutral)

  Scenario: A healthy control test is never quarantined
    Given a Test Engine deployment is wired
    And the workflow has uploaded repeated passing runs of "tests::demo_steady"
    When the flaky detector reconciles
    Then "tests::demo_steady" remains in state "enabled"
