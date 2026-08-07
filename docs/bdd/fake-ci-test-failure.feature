Feature: Run a failing fake CI command from a pull request

  Scenario: The fake CI workflow fails
    Given a GitHub repository has Slash installed
    And the command "fake-ci-test-failure" is configured in its ".slash" directory
    When a user opens a pull request
    And comments "/fake-ci-test-failure" on the pull request
    Then the Slash server receives the GitHub webhook
    And Slash triggers the configured GitHub Actions workflow
    And Slash continuously syncs the GitHub Actions status to the pull request Checks page
    And the check concludes with "failure"
