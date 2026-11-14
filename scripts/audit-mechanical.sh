#!/usr/bin/env bash
# Wayfinder — the mechanical half of the release audit.
#
# WHY THIS EXISTS: the audit instruction files hold ~136 prose Actions that a
# model interprets. Interpretation drifts, invents false findings, and can skip
# a check silently. Every Action that CAN be decided by a command lives here
# instead, so it runs the same way every time and grades its own output.
#
# It changes nothing. It prints one line per check ID and exits non-zero if any
# check FAILED.
#
# Each line is: <STATE>  <id>  <one-line result>
#   PASS   the check ran and the result is correct
#   FAIL   the check ran and the result is wrong — a release blocker
#   HINT   a tool complained about something that is not a blocker
#   SKIP   the check could not run here; the reason is a finding in itself
#   JUDGE  this ID is NOT mechanical. It is listed so it can never be counted
#          as passed by silence. A human or a model must answer it.
#
# Usage:  scripts/audit-mechanical.sh [--repo <path>]
#
# It contains no absolute personal path by design. Anything machine-specific is
# read from the environment, so the script is safe to publish with the source.
set -uo pipefail

REPO="${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
[[ "${1:-}" == "--repo" ]] && REPO="$2"
cd "$REPO" || { echo "cannot enter $REPO"; exit 2; }

PASS=0; FAIL=0; HINTS=0; SKIPS=0
pass(){ printf 'PASS   %-9s %s\n' "$1" "$2"; PASS=$((PASS+1)); }
fail(){ printf 'FAIL   %-9s %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
hint(){ printf 'HINT   %-9s %s\n' "$1" "$2"; HINTS=$((HINTS+1)); }
skip(){ printf 'SKIP   %-9s %s\n' "$1" "$2"; SKIPS=$((SKIPS+1)); }
judge(){ printf 'JUDGE  %-9s %s\n' "$1" "$2"; }
have(){ command -v "$1" >/dev/null 2>&1; }

echo "=== Wayfinder mechanical release audit"
echo "repo:   $REPO"
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
echo "version: ${VER:-UNKNOWN}"
echo "commit: $(git rev-parse --short HEAD 2>/dev/null)"
echo "date:   $(date -Is)"
echo

# ---------------------------------------------------------------- S8 version truth
echo "-- S8 version truth"
if [[ -z "$VER" ]]; then
  fail S8.0 "no version in src-tauri/Cargo.toml — every other version check is meaningless"
else
  pass S8.0 "Cargo.toml version = $VER"

  # W5b: the five in-tree files that must carry the version.
  declare -A WANT=(
    [src-tauri/Cargo.toml]="version = \"$VER\""
    [src-tauri/com.kadaken.wayfinder.metainfo.xml]="<release version=\"$VER\""
    [flatpak/com.kadaken.wayfinder.yml]="tag: v$VER"
    [CHANGELOG.md]="$VER"
    [SECURITY.md]="$VER"
  )
  miss=()
  for f in "${!WANT[@]}"; do
    if [[ ! -f "$f" ]]; then miss+=("$f (absent)")
    elif ! grep -qF -- "${WANT[$f]}" "$f"; then miss+=("$f")
    fi
  done
  if ((${#miss[@]})); then fail S8.1 "version missing from: ${miss[*]}"
  else pass S8.1 "all 5 expected files carry $VER"; fi

  # A sixth file carrying the version is a finding (W5b).
  mapfile -t carriers < <(git ls-files -- '*.toml' '*.json' '*.xml' '*.yml' '*.md' '*.sh' \
      | xargs -r grep -lF -- "$VER" 2>/dev/null | sort)
  extra=()
  for c in "${carriers[@]}"; do [[ -v WANT[$c] ]] || extra+=("$c"); done
  if ((${#extra[@]})); then hint S8.2 "also carry $VER (confirm each is intended): ${extra[*]}"
  else pass S8.2 "no unexpected file carries the version"; fi

  # The defect the fourth pass found: ^{commit} is mandatory on an annotated tag.
  if git rev-parse -q --verify "refs/tags/v$VER" >/dev/null; then
    tc="$(git rev-parse "v$VER^{commit}")"
    hc="$(git rev-parse HEAD)"
    if [[ "$tc" == "$hc" ]]; then pass S8.3 "tag v$VER^{commit} == HEAD ($(git rev-parse --short HEAD))"
    else fail S8.3 "tag v$VER points at ${tc:0:7}, HEAD is ${hc:0:7} — artifact and tag disagree"; fi
    td="$(git log -1 --format=%ct "v$VER^{commit}")"; hd="$(git log -1 --format=%ct HEAD)"
    if (( td > hd )); then fail S8.4 "tag commit is newer than HEAD"
    else pass S8.4 "tag is not newer than the build commit"; fi
  else
    skip S8.3 "no tag v$VER yet — re-run after tagging"
    skip S8.4 "no tag v$VER yet"
  fi
fi

dirty="$(git status --porcelain | wc -l)"
if (( dirty == 0 )); then pass S8.5 "working tree clean"
else fail S8.5 "$dirty uncommitted files — a release must not be built from this tree"; fi

# ---------------------------------------------------------------- S15 build provenance
echo; echo "-- S15 build provenance"
if (( dirty == 0 )); then pass S15.1 "tree clean at build commit"
else fail S15.1 "tree dirty: $(git status --porcelain | head -3 | awk '{print $2}' | tr '\n' ' ')..."; fi
if [[ -f src-tauri/Cargo.lock ]] && git ls-files --error-unmatch src-tauri/Cargo.lock >/dev/null 2>&1; then
  pass S15.2 "Cargo.lock committed"
else fail S15.2 "Cargo.lock missing or untracked — the build is not pinned"; fi
mapfile -t builds < <(git ls-files -- '*build.rs')
if ((${#builds[@]})); then hint S15.3 "build scripts to read by hand: ${builds[*]}"
else pass S15.3 "no build.rs in the tree"; fi
judge S15.4 "fresh clone at the tag, empty cache, built twice and compared — run by hand"

# ---------------------------------------------------------------- S18 desktop fit
echo; echo "-- S18 desktop fit"
if have desktop-file-validate; then
  for d in packaging/*.desktop; do
    [[ -f "$d" ]] || continue
    out="$(desktop-file-validate "$d" 2>&1)"
    b="$(basename "$d")"
    if grep -q "error:" <<<"$out"; then fail S18.1 "$b: $(grep -m1 'error:' <<<"$out")"
    elif grep -q "warning:" <<<"$out"; then fail S18.1 "$b: $(grep -m1 'warning:' <<<"$out")"
    elif grep -q "more than one main category" <<<"$out"; then
      # Graded as a hint, not a blocker. It is a menu-placement choice, the tool
      # itself calls it a hint, and no user loses data over it. Grading it FAIL
      # put a cosmetic choice in the same column as a leaked credential.
      hint S18.1 "$b: lists two main categories, so it may appear twice in the application menu"
    elif [[ -n "$out" ]]; then hint S18.1 "$b: $(head -1 <<<"$out" | sed "s|^.*hint: ||" | cut -c1-100)"
    else pass S18.1 "$b: clean"; fi
  done
else skip S18.1 "desktop-file-validate not installed"; fi

MI=src-tauri/com.kadaken.wayfinder.metainfo.xml
if have appstreamcli && [[ -f "$MI" ]]; then
  out="$(appstreamcli validate --no-net "$MI" 2>&1)"
  if grep -qiE "^E:|error" <<<"$out"; then fail S18.2 "metainfo: $(grep -m1 -iE '^E:|error' <<<"$out" | cut -c1-110)"
  elif grep -qiE "^W:|warning" <<<"$out"; then hint S18.2 "metainfo warning: $(grep -m1 -iE '^W:|warning' <<<"$out" | cut -c1-110)"
  else pass S18.2 "metainfo validates"; fi
  newest="$(grep -m1 -o '<release version="[^"]*"' "$MI" | cut -d'"' -f2)"
  [[ "$newest" == "$VER" ]] && pass S18.3 "newest <release> is $VER" \
    || fail S18.3 "newest <release> is $newest, version is $VER"
else skip S18.2 "appstreamcli or metainfo missing"; fi
judge S18.4 "menu entry, icon sizes, MIME claims and how the user undoes them, light/dark without restart"

# ---------------------------------------------------------------- S5 licence
echo; echo "-- S5 licence"
if [[ -f LICENSE ]]; then
  dec="$(sed -n 's/^license = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
  if grep -qi "GNU GENERAL PUBLIC LICENSE" LICENSE && [[ "$dec" == GPL-3.0* ]]; then
    pass S5.1 "LICENSE is GPL and Cargo.toml declares $dec"
  else fail S5.1 "LICENSE text and declared licence '$dec' disagree"; fi
else fail S5.1 "no LICENSE file"; fi
if [[ -f src-tauri/THIRD-PARTY-NOTICES.md || -f THIRD-PARTY-NOTICES.md ]]; then
  pass S5.2 "third-party notices present"
else hint S5.2 "no THIRD-PARTY-NOTICES file found — scripts/third-party-notices.py exists, run it"; fi
judge S5.3 "site terms vs the shipped licence, and a plain-English warranty disclaimer"

# ---------------------------------------------------------------- K8 supply chain
echo; echo "-- K8 supply chain"
if have cargo-audit || cargo audit --version >/dev/null 2>&1; then
  out="$(cd src-tauri && cargo audit --quiet 2>&1)"
  # A vulnerability blocks. "unmaintained" and "unsound" are risks to accept in
  # writing, not blockers — grading them the same way trains him to ignore both.
  vuln="$(grep -B3 "^ID:" <<<"$out" | grep -ciE "Title:.*(vulnerab|overflow|RCE|injection|use after free|denial)")"
  unmaint="$(grep -ciE "Title:.*(unmaintained|unsound|yanked)" <<<"$out")"
  ids="$(grep '^ID:' <<<"$out" | awk '{print $2}' | tr '\n' ' ')"
  if (( vuln > 0 )); then fail K8.1 "cargo audit: $vuln VULNERABILITY advisories — $ids"
  elif (( unmaint > 0 )); then hint K8.1 "cargo audit: $unmaint unmaintained/unsound, 0 vulnerabilities — $ids (needs a written acceptance, not a fix)"
  else pass K8.1 "cargo audit: no advisories"; fi
else skip K8.1 "cargo audit not installed"; fi
cnt="$(cd src-tauri && cargo tree 2>/dev/null | wc -l)"
(( cnt > 0 )) && pass K8.2 "dependency tree lines: $cnt (compare to the last release)" \
             || skip K8.2 "cargo tree produced nothing"
for t in cargo-deny cargo-geiger gitleaks; do
  have "$t" || hint K8.3 "$t not installed — that check cannot run"
done

# ---------------------------------------------------------------- K1/K7 code safety
echo; echo "-- K1/K7 code safety"
u="$(grep -rn "unsafe" src-tauri/src --include=*.rs 2>/dev/null | grep -vc "^\s*//")"
pass K1.1 "lines mentioning unsafe: $u (each needs a stated invariant)"
grep -rq "forbid(unsafe_code)" src-tauri/src 2>/dev/null \
  && pass K1.2 "forbid(unsafe_code) present somewhere" \
  || hint K1.2 "no forbid(unsafe_code) anywhere"
up="$(grep -rno "\.unwrap()\|\.expect(" src-tauri/src --include=*.rs 2>/dev/null | wc -l)"
pass K7.1 "unwrap/expect sites: $up (K7 asks which are reachable from untrusted input)"
pn="$(grep -rno "panic!(" src-tauri/src --include=*.rs 2>/dev/null | wc -l)"
pass K7.2 "explicit panic! sites: $pn"
judge K1.3 "clippy with warnings denied — run it and grade by lint"

# ---------------------------------------------------------------- K6 secrets
echo; echo "-- K6 secrets"
# CERTAIN: a hit is a secret. SUSPECT: a hit needs a human to read it, because a
# test fixture holding the string "password=hunter2" is not a leak.
CERTAIN='BEGIN (RSA|OPENSSH|EC|DSA|PRIVATE) KEY|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{30}|AKIA[0-9A-Z]{16}|sk-[A-Za-z0-9]{32}|xox[baprs]-[A-Za-z0-9-]{10}'
# Rust uses `token` for cancellation tokens, so a bare `token =` is noise and
# noise trains him to skip the whole section. A suspect hit needs a credential
# word AND a literal value.
SUSPECT='(api|auth|access|bearer|private|client)[_-]?(key|token|secret)[[:space:]]*[=:][[:space:]]*["'"'"']|password[[:space:]]*[=:][[:space:]]*["'"'"']|passwd[[:space:]]*[=:][[:space:]]*["'"'"']'
c="$(git grep -IlE "$CERTAIN" -- . 2>/dev/null | wc -l)"
(( c == 0 )) && pass K6.1 "working tree: 0 files match a certain-secret probe" \
             || fail K6.1 "working tree: $c files match a CERTAIN secret probe: $(git grep -IlE "$CERTAIN" -- . | tr '\n' ' ')"
mapfile -t susp < <(git grep -InE "$SUSPECT" -- . 2>/dev/null | cut -c1-100)
if ((${#susp[@]} == 0)); then pass K6.2 "working tree: 0 suspect-pattern lines"
else
  hint K6.2 "${#susp[@]} suspect-pattern lines to read (a fixture is not a leak):"
  for l in "${susp[@]}"; do printf '                 %s\n' "$l"; done
fi
# History: count distinct PATHS, not path-per-commit. 154 hits of one file across
# 154 commits is one file, and reporting it as 154 is a false alarm.
if (( $(git rev-list --all --count) > 0 )); then
  hp="$(git grep -IlE "$CERTAIN" $(git rev-list --all -n 300) -- . 2>/dev/null | cut -d: -f2- | sort -u | wc -l)"
  (( hp == 0 )) && pass K6.3 "last 300 commits: 0 distinct paths match a certain-secret probe" \
                || fail K6.3 "$hp distinct paths in history match a CERTAIN probe: $(git grep -IlE "$CERTAIN" $(git rev-list --all -n 300) -- . | cut -d: -f2- | sort -u | tr '\n' ' ')"
else skip K6.3 "no history"; fi
# S2: the builder's own machine must not be in the tree or the artifact.
# Built from the environment, never written as a literal: a hardcoded home path
# or username in a public repo is itself the leak this check exists to catch,
# and the first version of this script failed its own S2.1 for exactly that.
USERNAME="$(id -un)"
mapfile -t leaks < <(git grep -In -e "$HOME" -e "$USERNAME" -- . 2>/dev/null | cut -c1-100)
if ((${#leaks[@]} == 0)); then pass S2.1 "no home path or username in tracked files"
else
  fail S2.1 "${#leaks[@]} tracked lines contain the home path or username:"
  for l in "${leaks[@]}"; do printf '                 %s\n' "$l"; done
fi

# ---------------------------------------------------------------- S2 artifact leakage
echo; echo "-- S2 artifact leakage (needs a release build)"
for b in wayfinder-native wayfinder-portal; do
  p="src-tauri/target/release/$b"
  if [[ -x "$p" ]]; then
    for probe in "$HOME" "$(id -un)" ".cargo/registry" ".rustup"; do
      c="$(strings -a "$p" 2>/dev/null | grep -cF -- "$probe")"
      (( c == 0 )) && pass S2.2 "$b: 0 hits for '$probe'" \
                   || fail S2.2 "$b: $c hits for '$probe'"
    done
  else skip S2.2 "$b not built at $p — run the release build first"; fi
done

# ---------------------------------------------------------------- W1 two binaries
echo; echo "-- W1 the installed copy"
inst="$HOME/.local/bin/wayfinder"
shipped=src-tauri/target/release/wayfinder-native
if [[ -f "$inst" && -f "$shipped" ]]; then
  if cmp -s "$inst" "$shipped"; then pass W1.3 "installed copy matches the built binary"
  else
    a=$(date -r "$inst" -Is); b=$(date -r "$shipped" -Is)
    hint W1.3 "installed copy differs — installed $a, built $b"
  fi
else skip W1.3 "installed copy or release build missing"; fi

# ---------------------------------------------------------------- W7 public history
echo; echo "-- W7 public history"
t="$(git ls-files | grep -c "^\.crews/\|^dist/")"
if (( t == 0 )); then pass W7.1 "no .crews/ or dist/ file tracked"
elif [[ "$(git ls-files | grep "^\.crews/\|^dist/")" == ".crews/.gitignore" ]]; then
  pass W7.1 "only .crews/.gitignore tracked, which is the file doing the ignoring"
else fail W7.1 "tracked files under .crews/ or dist/: $(git ls-files | grep '^\.crews/\|^dist/' | tr '\n' ' ')"; fi
grep -q "^\.crews/" .gitignore && grep -q "^dist/" .gitignore \
  && pass W7.2 ".crews/ and dist/ both ignored" || fail W7.2 ".gitignore is missing one of them"

# ---------------------------------------------------------------- W5 packaging
echo; echo "-- W5 packaging"
for f in scripts/make-release-tarball.sh packaging/install.sh packaging/dependencies.sh; do
  [[ -x "$f" ]] && pass W5.1 "$f present and executable" || fail W5.1 "$f missing or not executable"
done
for flag in --check --dry-run --uninstall; do
  grep -q -- "$flag" packaging/install.sh 2>/dev/null \
    && pass W5.2 "installer accepts $flag" || fail W5.2 "installer has no $flag"
done
ft="$(grep -m1 -o "tag: v[0-9.]*" flatpak/com.kadaken.wayfinder.yml 2>/dev/null)"
[[ "$ft" == "tag: v$VER" ]] && pass W5.3 "flatpak $ft" || fail W5.3 "flatpak says '$ft', version is $VER"
judge W5.4 "tarball script RUN with its leak guard, installer --check/--dry-run/--uninstall EXECUTED"

# ---------------------------------------------------------------- judgement-only IDs
echo; echo "-- not mechanical: these must be answered by hand, never by silence"
judge S1    "install from the shipped artifact into a clean user, and the first screen reached"
judge S4    "every destructive operation: reversible, and does the app say so first"
judge S6    "every outbound connection by host and purpose; 'none' must be proved"
judge S7    "download link from a clean browser, checksums against the served file, live security contact"
judge S9    "previous version still downloadable, uninstall executed, what the user does to go back"
judge S11   "previous version's config read by this one"
judge S12   "keyboard-only reach, visible focus, 4.5:1 contrast, at-spi labels"
judge S13   "a test per bug fixed since the last release"
judge S14   "privileges the install needs, and what it registers system-wide"
judge S16   "the core workflows run end to end in the installed build"
judge S17   "disk full, read-only, permission denied, volume unplugged mid-operation, stale mount, 100k entries"
judge S19   "startup cold and warm, memory at idle and at scale, idle CPU, unbounded caches"
judge S10   "the one unmeasured list that ships with the release"
judge K2    "paths, symlinks, and check-then-act races"
judge K3    "every process spawned, with its exact argument list"
judge K4    "where untrusted data enters, and the parser on each boundary"
judge K5    "webview capabilities and anything rendered unescaped"
judge K9    "how an update is learned, signed or merely checksummed, and what happens when it fails"
judge W2    "the portal's D-Bus methods and the hostile run at the file dialog"
judge W3    "move, rename, copy-overwrite, trash, delete — and failure mid-operation"
judge W4    "the GTK headless suite, with its real test result lines"
judge W6    "kadaken.com: update manifest, /get/wayfinder, rendered news, warnings above the button"

echo
echo "=== $PASS pass, $FAIL fail, $HINTS hint, $SKIPS skip"
echo "JUDGE lines are not results. They are the work this script cannot do."
(( FAIL == 0 )) || echo "RELEASE BLOCKED by the FAIL lines above."
exit $(( FAIL > 0 ? 1 : 0 ))
