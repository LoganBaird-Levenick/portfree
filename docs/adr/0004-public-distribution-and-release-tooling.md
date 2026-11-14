# Public distribution: GitHub Releases, `cargo-release`, and an unsigned v1

Portfree is distributed as public source and public GitHub Releases. The release artifact is a Windows-only `portfree.exe`: v1 ships both a versioned zip and the standalone executable, with a `SHA256SUMS` file and GitHub build-provenance attestations. The package keeps `publish = false`; there is no crates.io release, MSI, or package-manager package.

This decision supersedes the "A release pipeline" non-goal in `docs/portfree-spec.md`; distribution is a separate workstream from the v1 runtime scope. Rollout is incremental: the repository is renamed and prepared first, and the visibility flip and first public release follow once the CI and release spine are green.

Versions are cut with `cargo-release`, which bumps the `portfree` package, commits, tags `portfree-v{version}`, and pushes. A hand-written GitHub Actions workflow triggers on that tag, runs the full test suite, builds `x86_64-pc-windows-msvc`, packages the zip, writes checksums, attests the artifacts, and creates a draft GitHub Release with generated notes. The release manager runs `cargo-release` locally; a tag pushed by `GITHUB_TOKEN` would not trigger its own release workflow. The `cargo-release` configuration and tag-driven workflow are delivered by the release-workflow work; this record fixes the tooling choice.

v1 is unsigned. An Authenticode certificate requires a separate acquisition and key-management project, so the first release accepts the SmartScreen warning and documents the checksum and attestation verification path instead.

## Considered Options

- **`dist` (formerly `cargo-dist`).** Rejected for v1. It generates the release workflow, but that workflow does not run the test suite; its only bundling installer is an MSI built with end-of-life WiX v3; and its installer surface targets package-manager-style installs that a single Windows executable does not need. Revisit `dist` only if an MSI or one-liner installer becomes a real requirement.
- **Hand-written versioning without `cargo-release`.** Rejected: the version bump, dependent updates, commit, tag, and push are exactly the mechanical release steps most likely to drift. `cargo-release` performs them repeatably without taking over artifact building.
- **A signed v1.** Deferred: signing moves the release date behind certificate procurement and HSM/token-held key management, and even an EV certificate no longer bypasses SmartScreen immediately. Revisit signing before a wider launch or if SmartScreen warnings become a support burden; Azure Artifact Signing is the likely route.

The 2026-11-13 [release-tooling research note](../research/2026-11-13-windows-rust-release-tooling.md) records the primary-source comparison behind these choices.

## Consequences

- Maintainers own the release workflow: tests, packaging, checksums, attestations, and the draft release are one reviewable file rather than generated YAML.
- Release assets are a zip and a standalone `.exe`, not an installer. There is no MSI, winget/Scoop/Homebrew package, or Microsoft Store listing.
- Unsigned binaries trigger SmartScreen. The README must explain the warning and the checksum/attestation verification steps; it must not claim that releases are signed.
- `cargo-release` runs locally and pushes the tag, so the tag-triggered workflow is not suppressed by `GITHUB_TOKEN`.
- `publish = false` stays. Revisit crates.io only if publication becomes a product requirement, which would add a publish step to `cargo-release`.
- `dist` is reconsidered if an MSI or one-liner installer becomes a requirement. Signing is reconsidered before a wider launch or if SmartScreen warnings become a support burden.
