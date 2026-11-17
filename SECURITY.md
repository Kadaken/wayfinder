# Reporting a security problem

Wayfinder is alpha software, and one half of it is a **portal backend**: a
service that any program on your machine can send requests to over D-Bus, and
that hands file paths back to whoever asked. That is the part worth attacking,
and the part I most want to hear about.

## How to report

**Please do not open a public issue for a security problem.**

1. [Open a private security advisory](https://github.com/Kadaken/wayfinder/security/advisories/new)
   — GitHub's own form, visible only to the maintainer.
2. Or email **support@kadaken.com** with `Wayfinder security` in the subject.

I will confirm I have read it, and tell you what I intend to do about it and
roughly when. If I disagree that something is a security problem, I will say so
and say why rather than leave you waiting.

## What is in scope

- Anything that makes the portal backend hand an app a path the person never
  chose, or a path outside the folder they were looking at.
- Anything that makes Wayfinder read, write, move or delete a file the person
  did not point it at — including through a symbolic link, a hard link, or a
  crafted archive entry.
- Anything a filename, an archive member name, a network share label or a
  D-Bus argument can do that it should not: shell injection, path traversal,
  a crash that is reachable from a hostile input.
- Anything that leaves the picker's answer file, or its folder, readable by
  another user.
- Anything that leaks a path out of a folder the person has told Wayfinder to
  keep out of recent lists.

## What is out of scope

- Crashes that need you to already be able to run code as that user.
- Bugs in GTK, GStreamer, zbus or any other dependency — report those upstream,
  but do tell me if Wayfinder's use of them is what makes the bug reachable.
- Missing hardening that no input can reach. Say it anyway, as an ordinary
  issue; it is just not an advisory.

## What I have and have not checked myself

The audit, and what in it is still unmeasured, is kept honest in
[`CHANGELOG.md`](CHANGELOG.md) and in the repository's own notes. The short
version as of 0.9.28: `cargo audit` reports no vulnerable dependency, the
refusals above all have tests behind them, and **a deliberate hostile run at
the file dialog has never been written.** If you want somewhere to start, start
there.

## Supported versions

The newest release, and only the newest. There is no long-term branch yet.
