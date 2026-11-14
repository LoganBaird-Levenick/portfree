# Windows Rust release tooling for Portfree (2026-11-13)

Scope: Windows-only Rust 1.100 CLI, private repo, `publish = false`, process-spawning tests. Primary sources accessed **2026-11-13**; judgements are labelled **Judgement**.

> **Status:** pre-decision research snapshot. The repository was later renamed to `portfree`, and the decision, including the public-distribution sequence and the revisit triggers, is recorded in [ADR-0004](../adr/0004-public-distribution-and-release-tooling.md). Statements below about the repository being private, empty, or on `master` describe the state when the research was done.

## Recommendation

**Judgement.** Use **approach 2**: `cargo-release` for the version bump, commit, tag, and push, plus a small hand-written workflow that runs `cargo test` on `windows-latest`, builds `portfree.exe`, zips it, writes a SHA-256 file, and optionally attests/signs.

Private release assets are visible only to people with repo access, so dist's installers (`irm | iex`, an MSI link) mostly serve users who could already clone the repo. The realistic artifact is a zip or raw `.exe`, while `dist` needs several Portfree-specific overrides (`repository`, `dist = true`, a binary allow-list), depends on end-of-life WiX v3 for MSI, and its generated `release.yml` does not run tests.

**Trade-off.** You own ~100 lines of YAML and get no generated MSI or PowerShell installer; if Portfree goes public or needs a double-click MSI, revisit `dist`.

## Current names and status (verified)

- `cargo-dist` is now branded **`dist`**: the README says “`dist` (formerly known as `cargo-dist`)”, the crate is still `cargo-dist` (0.34.0, 2026-11-02), and the installed binary is `dist` ([README](https://github.com/axodotdev/cargo-dist), [crates.io API](https://crates.io/api/v1/crates/cargo-dist), [release](https://github.com/axodotdev/cargo-dist/releases/tag/v0.34.0)). `cargo install cargo-dist --locked` installs it; invoke it as `dist` ([install](https://axodotdev.github.io/cargo-dist/book/install.html)).
- The canonical repository is `axodotdev/cargo-dist` (the `astral-sh` fork points upstream); it is active and pre-1.0.
- `cargo-release` is at 1.1.7 (2026-10-21), active, edition 2024, declared MSRV 1.97 ([crates.io API](https://crates.io/api/v1/crates/cargo-release), [repository](https://github.com/crate-ci/cargo-release)).
- Rust 1.100.0 shipped 2026-11-12 ([Rust blog](https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/)); `x86_64-pc-windows-msvc` is Tier 1 ([platform support](https://doc.rust-lang.org/rustc/platform-support.html)).

## Comparison

| Dimension | `dist` | `cargo-release` + GitHub Actions | Hand-written GitHub Actions |
|---|---|---|---|
| Artifacts | Windows `.zip`; shell, PowerShell, npm, Homebrew, MSI installers ([archives](https://axodotdev.github.io/cargo-dist/book/artifacts/archives.html), [installers](https://axodotdev.github.io/cargo-dist/book/installers/index.html)) | None from `cargo-release`; your workflow builds whatever you script | Anything you script: raw `.exe`, zip, MSI (WiX), setup.exe (InnoSetup), NSIS |
| Checksums / provenance | SHA-256 per archive plus unified `sha256.sum`; optional GitHub attestations behind an experimental flag ([checksums](https://axodotdev.github.io/cargo-dist/book/artifacts/checksums.html), [attestations](https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html)) | None built in; add `actions/attest` and a hash step | You add `Get-FileHash`/`sha256sum` and `actions/attest` yourself |
| Installers realistic for Windows-only | MSI (Program Files, WiX v3), PowerShell (user-level), zip. No NSIS, winget, or raw exe in the documented list | None; you write the packaging step | MSI, NSIS, InnoSetup, zip, raw exe, per-user or per-machine |
| Code signing | Built-in SSL.com eSigner or Azure Artifact Signing, signing EXE/MSI/PowerShell installer; x86_64 only ([Windows signing](https://axodotdev.github.io/cargo-dist/book/supplychain-security/signing/windows.html)) | None; call Azure/SSL.com action and/or `signtool` yourself | Same: `signtool` in the workflow, or a vendor action |
| Private repo | Release upload works with `GITHUB_TOKEN`; installers fetch anonymously by default, with a token env-var escape hatch; `github-releases-repo` can publish to a public repo; tag rules still apply ([usage](https://axodotdev.github.io/cargo-dist/book/installers/usage.html), [config](https://axodotdev.github.io/cargo-dist/book/reference/config.html)) | Works for version/tag; beware token-triggered workflows and tag rules | Same as left; you control every API call |
| Setup / maintenance / lock-in (judgement) | Low setup, medium lock-in: `release.yml` is generated and cannot be hand-edited without opting out ([customizing](https://axodotdev.github.io/cargo-dist/book/ci/customizing.html)) | Low setup, low lock-in: a small `[workspace.metadata.release]` table plus YAML | Highest setup, no tool lock-in, you own all breakage |
| Windows test suite | Separate CI required; generated `release.yml` never runs `cargo test` ([template](https://github.com/axodotdev/cargo-dist/blob/v0.34.0/cargo-dist/templates/ci/github/release.yml.j2)) | Separate CI required, or run tests locally before `cargo release --execute` | Can run tests and package in the same workflow |
| Versioning / tagging / changelog | Tag-driven; no bump, no tag creation. Reads changelog if present; repo has none | Bumps `Cargo.toml`, commits, tags, pushes; changelog via replacements/hooks ([reference](https://github.com/crate-ci/cargo-release/blob/main/docs/reference.md)) | Manual, or scripted around `cargo-release`; `gh release create --generate-notes` can write notes |
| Maturity / Rust 1.100 | 0.34.0, active, MSRV 1.74; supports `x86_64-pc-windows-msvc` | 1.1.7, stable, MSRV 1.97; edition 2024 | Mature; you own the logic |

## 1. `dist`

**Artifacts (verified).** On Windows the default archive is a `.zip`; the documented installers are shell, PowerShell, npm, Homebrew, and MSI. MSI is the only bundling installer, is built only on Windows, uses WiX v3 (not v4), and needs `authors` in `Cargo.toml` ([MSI guide](https://axodotdev.github.io/cargo-dist/book/installers/msi.html)). NSIS, winget, and raw-exe artifacts are not in the documented list; winget is a future request.

**Checksums and provenance (verified).** dist writes a SHA-256 file per archive and, since 0.24.0, a unified `sha256.sum`; the algorithm is configurable ([checksums](https://axodotdev.github.io/cargo-dist/book/artifacts/checksums.html)). `github-attestations = true` adds an `actions/attest@v4` step with `id-token`, `contents`, and `attestations` permissions, but the config still labels it experimental ([config](https://axodotdev.github.io/cargo-dist/book/reference/config.html), [attestation guide](https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html)). GitHub's current docs describe private-repo attestations via GitHub's Sigstore instance without an Enterprise caveat, while dist's book says private org repos need Enterprise; verify before relying on it ([GitHub](https://docs.github.com/en/actions/concepts/security/artifact-attestations)).

**Signing (verified).** Since 0.15.0 dist can sign Windows EXEs, PowerShell installers, and MSIs with SSL.com eSigner; since 0.32.0 it also supports Azure Artifact Signing, using OIDC and a `release` environment. Only x86_64 Windows targets are supported ([Windows signing](https://axodotdev.github.io/cargo-dist/book/supplychain-security/signing/windows.html)).

**Private-repo constraint (verified).** The release-upload job uses `GITHUB_TOKEN`; dist's generated workflow grants `contents: write` by default ([source](https://github.com/axodotdev/cargo-dist/blob/v0.34.0/cargo-dist/src/backend/ci/github.rs)). Installers fetch “without using any form of authentication” by default; private users can supply `${app}_GITHUB_TOKEN` ([installer usage](https://axodotdev.github.io/cargo-dist/book/installers/usage.html)), so the one-liner works against the private repo only for authorized users who set the variable. The escape hatch is `github-releases-repo` pointing at a public repo with a PAT in `GH_RELEASES_TOKEN`; dist then uploads a source tarball of the private repo unless `source-tarball = false` ([config](https://axodotdev.github.io/cargo-dist/book/reference/config.html)).

**Setup and lock-in (verified + judgement).** `dist init` writes config into `Cargo.toml` (or `dist-workspace.toml`) and generates `.github/workflows/release.yml` ([quickstart](https://axodotdev.github.io/cargo-dist/book/quickstart/rust.html)). The file is owned by dist: hand edits or staleness are an error unless you set `allow-dirty = ["ci"]`, which stops `dist init` updating it ([customizing](https://axodotdev.github.io/cargo-dist/book/ci/customizing.html)). **Judgement:** reasonable lock-in, but workflow changes go through dist config/custom jobs, not direct YAML edits.

**Tests and versioning (verified).** The generated release workflow builds and uploads but never runs `cargo test`; a separate CI workflow is needed. dist does not bump versions or create tags; it reacts to a tag and accepts `v1.0.0`, `my-app-v1.0.0`, or `my-app/1.0.0` ([introduction](https://axodotdev.github.io/cargo-dist/book/introduction.html)). It reads `CHANGELOG*`/`RELEASES*` for notes and archives; Portfree has neither ([archives](https://axodotdev.github.io/cargo-dist/book/artifacts/archives.html)).

**Portfree fit (verified).** dist also needs a `repository` URL (Portfree has none), `dist = true` because of `publish = false`, and `authors` for the MSI; a binaries allow-list keeps the test fixture out of releases ([config](https://axodotdev.github.io/cargo-dist/book/reference/config.html), [concepts](https://axodotdev.github.io/cargo-dist/book/reference/concepts.html)).

## 2. `cargo-release` + GitHub Actions

**What it does (verified).** `cargo-release` manages versions and VCS: checks branch/tree, bumps versions and dependents, runs pre-release replacements/hooks, commits, tags, pushes, and optionally runs `cargo publish` ([README](https://github.com/crate-ci/cargo-release), [reference](https://github.com/crate-ci/cargo-release/blob/main/docs/reference.md)). It produces no binaries, so approach 2 is approach 3 plus a versioning tool; artifact, signing, and private-repo mechanics are identical. `publish = false` already disables publishing; `--no-publish` is belt-and-braces ([Cargo manifest](https://doc.rust-lang.org/cargo/reference/manifest.html#the-publish-field)).

**Artifacts and signing.** These are your workflow's job: build with `cargo build --release`, zip `portfree.exe`, hash it, upload with `gh release create`, optionally attest with `actions/attest`, optionally sign with `azure/artifact-signing-action` or `sslcom/esigner-codesign` before zipping. `gh` is preinstalled on `windows-latest` ([gh manual](https://cli.github.com/manual/gh_release_create), [runner image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)).

**Private-repo constraints (verified).** A new personal repository's `GITHUB_TOKEN` is read-only for `contents`; declare `permissions: contents: write` to create a release ([settings](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#setting-the-permissions-of-the-github_token-for-your-repository), [workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions)). If `cargo-release` runs inside Actions and pushes the tag with `GITHUB_TOKEN`, the tag-push release workflow will **not** start; GitHub suppresses such runs ([GITHUB_TOKEN](https://docs.github.com/en/actions/concepts/security/github_token)). Run it locally, or use a PAT/`workflow_dispatch`. Tag rulesets can also block the push; they can grant bypass to specific GitHub Apps ([rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)).

**Versioning (verified).** For a package in a subdirectory, the default tag is `{crate_name}-v{version}`, i.e. `portfree-v1.0.0`, which matches what dist accepts ([reference](https://github.com/crate-ci/cargo-release/blob/main/docs/reference.md), [FAQ](https://github.com/crate-ci/cargo-release/blob/main/docs/faq.md)). It can update `CHANGELOG.md` through `pre-release-replacements` or a `pre-release-hook` (for example `git-cliff`), but is deliberately unopinionated about changelog format ([FAQ](https://github.com/crate-ci/cargo-release/blob/main/docs/faq.md)). This is what dist's own docs recommend for version bumps ([guide](https://axodotdev.github.io/cargo-dist/book/workspaces/cargo-release-guide.html)).

## 3. Hand-written GitHub Actions

**Artifacts (verified).** You choose: raw `portfree.exe`, zip, WiX v3 (preinstalled) or InnoSetup 6.7.1 (preinstalled) for a setup executable, or NSIS after installing it (the image lists InnoSetup and WiX, not NSIS) ([runner image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)). MSI via the preinstalled WiX v3 works, but that toolchain left community support in 2025; newer WiX would need an install step ([WiX v3](https://wixtoolset.org/docs/wix3/)).

**Checksums, provenance, signing (verified).** You write the hash step and add `actions/attest` with `id-token: write` and `attestations: write` ([attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations), [how-to](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)). `signtool` ships in the Windows SDK (`/fd` for signing, `/td` for timestamping), but the certificate is the constraint: publicly trusted OV keys must be HSM- or token-held since June 2023, so a PFX secret in Actions cannot produce a compliant public-OV signature ([signtool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool), [code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)). Use Azure Artifact Signing or SSL.com's cloud service.

**Private repo.** Same GitHub rules as above: `contents: write`, tag protection, no anonymous asset downloads. The advantage is that you build the one auth-aware installer you actually need, or skip installers.

**Setup and lock-in (judgement).** Highest initial cost (roughly a day for a careful first workflow), no framework lock-in, and all release logic is reviewable in one file. `gh release create` creates a draft, uploads assets, then publishes, which is also the recommended pattern for immutable releases ([gh manual](https://cli.github.com/manual/gh_release_create), [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)).

## Portfree-specific notes (verified)

- The manifest has `publish = false` and `license = "MIT"`, but no `repository`, `authors`, `LICENSE`, or `CHANGELOG`. dist needs the repository URL, and its MSI needs `authors`; its archives will contain neither a license nor a changelog until those files exist.
- The local branch is `master`; the GitHub repo is currently empty (`isEmpty: true`) with no branches, commits, or default branch (observed with `gh repo view` on 2026-11-13). Reconcile the branch name and push before tagging; `cargo-release` may not be able to push to a protected `main`.
- `windows-latest` is Windows Server 2025 with Rust 1.99.1 and WiX 3.14.1 preinstalled ([runner image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md), [runner labels](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job)). Since the crate requires 1.100, add a `rustup` step or `rust-toolchain.toml`; do not rely on the image's Rust.
- Private Windows jobs draw from included compute: GitHub Free includes $12/month, Windows runners cost $0.010/minute, and public-repo standard runners are free ([billing](https://docs.github.com/en/billing/managing-billing-for-your-products/managing-billing-for-github-actions/about-billing-for-github-actions)).
- Release assets on a private repo are visible only to people with read access ([about releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases), [visibility](https://docs.github.com/en/repositories/creating-and-managing-repositories/about-repositories#about-repository-visibility)).

**Not verified:** I did not run `dist init` or `cargo release` against this repo. Rust 1.100 compatibility rests on declared MSRVs and the target tier, not a trial build.

## Open questions

1. **Public or private distribution?** If public, dist's installers and Azure signing matter more, and attestations become externally verifiable.
2. **Is code signing required?** Unsigned is fine internally; Azure Artifact Signing is about $9.99/month with identity validation and geographic limits, and still builds SmartScreen reputation over time ([Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)). EV no longer bypasses SmartScreen instantly (since 2024).
3. **Is crates.io ever in scope?** `publish = false` says no. If that changes, `cargo-release` gains a publish step.
4. **Does anyone need an MSI?** An MSI defaults to `%ProgramFiles%` (per-machine, UAC), a poor fit for a per-user developer CLI; a zip or per-user installer may be the better product decision.

## Sources (accessed 2026-11-13)

**dist**
- [Repository and README](https://github.com/axodotdev/cargo-dist)
- [Book: install](https://axodotdev.github.io/cargo-dist/book/install.html), [quickstart](https://axodotdev.github.io/cargo-dist/book/quickstart/rust.html), [introduction](https://axodotdev.github.io/cargo-dist/book/introduction.html)
- [Installers](https://axodotdev.github.io/cargo-dist/book/installers/index.html), [MSI](https://axodotdev.github.io/cargo-dist/book/installers/msi.html), [installer usage](https://axodotdev.github.io/cargo-dist/book/installers/usage.html)
- [Archives](https://axodotdev.github.io/cargo-dist/book/artifacts/archives.html), [checksums](https://axodotdev.github.io/cargo-dist/book/artifacts/checksums.html)
- [Windows signing](https://axodotdev.github.io/cargo-dist/book/supplychain-security/signing/windows.html), [GitHub attestations](https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html)
- [Config reference](https://axodotdev.github.io/cargo-dist/book/reference/config.html), [concepts](https://axodotdev.github.io/cargo-dist/book/reference/concepts.html), [CI customization](https://axodotdev.github.io/cargo-dist/book/ci/customizing.html), [cargo-release guide](https://axodotdev.github.io/cargo-dist/book/workspaces/cargo-release-guide.html)
- [Release template](https://github.com/axodotdev/cargo-dist/blob/v0.34.0/cargo-dist/templates/ci/github/release.yml.j2), [workflow permissions source](https://github.com/axodotdev/cargo-dist/blob/v0.34.0/cargo-dist/src/backend/ci/github.rs)
- [crates.io API 0.34.0](https://crates.io/api/v1/crates/cargo-dist), [release v0.34.0](https://github.com/axodotdev/cargo-dist/releases/tag/v0.34.0)

**cargo-release**
- [Repository](https://github.com/crate-ci/cargo-release), [reference](https://github.com/crate-ci/cargo-release/blob/main/docs/reference.md), [FAQ](https://github.com/crate-ci/cargo-release/blob/main/docs/faq.md), [crates.io API 1.1.7](https://crates.io/api/v1/crates/cargo-release)

**GitHub**
- [Actions billing](https://docs.github.com/en/billing/managing-billing-for-your-products/managing-billing-for-github-actions/about-billing-for-github-actions)
- [`GITHUB_TOKEN`](https://docs.github.com/en/actions/concepts/security/github_token), [workflow permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions), [default token permissions](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#setting-the-permissions-of-the-github_token-for-your-repository)
- [Artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations), [using attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [About releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases), [repository visibility](https://docs.github.com/en/repositories/creating-and-managing-repositories/about-repositories#about-repository-visibility)
- [Rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets), [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
- [Choosing a runner](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job), [windows-2025 image](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)
- [`gh release create`](https://cli.github.com/manual/gh_release_create)

**Microsoft / Windows**
- [Code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options), [SignTool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool)
- [Azure Artifact Signing action](https://github.com/Azure/artifact-signing-action), [SSL.com eSigner action](https://github.com/SSLcom/esigner-codesign)
- [WiX v3 status](https://wixtoolset.org/docs/wix3/)

**Rust / Cargo**
- [Rust 1.100.0](https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/), [platform support](https://doc.rust-lang.org/rustc/platform-support.html), [Cargo `publish` field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-publish-field)
