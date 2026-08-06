# Manages the trigger settings for the "slash" Buildkite pipeline, so that
# CI runs only when someone comments the command word on a PR, not on every
# push — the Prow-style /test flow. This is an existing pipeline (created
# when Buildkite was connected to this repo), so it must be imported once
# before the settings below take effect; see the workflow below.
#
# Auth: set BUILDKITE_API_TOKEN (scopes: graphql, read_pipelines,
# write_pipelines) and BUILDKITE_ORGANIZATION_SLUG as environment variables.
# Never put the token in this file.
#
# One-time setup:
#   1. terraform init
#   2. Get the pipeline's GraphQL ID from its Buildkite settings page (URL
#      or page source), then:
#        terraform import buildkite_pipeline.slash <graphql-id>
#   3. terraform plan
#      The plan will likely show changes beyond the ones below — anything
#      already configured on the real pipeline that isn't listed in this
#      file's `provider_settings` block (e.g. build_pull_request_forks,
#      separate_pull_request_statuses) reads as "unset" here and Terraform
#      will plan to reset it to Buildkite's default. provider_settings'
#      fields are plain optional attributes, not computed/preserved — so
#      before applying, run `terraform show buildkite_pipeline.slash` and
#      copy every currently-set field you want to keep into the block below
#      verbatim. Once `terraform plan` shows only the intended trigger
#      changes, apply.
#   4. terraform apply
#
# State: this config has no backend block, so state defaults to a local
# terraform.tfstate file — fine solo, but move to a shared/remote backend
# before more than one person runs apply against this pipeline.

terraform {
  required_version = ">= 1.11"

  required_providers {
    buildkite = {
      source  = "buildkite/buildkite"
      version = "~> 1.16"
    }
  }
}

provider "buildkite" {}

resource "buildkite_pipeline" "slash" {
  name = "slash" # confirm this matches the pipeline's actual name
  # Confirm this matches the exact string in `terraform show` after import —
  # Buildkite may have this stored as an HTTPS URL instead of SSH depending
  # on how the repo was originally connected.
  repository = "git@github.com:HenryZhang-ZHY/slash.git"

  provider_settings = {
    # Keep webhook-driven triggering on ("none" would also disable the
    # comment trigger below, not just push/PR).
    trigger_mode = "code"

    # No build on every push or PR update.
    build_branches      = false
    build_pull_requests = false
    build_tags          = false # flip to true later for tag/release builds

    # Build once per "/test" comment on a PR. Buildkite only honours this
    # from a trusted commenter (repo owner/member/collaborator, or a GitHub
    # account linked to a Buildkite user with build permission) — see
    # https://buildkite.com/docs/pipelines/source-control/github.
    build_issue_comment_created = true
    issue_comment_command_word  = "/test"
    issue_comment_match_mode    = "exact" # whole comment must be exactly "/test"; "contains" allows e.g. "/test please"

    # Required for the build's result to land back on the PR as a status check.
    publish_commit_status = true
  }

  lifecycle {
    # Only manage the trigger settings above — leave every other existing
    # attribute on this pipeline (cluster, team, cosmetics, steps source,
    # etc.) exactly as it already is.
    ignore_changes = [
      cluster_id, default_team_id, description, emoji, color, tags,
      visibility, steps, default_branch, branch_configuration,
      clone_mirror_url, pipeline_template_id, allow_rebuilds, archived,
      cancel_intermediate_builds, cancel_intermediate_builds_branch_filter,
      skip_intermediate_builds, skip_intermediate_builds_branch_filter,
      default_timeout_in_minutes, maximum_timeout_in_minutes,
    ]
  }
}
