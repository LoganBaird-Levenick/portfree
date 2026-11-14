# Contributing to Portfree

Portfree is a Windows-only Rust command-line tool. Contributions are welcome through GitHub issues and pull requests. This guide covers the local setup, the check that CI runs, what the tests expect, and what a pull request should contain.

## Before you start

- Search [existing issues](https://github.com/LoganBaird-Levenick/portfree/issues) before opening a new one.
- Use the bug report or feature request template so the report includes the details needed to act on it.
- Report security vulnerabilities privately through [SECURITY.md](SECURITY.md), never in a public issue.
- Pull requests are not a feature-request channel. Open an issue before writing code for a new capability or a large behavior change; a pull request should implement an accepted issue or a small, clearly scoped fix.

## Setup

Prerequisites:

- Windows 10 or 11.
- The MSVC build tools (Visual Studio Build Tools with the **Desktop development with C++** workload, or a Visual Studio installation that includes it).
- [rustup](https://rustup.rs/) for the Rust toolchain.
- Git.

`rust-toolchain.toml` pins the toolchain (currently Rust 1.100.0) and its `rustfmt` and `clippy` components. rustup installs that toolchain automatically the first time you run a Cargo command in the repository, so do not override it with a different default toolchain.

```powershell
git clone https://github.com/LoganBaird-Levenick/portfree.git
cd portfree
cargo build --locked
```

The debug binary is `target\debug\portfree.exe`; the release binary is `target\release\portfree.exe`.

## The Windows check

CI runs one `windows-latest` job for every pull request and every push to `main`. Before pushing, run the same gate locally. All four commands must succeed:

```powershell
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

Run them from the repository root, in a normal (non-elevated) terminal. `cargo fmt --all` rewrites formatting in place; use it before the check if `--check` reports a difference. Clippy warnings are errors, so do not silence a lint with `#[allow]` unless the lint is genuinely wrong for the code.

## Test expectations

- `cargo test --all-targets --locked` is the full suite. It contains unit tests and end-to-end tests that drive the public CLI boundary.
- The end-to-end tests spawn `portfree-test-listener`, the fixture binary built from `portfree\tests\fixtures\listener.rs`. They start their own listeners on free ports and kill only the fixture processes they created, so the suite does not touch unrelated services.
- Run the suite as your normal user, not elevated. The PID 4 safety test accepts either the normal critical-process or session-0 refusal, or `AccessDenied` on hosts that protect PID 4; all three outcomes mean nothing was killed.
- Add or update tests for every observable behavior change (output, JSON shape, exit code, kill/refusal outcome). Prefer asserting the public CLI contract over internal helpers. The integration suite lives in `portfree\tests\cli.rs`.
- Do not add test-only flags to the production binary. Testability comes from the fixture, not from hidden application switches.
- The suite is Windows-only; there is no supported cross-platform test path.

## Pull requests

1. Fork the repository and create a branch from `main`. Keep the branch focused on one logical change.
2. Write the change and its tests, then run the Windows check above.
3. Open the pull request and fill in the template. Link the issue it resolves with `Closes #<number>` when applicable.
4. Keep the pull request free of unrelated formatting, generated files (`target/`), and secrets.
5. Update the documentation when behavior or terminology changes: `README.md` for user-facing behavior, `GLOSSARY.md` for domain terms, and `docs/adr/` for a decision that should not be re-litigated later. A new decision generally gets a new ADR rather than a rewrite of an accepted one.
6. Make sure CI is green and respond to review comments. The maintainer may ask for a smaller scope or additional tests before merging.

Use an imperative, present-tense commit subject ("Add the JSON outcome field", not "Added" or "Adds"), matching the existing history. A short subject is enough for a small change; add a body when the reason is not obvious.

Every pull request must pass the Windows check before merge, including documentation-only and CI-only changes. The required status check runs whether or not the change affects the build, so run the same gate locally rather than relying on the CI queue to find a problem.

## Releasing

Releases are maintainer-only. Version bumps, tags, and pushes are handled by `cargo-release`, and the `portfree-v{version}` tag triggers the release workflow. The dry-run procedure, asset contract, verification steps, and repair path are in [docs/releasing.md](docs/releasing.md).
