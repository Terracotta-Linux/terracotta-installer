//! The questions, and the order they are asked in.
//!
//! This is a state machine rather than a straight run of prompts, for one
//! reason: **Esc goes back**. Somebody who mistypes a hostname on screen three
//! and notices on screen eleven should not have to restart an installer, and an
//! installer that cannot be corrected is one people restart by rebooting.
//!
//! Nothing here touches the disk. The interview produces `Answers` and returns;
//! `steps.rs` is the only module that writes anything, and it does not run
//! until the review screen has been accepted.

use crate::block::{self, Disk};
use crate::catalog::{self, Entry};
use crate::run::Runner;
use crate::tui::{Nav, Opt, Page, Ui, ACCENT, BOLD, DANGER, DIM, OK, RESET, WARN};
use std::path::Path;

/// Everything the install needs to know.
pub struct Answers {
    pub disk: Disk,
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keymap: String,
    pub profile: String,
    pub modules: Vec<String>,
    pub packages: Vec<String>,
    pub username: String,
    pub user_password: String,
    /// Empty means the root account is left locked, which is only offered when
    /// something in the configuration grants `wheel` sudo.
    pub root_password: String,
    /// Whether the root partition is LUKS2. `/boot` and the ESP are always
    /// written in the clear — GRUB reads neither through its own (fragile)
    /// LUKS support, only the initramfs unlocks anything, and it does that
    /// from the kernel cmdline, not from a passphrase GRUB has to know.
    pub encrypt: bool,
    /// Asked once, twice, only when `encrypt` is set. Piped to `cryptsetup`
    /// over stdin and never written anywhere — not the log, not the
    /// configuration. Empty when `encrypt` is false.
    pub passphrase: String,
    /// Not asked — filled in by `steps.rs` once `mkfs.ext4` has made one, and
    /// read back out of `blkid`. It is in `Answers` because it is an input to
    /// `config.rs`, and `config.rs` should have exactly one argument.
    pub root_uuid: String,
    /// The LUKS container's own UUID — not the filesystem's — filled in by
    /// `steps.rs` from `cryptsetup luksUUID` when `encrypt` is set. `None`
    /// otherwise. This is what `rd.luks.uuid=` has to name.
    pub luks_uuid: Option<String>,
}

impl Answers {
    /// Whether an unlocked root account is optional.
    fn sudo(&self) -> bool {
        grants_sudo(&self.profile, &self.modules)
    }
}

/// The module every profile but `minimal` already includes, and the one a
/// `minimal` install has to be told to add.
const WHEEL_SUDO: &str = "@kiln/security/wheel-sudo";

/// Whether the configuration about to be written lets `wheel` sudo, and so
/// whether root may be left locked.
///
/// Kiln does not manage accounts, so this is the installer reasoning about the
/// configuration it is about to write rather than Kiln reasoning about users —
/// which makes it a *third* copy of what the module library says, after
/// `catalog.rs` and `config.rs`. A profile that quietly dropped `wheel-sudo`
/// would have the root-password screen call itself "Optional" and hand
/// somebody a machine with no root password and no sudo, so the copy is
/// checked against a real library in this module's tests.
fn grants_sudo(profile: &str, modules: &[String]) -> bool {
    profile != "@kiln/profiles/minimal" || modules.iter().any(|m| m == WHEEL_SUDO)
}

const SCREENS: usize = 14;

/// Ask everything. `Ok(None)` is Ctrl-C at any point: nothing has been written,
/// so quitting is always free.
pub fn ask(ui: &mut Ui, run: &mut Runner, module_root: &Path) -> Result<Option<Answers>, String> {
    let disks = block::disks(run)?;
    if disks.iter().all(|d| d.busy.is_some()) {
        return Err(if disks.is_empty() {
            "no disks found".into()
        } else {
            "every disk on this machine is in use or too small; terracotta-installer has nothing it \
             may erase"
                .into()
        });
    }
    let profiles = catalog::profiles(module_root);
    let extras = catalog::extras(module_root);
    if profiles.is_empty() {
        return Err(format!(
            "no profiles in the module library at {} — the `kiln` package ships them at \
             /usr/share/kiln/modules",
            module_root.display()
        ));
    }
    let locales = catalog::locales();
    let zones = catalog::timezones();
    let maps = catalog::keymaps(run);

    let mut a = Answers {
        disk: disks[0].clone(),
        hostname: "terracotta".into(),
        timezone: "UTC".into(),
        locale: locales[0].clone(),
        keymap: maps[0].clone(),
        profile: profiles[0].module.into(),
        // The defaults live in `a` from the start rather than being rebuilt by
        // `modules_screen` every time it is drawn: that screen pre-checks from
        // `a.modules`, so a fresh copy of the defaults there would silently
        // discard everything chosen the first time round on the way back.
        modules: catalog::defaults(&extras),
        packages: Vec::new(),
        username: String::new(),
        user_password: String::new(),
        root_password: String::new(),
        encrypt: false,
        passphrase: String::new(),
        root_uuid: String::new(),
        luks_uuid: None,
    };

    let mut at = 0usize;
    loop {
        let step = (at + 1, SCREENS);
        let outcome = match at {
            0 => disk_screen(ui, step, &disks, &mut a),
            1 => encrypt_screen(ui, step, &mut a),
            2 => confirm_screen(ui, step, &a),
            3 => hostname_screen(ui, step, &mut a),
            4 => pick(
                ui,
                step,
                "Timezone",
                "Used for the clock, not for the image.",
                &zones,
                &mut a.timezone,
            ),
            5 => pick(
                ui,
                step,
                "Locale",
                "Compiled into the image by a build script.",
                &locales,
                &mut a.locale,
            ),
            6 => pick(
                ui,
                step,
                "Console keymap",
                "The virtual console; a desktop sets its own.",
                &maps,
                &mut a.keymap,
            ),
            7 => profile_screen(ui, step, &profiles, &mut a),
            8 => modules_screen(ui, step, &extras, &mut a),
            9 => packages_screen(ui, step, &mut a),
            10 => username_screen(ui, step, &mut a),
            11 => password_screen(ui, step, &mut a),
            12 => root_password_screen(ui, step, &mut a),
            _ => review_screen(ui, step, &a),
        };
        match outcome {
            Ok(()) => {
                at += 1;
                if at == SCREENS {
                    return Ok(Some(a));
                }
            }
            Err(Nav::Back) => at = at.saturating_sub(1),
            Err(Nav::Quit) => return Ok(None),
        }
    }
}

type Screen = Result<(), Nav>;

fn disk_screen(ui: &mut Ui, step: (usize, usize), disks: &[Disk], a: &mut Answers) -> Screen {
    // The first screen, and the one place Esc has nowhere to go back to, so
    // the legend does not offer it.
    let page = Page::new(step, "Which disk should Kiln be installed on?")
        .note("Everything on the disk you choose is erased. Disks holding a mounted")
        .note("filesystem — the medium you booted from, most likely — cannot be chosen.")
        .help("↑↓ move · type to filter · enter select · ctrl-c quit");
    let options: Vec<Opt> = disks
        .iter()
        .map(|d| {
            let model = if d.model.is_empty() {
                "—".to_string()
            } else {
                d.model.clone()
            };
            let kind = match (d.removable, d.transport.as_str()) {
                (true, _) => "removable".to_string(),
                (_, "") => String::new(),
                (_, t) => t.to_string(),
            };
            let note = match &d.busy {
                Some(why) => format!("{} · {model} · {why}", d.size()),
                None if kind.is_empty() => format!("{} · {model}", d.size()),
                None => format!("{} · {model} · {kind}", d.size()),
            };
            let opt = Opt::new(d.path.clone(), note);
            match (&d.busy, d.path == a.disk.path) {
                (Some(_), _) => opt.disabled(),
                (None, true) => opt.checked(),
                (None, false) => opt,
            }
        })
        .collect();
    let i = ui.select(&page, &options)?;
    a.disk = disks[i].clone();
    Ok(())
}

fn confirm_screen(ui: &mut Ui, step: (usize, usize), a: &Answers) -> Screen {
    let d = &a.disk;
    let mut page = Page::new(step, format!("Erase {}?", d.path)).note(format!(
        "{} · {}",
        d.size(),
        if d.model.is_empty() {
            "unknown model"
        } else {
            &d.model
        }
    ));
    if d.removable {
        // Not refused — an external SSD is a perfectly good place to install a
        // system, and refusing would be Kiln deciding what your disk is for.
        // Said out loud, though, because the other thing a removable disk is
        // very often is the stick this installer booted from.
        page = page.note(format!(
            "{WARN}This is a removable disk. Make sure it is not the medium you booted.{RESET}"
        ));
    }
    let root_row = if a.encrypt {
        format!("  {ACCENT}3{RESET}  rest LUKS2 → ext4        → /")
    } else {
        format!("  {ACCENT}3{RESET}  rest ext4                 → /")
    };
    let page = page
        .note(String::new())
        .note(format!(
            "{DANGER}The partition table and every filesystem on this disk are destroyed.{RESET}"
        ))
        .note("There is no undo, and nothing else on the machine is touched.")
        .note(String::new())
        .note(format!("Kiln will lay it out as {DIM}follows{RESET}:"))
        .note(format!(
            "  {ACCENT}1{RESET}  1G   EFI system partition   → /boot/efi"
        ))
        .note(format!(
            "  {ACCENT}2{RESET}  1G   ext4                   → /boot"
        ))
        .note(root_row);
    ui.typed_confirm(&page, &d.word())
}

/// Whether the root partition is LUKS2, and the passphrase if so.
///
/// One screen, not two: the select and the secret prompt are both driven from
/// here so that Esc from the passphrase field returns to the yes/no choice
/// rather than to the previous top-level screen, and picking "No" never draws
/// a passphrase field at all. `/boot` and the ESP are never offered — see the
/// note on `Answers::encrypt`.
fn encrypt_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let page = Page::new(step, "Encrypt the disk?")
        .note("LUKS2 over the root partition. /boot and the EFI system partition stay")
        .note("in the clear — GRUB never needs to unlock anything, the initramfs does")
        .note("that at boot from a passphrase you type there, not from this one.");
    loop {
        let no = Opt::new("No", "root is written in the clear");
        let yes = Opt::new(
            "Yes",
            "cryptsetup luksFormat; unlocked at every boot with a passphrase",
        );
        let options = if a.encrypt {
            vec![no, yes.checked()]
        } else {
            vec![no.checked(), yes]
        };
        let i = ui.select(&page, &options)?;
        a.encrypt = i == 1;
        if !a.encrypt {
            a.passphrase.clear();
            return Ok(());
        }
        // Asked here rather than in `preflight`, which has no warning channel:
        // a missing `cryptsetup` cannot fail an unencrypted install, and a hard
        // block at startup would refuse a machine that would have installed
        // fine.
        if crate::preflight::which("cryptsetup").is_none() {
            let missing = Page::new(step, "Encrypt the disk?")
                .note(format!(
                    "{DANGER}`cryptsetup` is not on PATH, so nothing on this medium can{RESET}"
                ))
                .note(format!("{DANGER}create a LUKS2 container.{RESET}"))
                .note(String::new())
                .note("Install it with `pacman -S cryptsetup` and start the installer")
                .note("again, or continue without encryption.")
                .help("enter back to the question · ctrl-c quit");
            ui.pause(&missing)?;
            continue;
        }
        match ui.secret(&page, "passphrase", false) {
            Ok(p) => {
                a.passphrase = p;
                return Ok(());
            }
            Err(Nav::Back) => continue,
            Err(Nav::Quit) => return Err(Nav::Quit),
        }
    }
}

fn hostname_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let page = Page::new(step, "What is this machine called?")
        .note("Set with `systemd-firstboot`, not written into the image: changing a")
        .note("hostname does not need a new image, so it is not Kiln's to hold.");
    a.hostname = ui.text(&page, "hostname", &a.hostname, |s| {
        if s.is_empty() || s.len() > 63 {
            Err("one to sixty-three characters".into())
        } else if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            Err("letters, digits and hyphens only".into())
        } else if s.starts_with('-') || s.ends_with('-') {
            Err("cannot begin or end with a hyphen".into())
        } else {
            Ok(())
        }
    })?;
    Ok(())
}

fn pick(
    ui: &mut Ui,
    step: (usize, usize),
    title: &str,
    note: &str,
    from: &[String],
    into: &mut String,
) -> Screen {
    let page = Page::new(step, title)
        .note(note)
        .note("Start typing to filter.");
    let options: Vec<Opt> = from
        .iter()
        .map(|s| {
            let opt = Opt::new(s.clone(), "");
            if *s == *into {
                opt.checked()
            } else {
                opt
            }
        })
        .collect();
    let i = ui.select(&page, &options)?;
    *into = from[i].clone();
    Ok(())
}

fn profile_screen(
    ui: &mut Ui,
    step: (usize, usize),
    profiles: &[&'static catalog::Profile],
    a: &mut Answers,
) -> Screen {
    let page = Page::new(step, "What kind of system is this?")
        .note("A profile is one line of `include` and the packages nobody has an")
        .note("opinion about. It also picks the kernel — two kernel modules are a")
        .note("conflict by design, so this is where that choice is made.");
    let options: Vec<Opt> = profiles
        .iter()
        .map(|p| {
            let opt = Opt::new(p.label, p.note);
            if p.module == a.profile {
                opt.checked()
            } else {
                opt
            }
        })
        .collect();
    let i = ui.select(&page, &options)?;
    a.profile = profiles[i].module.into();
    Ok(())
}

fn modules_screen(
    ui: &mut Ui,
    step: (usize, usize),
    extras: &[&'static Entry],
    a: &mut Answers,
) -> Screen {
    let page = Page::new(step, "Anything else from the module library?")
        .note("Each is one line of `include` in the configuration you end up with,")
        .note("and you can add or remove any of them later with `kiln apply`.")
        .note("Everything your profile already covers is left out of this list.");
    let options: Vec<Opt> = extras
        .iter()
        .map(|e| match e {
            Entry::Group(g) => Opt::heading(*g),
            Entry::Module(m) => {
                let opt = Opt::new(m.label, m.note);
                if a.modules.iter().any(|chosen| chosen == m.module) {
                    opt.checked()
                } else {
                    opt
                }
            }
        })
        .collect();
    let chosen = ui.multiselect(&page, &options)?;
    a.modules = chosen
        .iter()
        .filter_map(|&i| match extras[i] {
            Entry::Module(m) => Some(m.module.to_string()),
            Entry::Group(_) => None,
        })
        .collect();
    Ok(())
}

fn packages_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let page = Page::new(step, "Any packages to add?")
        .note("Space-separated Arch package names — `neovim git firefox`. Leave it")
        .note("empty if you would rather add them later; `kiln apply` is the same")
        .note("command either way.");
    // The character set is closed, and `config.rs` depends on it being closed:
    // package names are interpolated straight into TOML string literals with no
    // escaping, which is only safe while nothing here can produce a quote or a
    // backslash.
    let typed = ui.text(&page, "packages", &a.packages.join(" "), |s| {
        match s.split_whitespace().find(|p| {
            !p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
        }) {
            Some(bad) => Err(format!("`{bad}` is not a package name")),
            None => Ok(()),
        }
    })?;
    a.packages = typed.split_whitespace().map(str::to_string).collect();
    a.packages.sort();
    a.packages.dedup();
    Ok(())
}

fn username_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let page = Page::new(step, "Who is the first user?")
        .note("Created with `useradd` in the deployment, and put in `wheel`. Kiln")
        .note("itself has no idea this account exists — accounts live in /etc and")
        .note("are not image content, deliberately and permanently.");
    a.username = ui.text(&page, "username", &a.username, |s| {
        if s.is_empty() || s.len() > 32 {
            Err("one to thirty-two characters".into())
        } else if s == "root" {
            Err("root already exists".into())
        } else if !s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') {
            Err("must start with a lowercase letter or an underscore".into())
        } else if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c))
        {
            Err("lowercase letters, digits, underscore and hyphen only".into())
        } else {
            Ok(())
        }
    })?;
    Ok(())
}

fn password_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let page = Page::new(step, format!("A password for {}", a.username))
        .note("Typed twice. Nothing is echoed, and it never reaches the image or")
        .note("the log — it is piped to `chpasswd` inside the deployment.");
    a.user_password = ui.secret(&page, "password", false)?;
    Ok(())
}

fn root_password_screen(ui: &mut Ui, step: (usize, usize), a: &mut Answers) -> Screen {
    let sudo = a.sudo();
    let mut page = Page::new(step, "A password for root");
    if sudo {
        page = page
            .note("Optional. Your configuration grants `wheel` sudo, so you can leave")
            .note("this empty and the root account stays locked — which is what most")
            .note("Arch installs do now.");
    } else {
        page = page
            .note("Required. The profile you picked does not grant `wheel` sudo, so")
            .note("root is the only way to administer this machine. Add")
            .note("`@kiln/security/wheel-sudo` later if you would rather use sudo.");
    }
    a.root_password = ui.secret(&page, "root password", sudo)?;
    Ok(())
}

fn review_screen(ui: &mut Ui, step: (usize, usize), a: &Answers) -> Screen {
    let page = Page::new(step, "Ready")
        .note("Everything up to here was a question. Past this screen the disk is")
        .note("erased and a real image is built, which takes a while.");
    let modules = if a.modules.is_empty() {
        format!("{DIM}none{RESET}")
    } else {
        a.modules
            .iter()
            .map(|m| short(m))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let packages = if a.packages.is_empty() {
        format!("{DIM}none{RESET}")
    } else {
        a.packages.join(" ")
    };
    let rows = vec![
        (
            "disk".into(),
            format!(
                "{BOLD}{DANGER}{}{RESET}  {} — erased",
                a.disk.path,
                a.disk.size()
            ),
        ),
        (
            "encryption".into(),
            if a.encrypt {
                format!("{OK}LUKS2, root only{RESET}")
            } else {
                format!("{DIM}none{RESET}")
            },
        ),
        ("hostname".into(), a.hostname.clone()),
        ("timezone".into(), a.timezone.clone()),
        (
            "locale".into(),
            format!("{}  ·  keymap {}", a.locale, a.keymap),
        ),
        ("profile".into(), short(&a.profile).to_string()),
        ("modules".into(), modules),
        ("packages".into(), packages),
        ("user".into(), format!("{} (wheel)", a.username)),
        (
            "root".into(),
            if a.root_password.is_empty() {
                format!("{DIM}locked — administer with sudo{RESET}")
            } else {
                "password set".into()
            },
        ),
    ];
    ui.review(&page, &rows)
}

/// `@kiln/gpu/amd` → `gpu/amd`, for a screen with a column to spare rather than
/// a namespace to prove. Not `trim_start_matches`, which strips the prefix
/// repeatedly.
fn short(reference: &str) -> &str {
    reference.strip_prefix("@kiln/").unwrap_or(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{module_file, probe, PROFILES};

    /// Every `@kiln/...` reference a module file names, following `include`
    /// through the whole graph. A superset of the include list — any other
    /// array holding a module reference is counted too — which is the safe
    /// direction for the check below.
    fn reaches(root: &Path, reference: &str, seen: &mut Vec<String>) -> bool {
        if reference == WHEEL_SUDO {
            return true;
        }
        if seen.iter().any(|s| s == reference) {
            return false;
        }
        seen.push(reference.to_string());
        let Ok(text) = std::fs::read_to_string(module_file(root, reference)) else {
            return false;
        };
        // Odd segments of a split on `"` are the quoted strings; comments
        // mentioning a module in prose are not among them.
        let refs: Vec<String> = text
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| s.starts_with("@kiln/"))
            .map(str::to_string)
            .collect();
        refs.iter().any(|r| reaches(root, r, seen))
    }

    /// `grants_sudo` is a copy of what Kiln's library says, in a repository
    /// that releases separately from it. If a profile ever drops `wheel-sudo`,
    /// this is what stops the root-password screen from calling itself
    /// "Optional" on a machine that would then have neither root nor sudo.
    #[test]
    fn sudo_matches_what_the_profiles_actually_include() {
        let Some(root) = probe::module_root() else {
            eprintln!("skipping: no Kiln module library to read");
            return;
        };
        for p in PROFILES {
            if !module_file(&root, p.module).is_file() {
                continue;
            }
            let real = reaches(&root, p.module, &mut Vec::new());
            assert_eq!(
                grants_sudo(p.module, &[]),
                real,
                "{} {} `{WHEEL_SUDO}`, and the installer thinks otherwise",
                p.module,
                if real { "includes" } else { "does not include" }
            );
        }
    }

    /// The other half: a `minimal` install that picks the module by hand.
    #[test]
    fn choosing_wheel_sudo_unlocks_the_optional_root_password() {
        let minimal = "@kiln/profiles/minimal";
        assert!(!grants_sudo(minimal, &[]));
        assert!(grants_sudo(minimal, &[WHEEL_SUDO.to_string()]));
    }
}
