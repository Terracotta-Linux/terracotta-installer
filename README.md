<div align="center">

# 💿 Terracotta Installer

**An interactive terminal installer for [Terracotta Linux](https://github.com/Terracotta-Linux).**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Latest release](https://img.shields.io/github/v/release/Terracotta-Linux/terracotta-installer?color=orange)](https://github.com/Terracotta-Linux/terracotta-installer/releases)
[![Release build](https://img.shields.io/github/actions/workflow/status/Terracotta-Linux/terracotta-installer/release.yml?label=release%20build)](https://github.com/Terracotta-Linux/terracotta-installer/actions/workflows/release.yml)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-B7410E?logo=rust&logoColor=white)](Cargo.toml)
[![Last commit](https://img.shields.io/github/last-commit/Terracotta-Linux/terracotta-installer)](https://github.com/Terracotta-Linux/terracotta-installer/commits/main)

</div>

---

Terracotta Installer puts a [Kiln](https://github.com/Terracotta-Linux/kiln)-managed system on a
disk. It partitions the target, initializes a Kiln sysroot, builds generation 1, deploys it,
installs GRUB, and creates your first account.

It asks every question up front. Nothing touches the disk until you accept the review screen,
and <kbd>Esc</kbd> goes back a screen at any point before that.

```console
$ sudo terracotta-installer
```

## ✨ Highlights

|  | |
|---|---|
| 🖥️ **Runs anywhere a terminal does** | Hand-written ANSI over a single dependency. Works on a serial console at 80×24. |
| ↩️ **Every answer is correctable** | The interview is a state machine, not a straight run of prompts. <kbd>Esc</kbd> from the last screen to the first, and your answers are still there. |
| 🔒 **Optional full-disk encryption** | LUKS2 over the root partition, unlocked by the initramfs from declarative kernel arguments. |
| 🧪 **A real dry run** | `--dry-run` asks everything and writes nothing, printing the exact command list a real run would execute. |
| 🛡️ **Careful about your disks** | A disk with anything mounted from it is shown, explained, and not selectable. Erasing one means typing its name. |
| 📝 **Nothing is silent** | Every command and every line it prints goes to `/var/log/terracotta-installer.log`. |

## 🚀 Getting started

### From the live ISO

The [Terracotta ISO](https://github.com/Terracotta-Linux/terracotta-iso) ships the installer and
starts it automatically on tty1. Boot the ISO and you are already in it.

### From the package

Grab the latest `.pkg.tar.zst` from the
[releases page](https://github.com/Terracotta-Linux/terracotta-installer/releases/latest) and
install it:

```console
$ sudo pacman -U terracotta-installer-x86_64.pkg.tar.zst
```

Each release also ships a `.sha256` file if you want to verify the download first.

### Running it

```console
$ sudo terracotta-installer                     # the real thing
$ sudo terracotta-installer --dry-run           # ask everything, write nothing
$ terracotta-installer --help
```

| Option | What it does |
|---|---|
| `--dry-run` | Runs the whole interview, then prints the plan instead of executing it. Only `lsblk`, `blkid` and `localectl` actually run. |
| `--module-root DIR` | Use a module library other than `/usr/share/kiln/modules`. Also settable with `KILN_MODULE_DIR`. |
| `--version` | Print the version and exit. |
| `-h`, `--help` | Print usage and exit. |

Exit codes follow Kiln's own taxonomy: `0` on success, `3` when the image build fails, `4` when
the system refuses (a failed preflight check, a bad argument, or any other step failing).

### ⌨️ Keys

| Key | Action |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> (or <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>N</kbd>) | Move |
| *type anything* | Filter the list (timezones, locales, keymaps, ...) |
| <kbd>Space</kbd> | Toggle a module on the multi-select screen |
| <kbd>Enter</kbd> | Confirm |
| <kbd>Ctrl</kbd>+<kbd>U</kbd> | Clear a text field |
| <kbd>Esc</kbd> | Back one screen |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Quit (free until the review screen is accepted) |

Once the install starts it runs to completion. There is no abort key, and the footer says so.

## ⚙️ What it asks

Fourteen screens, in order:

**Disk** → **encryption** → **type the disk name to confirm** → **hostname** → **timezone** →
**locale** → **console keymap** → **profile** → **extra modules** → **extra packages** →
**username** → **user password** → **root password** → **review**

A few notes on the less obvious ones:

- **Profile** is `workstation`, `server` or `minimal`. Each one already picks a kernel, which is
  why there is no separate kernel question: two kernel modules in one configuration are a
  conflict by design.
- **Modules** are the rest of Kiln's library, grouped by namespace, with anything your profile
  already covers left out. Every one of them is a single line of `include` you can add or remove
  later with `kiln apply`.
- **Root password** is optional whenever your configuration grants `wheel` sudo. Leave it empty
  and the root account stays locked, which is what most Arch installs do now.

Only the choices that actually need a new image and a reboot end up in `/etc/kiln/system.toml`.
The hostname, timezone, accounts and `fstab` are written once into the deployment's `/etc`,
where libostree's three-way merge carries them into every later generation. No account,
password or passphrase ever reaches the configuration.

## 🧱 How it works

Kiln has no `install` verb and never will. This is a separate program with its own release
cycle, and it reaches Kiln only through the `kiln` binary and the flags Kiln documents. That
seam is the whole contract between the two.

| # | Step | What happens |
|---|---|---|
| 1 | `partition` | GPT: 1 GiB ESP (`ef00`), 1 GiB ext4 `/boot`, the rest for `/`. LUKS2 goes over the root partition first if you asked for encryption. |
| 2 | `format` | `mkfs.fat -F 32` for the ESP, `mkfs.ext4` for `/boot` and `/`. |
| 3 | `mount` | The target at `/mnt`, then `/mnt/boot`, then `/mnt/boot/efi`. |
| 4 | `sysroot` | `kiln sysroot init /mnt` |
| 5 | `configure` | Writes `/mnt/etc/kiln/system.toml` for the build to read. |
| 6 | `build` | `kiln --sysroot /mnt --config /mnt/etc/kiln build` |
| 7 | `deploy` | `kiln --sysroot /mnt deploy 1` (`build` commits, it does not deploy) |
| 8 | `etc` | Writes `/etc/fstab` into the deployment and **moves** `/etc/kiln` into it. |
| 9 | `bootloader` | `grub-install` and `grub-mkconfig`, chrooted into the deployment. |
| 10 | `accounts` | Hostname, timezone, the first user, passwords. |
| 11 | `finish` | `umount -R /mnt`, and closes the LUKS mapping if there is one. |

Three of those rows are load-bearing and easy to get backwards:

<details>
<summary><b>Why <code>sysroot init</code> has to come before <code>build</code></b></summary>

`kiln build --sysroot` creates `ostree/repo` by itself, so building into an uninitialized target
*succeeds* and then fails at the deploy, several minutes and several hundred megabytes after the
mistake.
</details>

<details>
<summary><b>Why the bootloader comes after the deploy</b></summary>

A `--sysroot` deploy writes BLS entries and no `grub.cfg`, because libostree's grub2 backend runs
`grub-mkconfig` chrooted with a host-absolute output path, which cannot work from outside `/`. So
the installer runs it, and it has to run afterwards: `grub-mkconfig` sources `/etc/grub.d`, and
the two fragments that matter live inside the image. libostree's `15_ostree` turns BLS entries
into menu entries, and Kiln's `09_kiln_boot_counter` is what makes automatic rollback happen.

The generated config does not stay at `/boot/grub/grub.cfg` either. libostree maintains
`/boot/loader.N/grub.cfg` and swaps the `loader` symlink, so a regular file where GRUB looks is a
config frozen at one bootversion, and a machine that boots exactly once more.
</details>

<details>
<summary><b>Why <code>/mnt/etc</code> is not the installed machine's <code>/etc</code></b></summary>

`/mnt` is the *physical* root. From the first boot onward `/` is the deployment, so anything left
in `/mnt/etc` reappears at `/sysroot/etc`, which nothing reads. A configuration left there is a
machine that boots with no `/etc/kiln` at all and a `kiln check` that answers *no configuration at
/etc/kiln*. Step 8 moves the config root into the deployment, next to `fstab` and the accounts.

Moved, not copied: a second, invisible `system.toml` is stale the moment the real one is edited.
</details>

### 🔐 Encryption

When you ask for it, encryption is LUKS2 over `/` and nothing else. `/boot` and the EFI system
partition stay in the clear.

GRUB's own LUKS support exists, but it is easy to get wrong and unnecessary here. Nothing before
the kernel needs to read anything encrypted: dracut's `crypt` module unlocks root from
`rd.luks.uuid=` and `rd.luks.name=` on the kernel command line, the same fully declarative place
`root=` already lives, before `ostree-prepare-root` ever runs.

The kernel arguments, `dracut_modules = ["crypt"]` and the `cryptsetup` package are written all
three together or not at all. A config with the arguments and no package fails during dracut's own
module check; a config with the package and no arguments boots an image that cannot find its own
root. The passphrase itself follows the same rule as an account password: piped to
`cryptsetup --key-file=-` over stdin, recorded in the log only as "N bytes, not logged", and never
written into `/etc/kiln`.

## 📋 Requirements

The installer refuses to start unless all of this is true, because every item is a way to fail
*after* the disk has been erased:

| | |
|---|---|
| 🔌 **UEFI** | Kiln's bootloader arrangement is UEFI-only. No BIOS or CSM path. |
| 👑 **root** | It partitions a disk and runs a build. |
| 🌐 **Network** | Kiln resolves packages against real Arch mirrors. If NetworkManager is running, you get one shot at `nmtui` before it gives up. |
| 📚 **A Kiln module library** | `/usr/share/kiln/modules`, from the `terracotta-kiln` package. |
| 🧰 **Tools on `PATH`** | `kiln`, `sgdisk`, `mkfs.ext4`, `mkfs.fat`, `blkid`, `lsblk`, `mount`, `umount`, `chroot`, `partprobe`, `udevadm`, `wipefs`. Plus `cryptsetup` if you choose encryption, which is checked when you choose it. |

You also need a terminal of at least 60×18 and a target disk of at least 4 GiB with nothing
mounted from it.

`--dry-run` skips the hard blocks with a warning so you can exercise the interview from an
ordinary desktop. It still needs a module library, since there are no profiles to offer without
one; point `--module-root` at a Kiln checkout's `./modules` if the package is not installed.

## 📦 Building from source

Rust 1.85 or newer. One dependency, `crossterm`, for raw mode, the alternate screen and key
decoding. Styling is hand-written ANSI, the way `kiln` itself writes it.

```console
$ cargo build --release
$ cargo test
```

To build the Arch package yourself:

```console
$ cd packaging
$ makepkg --nodeps
$ sudo pacman -U terracotta-installer-*.pkg.tar.zst
```

`--nodeps` is there because the PKGBUILD depends on `terracotta-kiln`, which lives in Terracotta's
own releases rather than the official Arch repositories, so `pacman` cannot resolve it. Install
[Kiln](https://github.com/Terracotta-Linux/kiln) first and the dependency is satisfied.

Tagging a release as `v$pkgver` triggers the `Release` workflow, which builds the package in an
Arch container and attaches it to the GitHub release along with its checksum.

## 🛠️ Development

```console
$ cargo test                                       # 31 tests, none of which touch a disk
$ cargo fmt --check && cargo clippy --all-targets  # both are expected to be clean
$ ./target/debug/terracotta-installer --dry-run    # the only way to exercise the TUI
```

A dry run needs no root, since it skips the hard preflight checks with a warning. It does need a
real terminal, so it cannot be driven by piping input: verify screen changes by running it and
looking at them.

| Module | Responsibility |
|---|---|
| `main.rs` | Arguments, the panic hook that restores the terminal, and the final screens. |
| `preflight.rs` | The checks that have to pass before a question is worth asking. |
| `interview.rs` | The fourteen screens, as a state machine. Touches nothing. |
| `steps.rs` | The only module that writes anything. |
| `config.rs` | `Answers` into `system.toml`. |
| `catalog.rs` | The choices on offer, plus locales, timezones and keymaps read from the running system. |
| `block.rs` | `lsblk` parsing and the rules for which disks may be erased. |
| `run.rs` | Every command goes through here, interleaving both streams into the log and the screen. |
| `tui.rs` | Raw mode, the alternate screen, and the widgets. |

This program keeps a deliberate *copy* of Kiln's module names and of what a `system.toml` looks
like, because the two repositories release separately. Five tests check that copy against a real
Kiln:

| Test | What it proves |
|---|---|
| `kiln_accepts_what_the_installer_writes` | A rendered config survives the real `kiln show`: discovery, parse, include graph, merge, validate. |
| `kiln_accepts_the_encrypted_configuration` | The same, over the LUKS branch, which the plain config proves nothing about. |
| `every_reference_resolves_against_a_real_library` | Every `@kiln/...` on offer names a module that exists. |
| `headings_do_not_outlive_their_groups` | A namespace whose modules all vanished does not render as an empty heading. |
| `sudo_matches_what_the_profiles_actually_include` | The installer's idea of which profiles grant `wheel` sudo, which is what lets the root password be left empty, matches the profiles themselves. |

They look for Kiln in `KILN_MODULE_DIR`, then the installed package plus `kiln` on `PATH`, then a
sibling `../kiln` checkout, and skip with a message when none of those is there. Run them
somewhere Kiln is installed, which is how a stale copy gets caught.

## 🤝 Contributing

Contributions are welcome, in whatever form suits you: bug fixes, new features, documentation,
testing on hardware nobody here owns, or just a sharper sentence somewhere.

There is no contributor agreement and no template to fill in. The only expectations are the ones
the project already holds itself to:

1. `cargo fmt` and `cargo clippy --all-targets` come back clean.
2. `cargo test` passes.
3. Anything that changes a screen has been run under `--dry-run` and looked at.

Open a pull request against `main` and say what you changed and why.

## 🐛 Issues and bug reports

**Issues and bug reports are always welcome.** If something breaks, behaves unexpectedly, or
simply reads wrong, please
[open an issue](https://github.com/Terracotta-Linux/terracotta-installer/issues).

An install that failed leaves everything it ran in `/var/log/terracotta-installer.log`. On a live
medium that is tmpfs, so copy it somewhere before rebooting. Attaching it, along with your
hardware and the step the ✘ landed on, turns most reports into a fix.

## 🤖 AI assistance

Parts of this project's code and documentation were written with the help of AI coding
assistants. `CLAUDE.md` in the repository root is the guidance they work from.

Everything is reviewed, built and tested by a human before it lands, and the project's design
decisions are its maintainer's rather than a model's.

## 📄 License

[MIT](LICENSE) © 2026 Abdullah AL-Swedi
