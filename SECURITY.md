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

As of 0.9.28: `cargo audit` reports no vulnerable dependency, and the refusals
above all have tests behind them. What follows is everything in the release
audit that is reasoned about but **not measured**. It is here, in the file that
ships, because a gap nobody wrote down is a gap nobody fixes.

1. **A hostile run at the file dialog has never been written.** The portal
   backend takes its input from other applications, not from the person at the
   keyboard, so this is the largest gap in the release. Malformed byte arrays,
   a `current_name` of `../../.bashrc`, a symlink pointing out of the chosen
   folder, absurd lengths, control characters and right-to-left overrides in
   the title and the accept button, and a `Close()` arriving while the request
   is still open — none of it has been fired at it. If you want somewhere to
   start, start there.
2. **Unsafe code per dependency is not counted.** The repository's own `unsafe`
   lines are counted by the audit script, but `cargo geiger` failed three times
   on crate sources that were never extracted, so the dependency tree's own
   count is unknown.
3. **Disk full and a volume unplugged mid-operation are not staged.** Both need
   a filesystem this audit cannot create without root. Permission denied and a
   folder of 100,000 entries are tested; these two are not.
4. **Durability of the settings file is not proven.** Settings are written to a
   temporary file and renamed, with no `fsync` of either the file or its
   folder, so what survives a power cut depends on the filesystem. No crash has
   been staged. Two processes can also race on that one temporary name.
5. **The freeze this release exists to fix has no test.** The sidebar no longer
   waits on a blocked drive, which was fixed by hand and confirmed by hand. A
   test that deliberately hangs the background read, and proves the window
   still answers, is not written.
6. **Contrast is computed, not measured.** The ratios come from the WCAG
   formula applied to the stylesheet's own colours, against the darkest
   background. They have not been measured on a screen, and lighter surfaces
   -- a hovered or selected row -- are not covered.
7. **The build is not yet proven reproducible.** Building the shipped tag twice
   in a clean clone and comparing the bytes happens after the tag is published,
   so it is not a claim this file makes.
8. **Filenames that are not valid UTF-8 are not tested end to end.** Linux
   allows them; the program converts paths to text for display in hundreds of
   places. Whether every one of those survives the round trip is unknown.
9. **One command cannot run the whole suite.** Rust's test harness gives every
   test its own thread, and GTK refuses to start twice, so the GTK suite has to
   be run on its own. Both runs are reported in the release audit; a single
   `cargo test --bins` cannot pass by construction.

## Supported versions

The newest release, and only the newest. There is no long-term branch yet.
