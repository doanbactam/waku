# Security Policy

## Supported Versions

This is a fork of Waku maintained at `doanbactam/waku`. Only the latest
release of this fork receives security fixes. Updates ship through the
in-app updater and the project's release channel.

## Reporting a Vulnerability

Please use GitHub private vulnerability reporting for this repository:

https://github.com/doanbactam/waku/security/advisories/new

Please don't open a public issue for anything you believe is exploitable
before it has been fixed. Include reproduction steps and the Waku version
(Waku → About Waku) you tested.

## Branch protection

The `main` branch is protected against untracked or unreviewed changes:

- Regular pushes are blocked; changes normally land through pull requests.
- Every pull request requires at least one approving review and resolved
  conversations before it can be merged.
- Repository admins may bypass the review requirement to push or merge
  directly (standard for this single-maintainer project).
- Force pushes and branch deletions are not allowed on `main`.

Changes to this policy are made through the repository settings on GitHub.
