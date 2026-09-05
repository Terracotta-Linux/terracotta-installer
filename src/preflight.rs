//! What has to be true before a single question is worth asking.
//!
//! Every check here corresponds to a way the install fails *late* — after the
//! disk has been erased, which is the only point at which failing is expensive.
//! A missing `sgdisk` found at the start is a one-line message; the same
//! missing `sgdisk` found in step one of eleven is a machine with no operating
//! system on it.

use std::path::Path;

pub struct Problem {
    pub what: String,
    pub fix: String,
}

/// Programs the installer itself runs. Not `grub-install`, `grub-mkconfig`,
/// `useradd` or `systemd-firstboot`: those run *chrooted into the deployment*,
/// so the copies that matter are the image's, and they arrive with the packages
/// `@kiln/boot/grub2` and `@kiln/profiles/*` install.
const NEEDED: &[(&str, &str)] = &[
    ("kiln", "pacman -S kiln"),
    ("sgdisk", "pacman -S gptfdisk"),
    ("mkfs.ext4", "pacman -S e2fsprogs"),
    ("mkfs.fat", "pacman -S dosfstools"),
    ("blkid", "pacman -S util-linux"),
    ("lsblk", "pacman -S util-linux"),
    ("mount", "pacman -S util-linux"),
    ("umount", "pacman -S util-linux"),
    ("chroot", "pacman -S coreutils"),
    ("partprobe", "pacman -S parted"),
    ("udevadm", "pacman -S systemd"),
    ("wipefs", "pacman -S util-linux"),
];

pub fn check(module_root: &Path) -> Vec<Problem> {
    let mut out = Vec::new();

    // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
    if unsafe { geteuid() } != 0 {
        out.push(Problem {
            what: "terracotta-installer is not running as root".into(),
            fix: "it partitions a disk and runs a build; run it with sudo".into(),
        });
    }

    // GRUB2 through libostree's own backend, with the ESP at /boot/efi, and
    // no BIOS path — pretending there might be would mean an install that
    // completes and then does not boot.
    if !Path::new("/sys/firmware/efi").is_dir() {
        out.push(Problem {
            what: "this machine did not boot in UEFI mode".into(),
            fix: "Kiln's bootloader arrangement is UEFI-only; switch the firmware out of \
                  legacy/CSM mode and boot the installer again"
                .into(),
        });
    }

    for (program, fix) in NEEDED {
        if which(program).is_none() {
            out.push(Problem {
                what: format!("`{program}` is not on PATH"),
                fix: (*fix).into(),
            });
        }
    }

    // The generated configuration opens with `include = ["@kiln/profiles/…"]`,
    // and that reference is resolved against the *host's* module library while
    // `kiln build` runs. Without it the build fails on the first line of
    // the file the installer just wrote, which reads like the installer's bug.
    if !module_root.join("profiles/minimal.toml").is_file() {
        out.push(Problem {
            what: format!("no Kiln module library at {}", module_root.display()),
            fix: "install the `kiln` package, or point KILN_MODULE_DIR at a Kiln \
                  checkout's ./modules"
                .into(),
        });
    }

    // Resolution and package fetching are the two networked stages, and
    // both happen before anything is assembled. A build that gets three minutes
    // in and then cannot reach a mirror is the failure this turns into a
    // sentence.
    if !online() {
        out.push(Problem {
            what: "no network".into(),
            fix: "Kiln resolves packages against real Arch mirrors; connect first \
                  (`iwctl`, or plug in a cable) and start again"
                .into(),
        });
    }

    out
}

/// A DNS lookup, not a ping: ICMP is filtered on plenty of networks that carry
/// HTTPS perfectly well, and what the build needs is name resolution and TCP.
fn online() -> bool {
    std::process::Command::new("getent")
        .args(["ahosts", "geo.mirror.pkgbuild.com"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|c| c.is_file())
}

extern "C" {
    fn geteuid() -> u32;
}
