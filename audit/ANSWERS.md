# Audit answers — Wayfinder

The 27 checks a program cannot decide. `scripts/audit-mechanical.sh` reads this
file. An id with no line at the version being released prints `OPEN` and blocks
the release.

One line per answer, exactly this shape:

    <id> @ <version> — <what was done and what it showed> (<date>)

Rules that make this worth keeping:

- The answer records what was DONE, not what is believed. "Looks fine" is not an
  answer. "Installed the tarball into a fresh user, first screen was the home
  view" is.
- A finding is a valid answer. An id whose answer is "this is broken, logged in
  REDO_REGISTER.md" is answered; whether it blocks the release is a separate
  decision.
- An answer is tied to a version. The next release re-answers every id. Carrying
  an answer forward unchanged is allowed only by writing it again at the new
  version, which forces someone to decide it is still true.
- Old versions stay in this file. The history is the point.

## 0.9.22

(nothing answered yet — 27 open)
