# Security policy

## Supported versions

Security fixes are made on `main` and released in the latest `0.3.x` release. Earlier release
lines do not receive fixes; upgrade to the latest release to get them.

| Version | Supported |
|---------|-----------|
| Latest `0.3.x` release | Yes |
| Older `0.3.x` releases, `0.2.x`, `0.1.x` | No |

## Reporting a vulnerability

Please do not report security vulnerabilities in public issues, pull requests or discussions.

Report them privately through GitHub's private vulnerability reporting: open the repository's
**Security** tab and select **Report a vulnerability**
(<https://github.com/CountlinkX-Solutions/geoframe/security/advisories/new>). Only the
maintainers can see the report.

Examples of what is in scope:

- input (a file, WKT, WKB or GeoJSON text, a PostGIS value) that crashes or aborts the BEAM VM
  through the NIF, or that causes memory unsafety in the Rust code;
- input that consumes memory or CPU far beyond the documented
  [resource limits](docs/parity.md#resource-limits);
- a way to make the precompiled NIF download bypass its checksum verification.

## What to include

- The affected GeoFrame version (release tag or commit SHA).
- The Elixir and OTP versions, the operating system and target, and whether the NIF was built
  from source or downloaded precompiled.
- A description of the issue and its impact.
- A minimal reproduction: the code and, where relevant, the smallest input file that triggers it.
- Any mitigation or fix you are aware of.

## What to expect

GeoFrame is maintained on a best-effort basis, so there are no guaranteed response or fix
times. The maintainers will acknowledge the report, investigate it, and keep you informed
through the private advisory. When a fix is released, it is announced in a GitHub security
advisory and in the [changelog](CHANGELOG.md); reporters are credited in the advisory unless
they prefer not to be.
