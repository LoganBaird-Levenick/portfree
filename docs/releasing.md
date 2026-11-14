# Releasing Portfree

Releases are cut from `main` with `cargo-release` and published by the tag-driven [release workflow](../.github/workflows/release.yml). The tag is the single source of release truth: `cargo-release` creates and pushes it, and the workflow must never create one.

The release tag convention is `portfree-v{version}`, for example `portfree-v1.0.0`. Artifacts are named `portfree-v{version}-x86_64-pc-windows-msvc.zip` and `portfree.exe`.

## Prerequisites

- `cargo-release` installed (`cargo install cargo-release --locked`).
- A clean `main` checkout with the latest `origin/main` and green CI.
- Permission to push `main` and tags, and to write releases.

## Rehearse with a dry run

A manual dispatch builds every release artifact, attests it, and leaves a draft release. It does not publish and does not create a tag. Draft releases do not create git tags, so the tag name is reserved only for the draft.

```powershell
gh workflow run release.yml --ref main -f version=1.0.0 -f dry_run=true
gh run watch
gh release view portfree-v1.0.0
```

Download the draft assets and run the same checks a user would:

```powershell
gh release download portfree-v1.0.0 --dir dry-run
Get-Content dry-run\SHA256SUMS
gh attestation verify dry-run\portfree-v1.0.0-x86_64-pc-windows-msvc.zip --repo LoganBaird-Levenick/portfree
gh attestation verify dry-run\portfree.exe --repo LoganBaird-Levenick/portfree
```

Inspect the zip. It must contain exactly `portfree-v1.0.0-x86_64-pc-windows-msvc\portfree.exe`, `README.md`, and `LICENSE`, and must not contain `portfree-test-listener.exe`.

Delete the draft when the inspection is done. Do not use `--cleanup-tag`: a draft does not create a tag.

```powershell
gh release delete portfree-v1.0.0 --yes
git ls-remote --tags origin portfree-v1.0.0
```

## Cut the release

From a clean `main`:

```powershell
cargo release patch --execute
```

`patch`, `minor`, `major`, or an explicit version are all accepted. `cargo-release` bumps `portfree/Cargo.toml`, commits the bump, creates the `portfree-v{version}` tag, and pushes both. The tag push starts the release workflow, which:

1. Runs the full test suite.
2. Builds `portfree.exe` for `x86_64-pc-windows-msvc`.
3. Packages the versioned zip, the standalone executable, and `SHA256SUMS`.
4. Creates build-provenance attestations for the zip and executable.
5. Creates a draft release with generated notes.
6. Downloads the released assets and verifies the checksums, zip contents, and attestations.
7. Publishes the draft only after verification succeeds.

Watch the run and confirm the published release:

```powershell
gh run watch
gh release view portfree-v1.0.0
```

## Repair a failed release

The workflow is re-runnable for an existing tag. If packaging or an upload fails, the release is still a draft. Re-run the workflow for the same tag:

```powershell
gh run rerun <run-id>
```

or dispatch it explicitly from the tag:

```powershell
gh workflow run release.yml --ref portfree-v1.0.0 -f version=1.0.0 -f dry_run=false
```

The run reuses the existing draft, replaces the assets, verifies them, and publishes. A published release is immutable: a later run verifies the published assets and leaves them unchanged. If a published release is missing assets, cut a new version; do not rewrite the tag.

## Constraints

- Never run `cargo release` from a tag-pushing workflow step: a tag pushed with `GITHUB_TOKEN` does not trigger its own workflow.
- Never create the release tag in the workflow. A non-dry-run manual dispatch refuses to run unless the tag already exists and points at the dispatched commit.
- `publish = false` in `portfree/Cargo.toml` is deliberate. There is no crates.io release; the GitHub Release is the distribution channel.
