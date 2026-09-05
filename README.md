# terracotta-installer

An interactive installer for Terracotta Linux. It
partitions a disk, initializes a Kiln sysroot on it, builds generation 1, deploys it,
installs GRUB, and creates the first account.

```console
$ sudo terracotta-installer
```

## What it does, in order

| | |
|---|---|
| 1 `partition` | GPT: 1G ESP (`ef00`), 1G ext4 `/boot`, the rest ext4 `/` |
| 2 `format` | `mkfs.fat -F 32`, `mkfs.ext4` twice |
| 3 `mount` | the target at `/mnt`, then `/mnt/boot`, then `/mnt/boot/efi` |
| 4 `sysroot` | `kiln sysroot init /mnt` |
| 5 `configure` | writes `/mnt/etc/kiln/system.toml`, for the build to read |
| 6 `build` | `kiln build --sysroot /mnt --config /mnt/etc/kiln` |
| 7 `deploy` | `kiln deploy --sysroot /mnt 1` — `build` commits, it does not deploy |
| 8 `etc` | `/etc/fstab` in the deployment, and `/etc/kiln` moved into it |
| 9 `bootloader` | `grub-install` and `grub-mkconfig`, chrooted into the deployment |
| 10 `accounts` | hostname, timezone, the first user, passwords |
| 11 `finish` | `umount -R /mnt` |

Three rows are easy to get wrong and all of them are load-bearing.

**`sysroot init` before `build`.** `kiln build --sysroot` creates `ostree/repo` by itself, so
building into an uninitialized target *succeeds* and then fails at the deploy, several
minutes and several hundred megabytes after the mistake.

**The bootloader after the deploy.** A `--sysroot` deploy writes BLS entries and no
`grub.cfg`: libostree's grub2 backend runs `grub-mkconfig` chrooted into the deployment with
a host-absolute output path, which cannot work from outside `/`. So the installer runs it,
and it has to run afterwards — `grub-mkconfig` sources `/etc/grub.d`, and the two
fragments that matter live inside the image. libostree's `15_ostree` turns BLS entries into
menu entries; Kiln's `09_kiln_boot_counter` is what makes automatic rollback happen.

**`/mnt/etc` is not the installed machine's `/etc`.** It is tempting to think of the
configuration as "just a file at `/mnt/etc/kiln/system.toml`", and that is true of the file
`kiln build --config` reads and false of the file the machine boots with. `/mnt` is the
*physical* root; from the first boot onward `/` is the deployment, and everything left in
`/mnt/etc` reappears at `/sysroot/etc`, which nothing reads. A configuration left there is a
machine that boots with no `/etc/kiln` at all and a `kiln check` that answers *no
configuration at /etc/kiln* — the interview's answers, gone. So step 8 **moves** the config
root into the deployment's `/etc`, where libostree's three-way merge carries it into every
generation after this one, next to `fstab` and the accounts. Moved, not copied: a second,
invisible `system.toml` is stale the moment the real one is edited.

## Running it

It refuses to start unless it is root, booted in UEFI mode, on the network, with a Kiln
module library present and every tool it needs on `PATH` — because every one of those is a
way to fail *after* the disk has been erased.

```console
$ sudo terracotta-installer --dry-run     # ask everything, write nothing, print the plan
$ sudo terracotta-installer --help
```

`--dry-run` still runs `lsblk`, `blkid` and `localectl`, because the plan is wrong without
their answers, and runs nothing else. Everything that happens is appended to
`/var/log/terracotta-installer.log`, which on a live medium is tmpfs — copy it somewhere
before rebooting if the install failed.

Esc goes back a screen and Ctrl-C leaves. Nothing is written until the review screen is
accepted.

## Building it

```console
$ cargo build --release
$ cargo test
```

One dependency, `crossterm`, for raw mode, the alternate screen and key decoding. Styling is
hand-written ANSI, the way `kiln` itself writes it.

Three tests check this program's copies against a real Kiln:

| | |
|---|---|
| `kiln_accepts_what_the_installer_writes` | renders a `system.toml` and runs the real `kiln show` over it — the whole frontend: discovery, parse, include graph, merge, validate, `Manifest`. Catches a renamed key, or `include` ending up after a table header. |
| `every_reference_resolves_against_a_real_library` | every `@kiln/...` this program offers names a file the library actually has |
| `headings_do_not_outlive_their_groups` | a namespace whose modules all vanished does not render as an empty heading |

They look for Kiln in three places, in this order: `KILN_MODULE_DIR`, the installed package
at `/usr/share/kiln/modules` plus `kiln` on `PATH`, and a sibling `../kiln` checkout. They
skip with a message when none of those is there — which is how a stale copy gets caught: run
them somewhere Kiln is actually installed.

## What it does not do

No LUKS, no btrfs, no swap partition, no dual-boot, no installing into partitions somebody
else made. One disk, erased, laid out the one way settled on above. Every one of those is a
real thing somebody will want and none of them is a thing to add casually — a swap partition
in particular is the only choice on that list that cannot be undone later, which is why it is
not offered and `zram-generator` or a swapfile is the answer instead.
