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

As of 0.9.40: `cargo audit` reports no vulnerable dependency, and the refusals
above all have tests behind them. What follows is everything in the release
audit that is reasoned about but **not measured**. It is here, in the file that
ships, because a gap nobody wrote down is a gap nobody fixes. One item that
used to be on this list is now measured: two clean builds of the release
source produce byte-identical programs (checked again for 0.9.30).

1. **The hostile run at the file dialog covers one session's worth of input.**
   For 0.9.37 the release audit fired 85 hostile requests at the portal
   backend on a private bus: malformed byte arrays, a `current_name` of
   `../../.bashrc`, symlinks out of the chosen folder, absurd lengths, control
   characters and right-to-left overrides, wrong-typed options, and `Close()`
   while a request was open. Nothing crashed, and no path was handed back that
   the request or the picker did not name. Two weaknesses it found are fixed
   in 0.9.38 (wrong-typed options were read as "no"; the picker's done marker
   was read without a size limit). It was a fixed list, not a fuzzer: inputs outside
   that list have not been tried.
2. **Unsafe code per dependency is not counted.** The repository's own `unsafe`
   lines are counted by the audit script, but `cargo geiger` failed three times
   on crate sources that were never extracted, so the dependency tree's own
   count is unknown.
3. **Disk full and a volume unplugged mid-operation are not staged.** Both need
   a filesystem this audit cannot create without root. Permission denied and a
   folder of 100,000 entries are tested; these two are not.
4. **Contrast is computed, not measured.** The ratios come from the WCAG
   formula applied to the stylesheet's own colours, against the darkest
   background. They have not been measured on a screen, and lighter surfaces
   -- a hovered or selected row -- are not covered.
5. **Eject and unmount are tested with stand-in commands only.** The decision
   is tested: a failed unmount or a failed partition listing never powers the
   drive off, and both are refused while a transfer runs. The real `udisksctl`
   calls against a real USB drive are not part of any automated run.
6. **Previews hand files you did not make to outside tools.** LibreOffice,
   Ghostscript, ImageMagick, `pdftoppm`, 7-Zip, `unar`, `xz`, `ffmpeg`,
   GStreamer and Ruffle read the file to draw a preview or list an archive.
   Since 0.9.30, LibreOffice, Ghostscript and ImageMagick run with no network
   (a private network namespace via `unshare`), Ghostscript with `-dSAFER`,
   and ImageMagick with memory and time limits. None of them is otherwise
   sandboxed: each can still read any file you can. No hostile document,
   picture, archive or video has been fired at them.
7. **The no-network rule needs user namespaces.** Where unprivileged user
   namespaces are switched off, those previews still run, with network, and
   Wayfinder does not say so on screen.
8. **The PS2 drive row is proven on one machine only.** Reading a PS2-format
   hard drive was checked against one real drive. Other drives, and damaged
   ones, are not covered.
9. **Filenames that are not valid UTF-8 are not tested end to end.** Linux
   allows them; the program converts paths to text for display in hundreds of
   places. Whether every one of those survives the round trip is unknown.
10. **One command cannot run the whole suite.** Rust's test harness gives every
   test its own thread, and GTK refuses to start twice, so the GTK suite has to
   be run on its own. Both runs are reported in the release audit; a single
   `cargo test --bins` cannot pass by construction.

11. **Two previews are Wayfinder's own parsers.** Windows shortcuts (`.lnk`)
   and Outlook messages (`.msg`) are read by Wayfinder's own code, not an
   outside tool, so a damaged or hostile file reaches Wayfinder itself. Both
   read at most a fixed size (1 MiB and 64 MiB), check every offset, and turn a
   damaged file into an error message; damaged-file tests cover that. No
   hostile-file run or fuzzer has been fired at them.

12. **Archive extraction is locked, previews are not.** Since 0.9.38, Extract
   Here and Extract All run 7-Zip, `unar` or `tar` into a private staging
   folder that the tool can write to and nothing else (Landlock), then check
   every staged entry (no special files, no link leaving the folder) and move
   it into place without replacing anything. Since 0.9.39 an outside tool is
   not run at all where the kernel has no Landlock (the built-in ZIP reader
   still works), a hard link in the staged output is refused, and the
   destination is checked again before each entry is moved. Archive listing
   and previews are not locked at all.
13. **Updates are checksummed, not signed.** The update manifest is fetched
   from kadaken.com over HTTPS with no redirects. Since 0.9.39 the download
   checks each redirect before following it (at most three, each one HTTPS on
   kadaken.com), and the file must match the manifest's size and SHA-256. A proxy set in the environment is used. The manifest itself is not
   signed: whoever controls what kadaken.com serves controls the update.
14. **"No other connections" is read from the source, not measured.**
   Wayfinder's own code talks only to kadaken.com, for the update check. GIO,
   GStreamer, the apps a file is opened with, and Ruffle can make their own
   connections, and no run has recorded every connection the installed
   program makes.

## Supported versions

The newest release, and only the newest. There is no long-term branch yet.
