# Release Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a manually triggered workflow that publishes the Slash server image to GHCR and tags the released commit.

**Architecture:** A single GitHub Actions job resolves the workspace version with `cargo metadata`, rejects an existing release tag, and uses Docker Buildx to publish versioned and `latest` image tags. Only after the image push succeeds does it create and push an annotated Git tag for the selected commit.

**Tech Stack:** GitHub Actions, Cargo metadata, Docker Buildx, GitHub Container Registry

---

### Task 1: Add the release workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Release

on:
  workflow_dispatch:

permissions:
  contents: write
  packages: write

concurrency:
  group: ${{ github.repository }}-release
  cancel-in-progress: false

jobs:
  release:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - name: Check out release commit
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Resolve release metadata
        id: metadata
        shell: bash
        run: |
          set -euo pipefail

          version="$(
            cargo metadata --no-deps --format-version 1 |
              jq -r '[.packages[] | select(.name == "slash-server") | .version] | if length == 1 then .[0] else empty end'
          )"

          if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
            echo "::error::Unable to resolve a Docker-compatible slash-server version"
            exit 1
          fi

          repository="${GITHUB_REPOSITORY,,}"
          echo "version=$version" >> "$GITHUB_OUTPUT"
          echo "tag=v$version" >> "$GITHUB_OUTPUT"
          echo "image=ghcr.io/$repository" >> "$GITHUB_OUTPUT"

      - name: Verify release tag is new
        shell: bash
        env:
          TAG: ${{ steps.metadata.outputs.tag }}
        run: |
          set +e
          git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1
          status=$?
          set -e

          case "$status" in
            0)
              echo "::error::Tag $TAG already exists"
              exit 1
              ;;
            2)
              ;;
            *)
              echo "::error::Unable to check whether tag $TAG exists"
              exit "$status"
              ;;
          esac

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Build and publish image
        uses: docker/build-push-action@v6
        with:
          context: .
          file: ./Dockerfile
          push: true
          tags: |
            ${{ steps.metadata.outputs.image }}:${{ steps.metadata.outputs.version }}
            ${{ steps.metadata.outputs.image }}:latest
          labels: |
            org.opencontainers.image.revision=${{ github.sha }}
            org.opencontainers.image.source=${{ github.server_url }}/${{ github.repository }}
            org.opencontainers.image.version=${{ steps.metadata.outputs.version }}
          cache-from: type=gha
          cache-to: type=gha,mode=max

      - name: Tag released commit
        shell: bash
        env:
          TAG: ${{ steps.metadata.outputs.tag }}
        run: |
          set -euo pipefail
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git tag --annotate "$TAG" "$GITHUB_SHA" --message "Release $TAG"
          git push origin "refs/tags/$TAG"
```

- [ ] **Step 2: Validate the workflow and metadata lookup**

Run:

```powershell
actionlint .github/workflows/release.yml
$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
($metadata.packages | Where-Object name -eq 'slash-server').version
```

Expected: `actionlint` exits successfully and the metadata command prints the
workspace version exactly once.

- [ ] **Step 3: Build the release image locally**

Run:

```powershell
docker build --tag slash-server:release-test .
```

Expected: Docker exits successfully after building the `slash-server` runtime
image.

- [ ] **Step 4: Review the focused diff**

Run:

```powershell
git diff --check -- .github/workflows/release.yml
git diff -- .github/workflows/release.yml
```

Expected: no whitespace errors; the diff contains only the release workflow.
