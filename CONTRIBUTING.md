# Poke around

Wayfinder is small enough to change, and that is the point of it being public
this early. You do not need permission to look at anything, and you do not need
to be a Rust programmer to be useful.

## The quickest useful things

- **Use it and say where it is wrong.** Open an
  [issue](https://github.com/Kadaken/wayfinder/issues) with what you did, what
  you expected, and what happened. A screenshot of a window that looks wrong is
  a complete bug report.
- **Tell me what is missing.** The repository keeps an honest list of what is
  not written yet. If the thing you need is on it, say so and it moves up; if
  it is not on it, it should be.
- **Argue about how it should behave.** The
  [discussions](https://github.com/Kadaken/wayfinder/discussions) tab is for
  that, and a design argument you win changes the product.
- **Break the file dialog on purpose.** See [`SECURITY.md`](SECURITY.md) — that
  is the half of this program worth attacking, and nobody has really tried.

## Building it

Needs a Rust toolchain and the GTK4 development packages; the exact list is
under **Requirements** in [`README.md`](README.md).

```bash
git clone https://github.com/Kadaken/wayfinder.git
cd wayfinder
./packaging/install.sh          # builds both binaries, installs under ~/.local
./packaging/install.sh --check  # or just say what this machine is missing
```

`./packaging/install.sh --uninstall` takes all of it back off again.

Two binaries come out of one source tree: `wayfinder-native`, the file manager,
and `wayfinder-portal`, the file dialog other programs open. They share their
logic; the split is only in how each one is started.

## Running the tests

```bash
cd src-tauri
cargo test --release --bins
```

The long one is a headless GTK suite that drives real windows; it takes about
eight minutes and it is the one that catches layout regressions. No test should
need anything from your home folder — if one does, that is a bug, so report it.

## Sending a change

1. Branch off `main`.
2. Keep one change per branch, and say in the commit message what was wrong
   before, not only what the code now does.
3. Add or adjust a test if the change is something a test could have caught.
4. Run `cargo test --release --bins` and `cargo clippy` before you open the
   pull request. Say in the pull request if something failed and why you think
   it is unrelated — that is more useful than a green tick you had to fight for.
5. Open the pull request against `main`.

I would rather have a rough patch that shows what you mean than a polished one
you waited six weeks to feel ready about.

## Licence

Wayfinder is **GPL-3.0-or-later**. Anything you contribute is under the same
licence, and you keep your own copyright in it. You are free to fork it, change
it and redistribute your fork; that is what the licence is for.
