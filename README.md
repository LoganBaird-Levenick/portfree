# Portfree

A Windows 10/11 command-line tool that identifies which process owns a TCP port and stops exactly that process after confirmation.

```text
portfree <PORT> [--pid <PID>] [--dry-run] [--json] [--yes] [--no-color]
```

## Install

Download the latest release from [GitHub Releases](https://github.com/LoganBaird-Levenick/portfree/releases). Each release publishes:

- `portfree-v<version>-x86_64-pc-windows-msvc.zip` — the versioned package. It contains `portfree.exe`, `README.md`, and `LICENSE` under a top-level `portfree-v<version>-x86_64-pc-windows-msvc\` directory.
- `portfree.exe` — the same executable as a standalone download.
- `SHA256SUMS` — SHA-256 checksums for both artifacts.

For example, the v1.0.0 zip is `portfree-v1.0.0-x86_64-pc-windows-msvc.zip`.

Extract the zip and put `portfree.exe` somewhere on your `PATH`:

```powershell
Expand-Archive .\portfree-v1.0.0-x86_64-pc-windows-msvc.zip -DestinationPath .
.\portfree-v1.0.0-x86_64-pc-windows-msvc\portfree.exe 3000
```

Portfree is distributed only through these GitHub Release artifacts. There is no MSI, package-manager package, Microsoft Store listing, or crates.io release.

## Verify the download

v1 releases are unsigned (see [SmartScreen](#smartscreen)), so verify a download before running it.

1. Compare the file's SHA-256 hash with the matching line in `SHA256SUMS`:

   ```powershell
   $asset = "portfree-v1.0.0-x86_64-pc-windows-msvc.zip"
   $expected = (Get-Content .\SHA256SUMS | Where-Object { $_ -match [regex]::Escape($asset) }).Split()[0]
   $actual = (Get-FileHash .\$asset -Algorithm SHA256).Hash
   if ($actual -ne $expected) { throw "Checksum mismatch for $asset" }
   ```

   Repeat with `$asset = "portfree.exe"` if you downloaded the standalone executable.

2. Verify the build-provenance attestation with the [GitHub CLI](https://cli.github.com/):

   ```powershell
   gh attestation verify .\portfree-v1.0.0-x86_64-pc-windows-msvc.zip --repo LoganBaird-Levenick/portfree
   gh attestation verify .\portfree.exe --repo LoganBaird-Levenick/portfree
   ```

   The attestation records that the artifact was built by this repository's release workflow from the tagged commit.

## SmartScreen

The v1 executables are not code-signed. On first run, Windows may show **Windows protected your PC** with an unknown publisher. That warning is expected for an unsigned binary.

Verify the checksum and attestation first, then choose **More info** → **Run anyway**. Do not bypass the warning for a file you have not verified. Code signing is not part of v1; the trade-off and the conditions for revisiting it are recorded in [ADR-0004](docs/adr/0004-public-distribution-and-release-tooling.md).

## Usage

- `--pid` asserts which owner to kill when more than one process listens on the port.
- `--dry-run` resolves and reports, but never kills.
- `--json` emits exactly one JSON document on stdout. By itself it is a dry run; combine it with `--yes` to kill.
- `--yes` skips the prompt. It is the only way to authorize a kill without a TTY.
- `--no-color` (or `NO_COLOR`) disables ANSI color on each stream.

```powershell
portfree 3000                 # report, prompt, then kill
portfree 3000 --dry-run       # report only
portfree 3000 --pid 18412     # require PID 18412 to be the owner
portfree 3000 --json --yes    # kill and emit JSON
```

## Outcomes

| Outcome | Meaning | Exit |
|---|---|---|
| `no_listener` | Nothing was listening at inspection. | 0 |
| `dry_run` | Target resolved; no kill by design or by declining the prompt. | 0 |
| `not_attempted` | Refused before the kill phase (ambiguous port, system process, no TTY, etc.). | 3 (4 for `no_tty`) |
| `target_exited` | The target exited before the first kill; the port was observed free. | 0 |
| `killed` | The target and its descendants were terminated; the port was observed free. | 0 |
| `aborted` | Pre-flight failed after confirmation; nothing was killed. | 3 |
| `failed` | A survivor, respawn, or still-held port was observed after the kill attempt. | 1 |

Declining the interactive prompt reports `dry_run` and exits 0. EOF counts as "No"; Ctrl+C at the prompt exits 130 with nothing killed.

Pre-flight refusals use `aborted` / `access_denied` (the specific safety reason is carried in the message), or `aborted` / `stale_target` when the target's identity changed after confirmation.

## Build and test

Prerequisites: Windows 10/11, the MSVC build tools, and Rust 1.100. The repository's `rust-toolchain.toml` pins the toolchain version.

```powershell
cargo build --release --locked
cargo test --all-targets --locked
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
```

The release binary is `target\release\portfree.exe`. The crate is Windows-only and uses the native TCP table (`netstat2`), process metadata (`sysinfo`), and `windows-sys` for handles and safety checks.

The end-to-end tests spawn `portfree\tests\fixtures\listener.rs` as a separate fixture binary. It can listen normally, dual-stack, with `SO_REUSEADDR`, on extra ports, or wait for a PID to exit and rebind during the observation window. The production binary has no test-only flags.

## License

Portfree is MIT licensed.
