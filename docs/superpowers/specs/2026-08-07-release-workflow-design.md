# Release Workflow Design

## Goal

Add a manually triggered GitHub Actions workflow that builds the Slash server
Docker image, publishes it to GitHub Container Registry, and tags the released
commit after the image is available.

## Trigger and Permissions

The workflow uses `workflow_dispatch` without inputs. It releases the commit
selected when the workflow is dispatched.

The job has only the permissions it needs:

- `contents: write` to push the Git tag.
- `packages: write` to push the container image to GHCR.

Concurrent releases are serialized to prevent two runs from racing to publish
the same version.

## Version and Names

The workflow obtains the `slash-server` package version through
`cargo metadata`, which resolves `version.workspace = true` from the root
`Cargo.toml` without parsing TOML as text.

For a workspace version such as `0.1.2`, the release publishes:

- `ghcr.io/<lowercase-owner>/<lowercase-repository>:0.1.2`
- `ghcr.io/<lowercase-owner>/<lowercase-repository>:latest`
- Git tag `v0.1.2`

The workflow fails before building if the Git tag already exists on the
remote. Existing tags are never moved or overwritten.

## Release Flow

1. Check out the selected commit with full Git history and tags.
2. Resolve and validate the `slash-server` version and derived names.
3. Verify that the version tag does not exist on the remote.
4. Authenticate to GHCR with the workflow `GITHUB_TOKEN`.
5. Build the repository `Dockerfile` with BuildKit and push both image tags.
6. Create an annotated version tag on the selected commit and push it.

The image build uses GitHub Actions cache storage. The workflow does not
create a GitHub Release because only image publication and Git tagging are in
scope.

## Failure Semantics

No Git tag is created unless the image build and push succeed. A failure while
pushing the Git tag can leave a published image without a corresponding Git
tag because GHCR and Git do not provide a shared transaction. Rerunning after
fixing repository permissions safely republishes the same image content before
retrying the tag operation.

## Validation

Validate the workflow syntax, confirm `cargo metadata` resolves exactly one
`slash-server` version, and build the Docker image locally. The live GHCR push
and Git tag creation require GitHub Actions credentials and are verified only
when the workflow is manually dispatched.
