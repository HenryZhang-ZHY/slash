Feature: Run a fake CD release command with an argument from a pull request

  Scenario Outline: The fake CD workflow receives the requested release type
    Given a GitHub repository has Slash installed
    And the command "fake-cd-release" is configured in its ".slash" directory
    When a user opens a pull request
    And comments "/fake-cd-release <release_type>" on the pull request
    Then the Slash server receives the GitHub webhook
    And Slash passes "<release_type>" as the "release_type" workflow input
    And Slash triggers the configured GitHub Actions workflow
    And Slash continuously syncs the GitHub Actions status to the pull request Checks page
    And the check concludes with "success"

    Examples:
      | release_type |
      | preview      |
      | prerelease   |