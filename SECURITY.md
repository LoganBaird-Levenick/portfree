# Security policy

## Supported versions

Portfree is a Windows command-line tool distributed through GitHub Releases. Only the latest published release receives security fixes. If you are running an older build, reproduce the issue on the latest release before reporting it.

## Reporting a vulnerability

**Do not open a public issue for a security problem.** Public issues are visible to everyone, including people who may exploit the issue before a fix is available.

Report vulnerabilities privately through GitHub's private vulnerability reporting:

1. Open the repository's [Security tab](https://github.com/LoganBaird-Levenick/portfree/security).
2. Choose **Report a vulnerability**.
3. Describe the issue in the private advisory form.

The direct link is <https://github.com/LoganBaird-Levenick/portfree/security/advisories/new>.

A report is most useful when it includes:

- the Portfree version and how you obtained it (release zip, standalone `portfree.exe`, or a local build);
- your Windows version and architecture;
- the command or input that triggers the issue;
- the expected and actual behavior, including the observed outcome and exit code;
- any proof-of-concept code, logs, or screenshots that help explain the impact.

## What to expect

- **Acknowledgement within 7 calendar days.** If you do not hear back by then, reply in the private advisory to make sure it was not missed.
- **An initial assessment within 30 days.** That assessment covers severity, affected versions, and whether a fix or mitigation is planned. If the investigation needs longer, we will explain why and give an updated estimate.
- **Coordination before disclosure.** We will agree on a disclosure date with you. We aim to publish a fixed release and a GitHub security advisory together, and we will credit you unless you prefer to remain anonymous.
- **No public disclosure before a fix is available.** Please give us a reasonable opportunity to ship a fix before discussing the issue publicly.

Portfree v1 is distributed unsigned and is not a hosted service. Reports about the SmartScreen warning itself, or about a downloaded file whose SHA-256 checksum and build-provenance attestation do not match the published release, are still welcome through the same private route.

## Out of scope

- Vulnerabilities in Windows, Rust, or third-party dependencies that are not specific to Portfree. Report those to the upstream project; a Portfree-specific impact is still in scope.
- Social-engineering, physical-access, or denial-of-service scenarios that require an already-compromised machine.
- Issues that only affect a build made from an unreleased commit or a modified source tree.
