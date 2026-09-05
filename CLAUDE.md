# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```console
$ cargo build                       # debug
$ cargo build --release
$ cargo test                        # 22 tests, none of which touch a disk
$ cargo test the_bootloader_comes_after_the_deploy       # one test by name
$ cargo test config::tests -- --nocapture                # one module, with output
$ cargo fmt && cargo clippy --all-targets                # both are expected to be clean
```

`sudo terracotta-installer --dry-run` asks every question and writes nothing; it is the
only way to exercise the TUI without a disk. It cannot be driven non-interactively — the
interview needs a real terminal (crossterm raw mode, minimum 60x18) — so verify screen
changes by running it yourself, not by piping.

`--dry-run` skips preflight's hard blocks (root, UEFI, network, module library) with a
warning, so it works from a normal desktop.

## The seam this program is written against

`terracotta-installer` is a *separate program* from Kiln, not a Kiln subcommand. Three
rules follow, and breaking any of them breaks that arrangement:

- **Link no `kiln-*` crate.** Every interaction with Kiln goes through the `kiln` binary
  and the flags it documents (`kiln sysroot init`, `kiln build --sysroot --config`,
  `kiln deploy <gen>`). Linking `kiln-ostree` would let the installer deploy a generation
  Kiln does not know about, and then the machine's build record and its bootloader disagree.
  The one dependency is `crossterm`.
- **Kiln does not depend on this.** `kiln` lists `terracotta-installer` as an `optdepends`;
  the dependency never runs the other way.

Because the two repositories can drift silently, this crate carries a *copy* of Kiln's
module names (`catalog.rs`) and of what a `system.toml` looks like (`config.rs`), and three
tests check those copies against a real Kiln: `kiln_accepts_what_the_installer_writes`
(runs the real `kiln show` over a rendered config), `every_reference_resolves_against_a_real_library`,
`headings_do_not_outlive_their_groups`. They find Kiln via `catalog::probe`, in order:
`KILN_MODULE_DIR`, the installed `/usr/share/kiln/modules` plus `kiln` on `PATH`, then a
sibling `../kiln` checkout. They skip with a message when none is present.

## Architecture

```
main.rs  preflight::check → interview::ask → Answers → steps::Installer::install → summary
                              (nothing is written before the review screen is accepted)
```

- **`preflight.rs`** — every check corresponds to a way the install fails *after* the disk is
  erased. Its `Problem`s are hard blocks with no warning channel; do not add a check whose
  right response is "continue anyway".
- **`interview.rs`** — a state machine, not a run of prompts, because Esc goes back thirteen
  screens. Produces `Answers`, touches nothing.
- **`steps.rs`** — the only module that writes anything. `STEPS` is the fixed order the
  install runs in, and its order is asserted by tests rather than only commented.
- **`config.rs`** — `Answers` → `system.toml`. Nothing here writes to the file that the
  interview did not ask for.
- **`run.rs`** — every command goes through `Runner`, which interleaves stdout and stderr into
  `/var/log/terracotta-installer.log` and into a draw callback.
- **`tui.rs`** — hand-written ANSI; crossterm only for raw mode, the alternate screen and key
  decoding. Every widget returns `Answer<T> = Result<T, Nav>`, which is what makes Esc work.
- **`block.rs`** — `lsblk -P` parsing. Disks that must not be wiped are *shown and disabled*,
  never hidden.

### Two decisions that govern where new code goes

**The line Kiln draws.** *If it changes, do you need a new image and a reboot?* Yes → it
belongs in `config.rs`, in the generated `system.toml`, and Kiln owns it from then on. No →
it is done once in the deployment's `/etc` by `steps.rs` (hostname, timezone, accounts,
`fstab`). Accounts and passwords never reach the configuration.

**`/mnt` is not the installed machine's `/`.** `/mnt` is the *physical* root; from the first
boot onward `/` is the deployment at
`/mnt/ostree/deploy/kiln/deploy/<checksum>.<serial>`, and anything left in `/mnt/etc`
reappears at `/sysroot/etc`, where nothing looks. Files the machine must boot with go into
the deployment (`deployment()` finds it), where libostree's three-way merge carries them into
every later generation. The configuration is *staged* at `/mnt/etc/kiln` for `kiln build
--config` to read and then **moved** into the deployment by the `etc` step.

### Things that bite

- **`run::read_only`** is the allowlist of commands that still execute under `--dry-run`.
  Adding a command that writes to that list turns a dry run into one that partitions a disk.
- **`grub-install` and `grub-mkconfig` run chrooted into the deployment**, so the copies that
  matter are the *image's*, not the live medium's — including `efibootmgr`, which
  `grub-install` executes to write the NVRAM entry and `@kiln/boot/grub2` installs alongside
  `grub`. The `--removable` fallback install is required and runs first; the named install is
  best-effort, so a missing `efibootmgr` costs a firmware menu entry, not the install.
- **`enter()`'s bind mounts**: `/boot` must be `--rbind` (the ESP is a mount underneath it)
  and `/sysroot` must be bound at all (a deployment's `/ostree` is a symlink into it, and
  `grub-mkconfig` writes a `grub.cfg` that boots nothing without it).
- **Never re-run `kiln deploy 1` "to be safe"** after the bootloader step: an
  already-deployed generation takes `set_default`, which disarms the boot counter on
  purpose.
- **Exit codes follow `kiln`'s own taxonomy**: 3 is a build failure, 4 is the system
  refusing.
