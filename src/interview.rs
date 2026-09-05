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
use crate::tui::{Nav, Opt, Page, Ui, ACCENT, BOLD, DANGER, DIM, RESET, WARN};
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
    /// Not asked — filled in by `steps.rs` once `mkfs.ext4` has made one, and
    /// read back out of `blkid`. It is in `Answers` because it is an input to
    /// `config.rs`, and `config.rs` should have exactly one argument.
    pub root_uuid: String,
}

impl Answers {
    /// Whether an unlocked root account is optional. Kiln does not manage
    /// accounts, so this is the installer reasoning about the configuration it
    /// is about to write — not Kiln reasoning about users.
    fn sudo(&self) -> bool {
        self.profile != "@kiln/profiles/minimal"
            || self
                .modules
                .iter()
                .any(|m| m == "@kiln/security/wheel-sudo")
    }
}

const SCREENS: usize = 13;

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
        modules: Vec::new(),
        packages: Vec::new(),
        username: String::new(),
        user_password: String::new(),
        root_password: String::new(),
        root_uuid: String::new(),
    };

    let mut at = 0usize;
    loop {
        let step = (at + 1, SCREENS);
        let outcome = match at {
            0 => disk_screen(ui, step, &disks, &mut a),
            1 => confirm_screen(ui, step, &a),
            2 => hostname_screen(ui, step, &mut a),
            3 => pick(
                ui,
                step,
                "Timezone",
                "Used for the clock, not for the image.",
                &zones,
                &mut a.timezone,
            ),
            4 => pick(
                ui,
                step,
                "Locale",
                "Compiled into the image by a build script.",
                &locales,
                &mut a.locale,
            ),
            5 => pick(
                ui,
                step,
                "Console keymap",
                "The virtual console; a desktop sets its own.",
                &maps,
                &mut a.keymap,
            ),
            6 => profile_screen(ui, step, &profiles, &mut a),
            7 => modules_screen(ui, step, &extras, &mut a),
            8 => packages_screen(ui, step, &mut a),
            9 => username_screen(ui, step, &mut a),
            10 => password_screen(ui, step, &mut a),
            11 => root_password_screen(ui, step, &mut a),
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
    let page = Page::new(step, "Which disk should Kiln be installed on?")
        .note("Everything on the disk you choose is erased. Disks holding a mounted")
        .note("filesystem — the medium you booted from, most likely — cannot be chosen.");
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
            if d.busy.is_some() {
                opt.disabled()
            } else {
                opt
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
    let page = page
        .note(String::new())
        .note(format!(
            "{DANGER}The partition table and every filesystem on this disk are destroyed.{RESET}"
        ))
        .note("There is no undo, and nothing else on the machine is touched.".to_string())
        .note(String::new())
        .note(format!("Kiln will lay it out as {DIM}follows{RESET}:"))
        .note(format!(
            "  {ACCENT}1{RESET}  1G   EFI system partition   → /boot/efi"
        ))
        .note(format!(
            "  {ACCENT}2{RESET}  1G   ext4                   → /boot"
        ))
        .note(format!(
            "  {ACCENT}3{RESET}  rest ext4                   → /"
        ));
    ui.typed_confirm(&page, &d.word())
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
    let options: Vec<Opt> = from.iter().map(|s| Opt::new(s.clone(), "")).collect();
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
    let options: Vec<Opt> = profiles.iter().map(|p| Opt::new(p.label, p.note)).collect();
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
                if m.default {
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
        .note("Typed twice. Nothing is echoed, and it never reaches the image or".to_string())
        .note("the log — it is piped to `chpasswd` inside the deployment.".to_string());
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
            .map(|m| m.trim_start_matches("@kiln/"))
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
        ("hostname".into(), a.hostname.clone()),
        ("timezone".into(), a.timezone.clone()),
        (
            "locale".into(),
            format!("{}  ·  keymap {}", a.locale, a.keymap),
        ),
        (
            "profile".into(),
            a.profile.trim_start_matches("@kiln/").to_string(),
        ),
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
    ui.review(&page, &rows)?;
    Ok(())
}
