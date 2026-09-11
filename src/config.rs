//! Turning the interview into `/etc/kiln/system.toml`.
//!
//! The line this file draws is the one from `README.md`: **if it changes, do
//! you need a new image and a reboot?** Everything on the yes side is written
//! here and belongs to Kiln from then on; everything on the no side is done
//! once, in the deployment's `/etc`, by `steps.rs`.
//!
//! | answer | where it goes | why |
//! |---|---|---|
//! | profile, modules, extra packages | `include`, `packages.repo` | image content, plainly |
//! | the disk's `root=` | `kernel.cmdline` | kargs are fully declarative, so a karg not written down is one the next `kiln apply` removes |
//! | LUKS unlock (`rd.luks.*`, `dracut_modules = ["crypt"]`, `cryptsetup`) | `kernel.cmdline`, `kernel.dracut_modules`, `packages.repo` | the initramfs unlocks root before anything else runs, so this is exactly as declarative as `root=` itself — see `Answers::luks_uuid` |
//! | `KEYMAP`, `LANG`, locale generation | `[system]` | Kiln's own `[system]` table now materializes all three; writing them as a `[[file]]` plus a `locale-gen` `[[script]]` is refused outright as of the version that added it — `[system]` owns those targets |
//!
//! The generated file is meant to be **read and then edited**. It is the user's
//! configuration from the moment the installer exits, so it is commented the
//! way a configuration somebody has to maintain should be, and it is short.

use crate::interview::Answers;

/// `/etc/kiln/system.toml`, complete.
pub fn system_toml(a: &Answers) -> String {
    let mut s = String::new();

    s.push_str("# Written by terracotta-installer.\nkiln = 1\n\n");

    // `include` must come before any table header: a bare key written after
    // `[packages]` silently becomes `packages.include`, and Kiln has a
    // dedicated diagnostic for it that nobody should have to see on day one.
    s.push_str("include = [\n");
    s.push_str(&format!("  \"{}\",\n", a.profile));
    for m in &a.modules {
        s.push_str(&format!("  \"{m}\",\n"));
    }
    s.push_str("]\n\n");

    // The initramfs unlocks root before anything else runs — dracut's crypt
    // module reads `rd.luks.uuid`/`rd.luks.name` off the cmdline, the same
    // place `root=` already lives, which is why this needs no `[[file]]` for
    // `/etc/crypttab`: the root device is open by the time systemd, and
    // crypttab, would otherwise get a say.
    match &a.luks_uuid {
        Some(uuid) => s.push_str(&format!(
            "[kernel]\ncmdline = [\"rd.luks.uuid={uuid}\", \"rd.luks.name={uuid}=root\", \
             \"root=/dev/mapper/root\"]\ndracut_modules = [\"crypt\"]\n\n"
        )),
        None => s.push_str(&format!(
            "[kernel]\ncmdline = [\"root=UUID={}\"]\n\n",
            a.root_uuid
        )),
    }

    // `cryptsetup` ships dracut's crypt module — without it in the image,
    // `--add crypt` finds nothing and the build fails during dracut's own
    // verification, several minutes into a build the interview already
    // promised would produce a bootable disk.
    let mut packages = a.packages.clone();
    if a.luks_uuid.is_some() {
        packages.push("cryptsetup".to_string());
    }
    packages.sort();
    packages.dedup();
    if packages.is_empty() {
        s.push_str("[packages]\nrepo = []\n\n");
    } else {
        s.push_str("[packages]\nrepo = [\n");
        for p in &packages {
            s.push_str(&format!("  \"{p}\",\n"));
        }
        s.push_str("]\n\n");
    }

    s.push_str(&format!("[system]\nkeymap = \"{}\"\n", a.keymap));
    s.push_str(&locale_field(&a.locale));

    s
}

/// The `system.locale` value: `lang` always, plus `generate` for anything
/// beyond the one locale glibc's stock archive already carries.
///
/// glibc ships a locale *archive* containing `C.UTF-8` and nothing else; every
/// other locale has to be compiled, and the result lands in `/usr/lib/locale`,
/// which is image content by any reading of the test. Kiln now runs
/// `locale-gen` itself during assembly whenever `system.locale.generate` is
/// non-empty, so naming the locale here is the whole of what used to be a
/// hand-written build script.
///
/// `generate` wants `locale.gen`'s own two-column form (`"en_US.UTF-8
/// UTF-8"`), and the second column is the same suffix already on the first —
/// `/usr/share/i18n/SUPPORTED`, which `catalog::locales` reads its names from,
/// carries it as exactly that suffix for every UTF-8 locale.
fn locale_field(locale: &str) -> String {
    if locale == "C.UTF-8" || locale == "C" {
        return format!("locale = {{ lang = \"{locale}\" }}\n");
    }
    let charmap = locale.rsplit('.').next().unwrap_or("UTF-8");
    format!("locale = {{ lang = \"{locale}\", generate = [\"{locale} {charmap}\"] }}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Disk;

    fn answers() -> Answers {
        Answers {
            disk: Disk {
                path: "/dev/sda".into(),
                bytes: 500 << 30,
                model: "test".into(),
                transport: "sata".into(),
                removable: false,
                busy: None,
            },
            hostname: "kiln".into(),
            timezone: "Europe/Berlin".into(),
            locale: "en_US.UTF-8".into(),
            keymap: "us".into(),
            profile: "@kiln/profiles/workstation".into(),
            modules: vec!["@kiln/gpu/amd".into()],
            packages: vec!["neovim".into()],
            username: "ada".into(),
            user_password: "secret".into(),
            root_password: String::new(),
            encrypt: false,
            passphrase: String::new(),
            root_uuid: "1234-abcd".into(),
            luks_uuid: None,
        }
    }

    fn encrypted_answers() -> Answers {
        let mut a = answers();
        a.encrypt = true;
        a.passphrase = "correct horse battery staple".into();
        a.luks_uuid = Some("deadbeef-dead-beef-dead-beefdeadbeef".into());
        a
    }

    #[test]
    fn include_precedes_every_table_header() {
        let toml = system_toml(&answers());
        let include = toml.find("include").expect("an include list");
        let first_table = toml.find("\n[").expect("at least one table");
        assert!(
            include < first_table,
            "`include` after a table header becomes `packages.include`"
        );
    }

    #[test]
    fn the_root_karg_is_written_down() {
        let toml = system_toml(&answers());
        assert!(toml.contains("root=UUID=1234-abcd"), "{toml}");
    }

    #[test]
    fn no_account_reaches_the_configuration() {
        let toml = system_toml(&answers());
        for forbidden in ["ada", "secret", "[[user]]", "password"] {
            assert!(
                !toml.contains(forbidden),
                "`{forbidden}` is in the generated config; accounts are not \
                 image content\n{toml}"
            );
        }
    }

    /// An encrypted root writes `rd.luks.*` and `root=/dev/mapper/root`
    /// instead of `root=UUID=…`, and never the plain filesystem UUID: a
    /// generation deployed with the wrong one of the two boots straight to
    /// an emergency shell, since `ostree-prepare-root` gets a device that is
    /// still a locked LUKS container.
    #[test]
    fn an_encrypted_root_gets_luks_kargs_instead_of_a_uuid() {
        let toml = system_toml(&encrypted_answers());
        assert!(
            toml.contains("rd.luks.uuid=deadbeef-dead-beef-dead-beefdeadbeef"),
            "{toml}"
        );
        assert!(
            toml.contains("rd.luks.name=deadbeef-dead-beef-dead-beefdeadbeef=root"),
            "{toml}"
        );
        assert!(toml.contains("root=/dev/mapper/root"), "{toml}");
        assert!(toml.contains("dracut_modules = [\"crypt\"]"), "{toml}");
        assert!(!toml.contains("root=UUID=1234-abcd"), "{toml}");
    }

    #[test]
    fn an_encrypted_image_gets_cryptsetup() {
        let toml = system_toml(&encrypted_answers());
        assert!(toml.contains("\"cryptsetup\""), "{toml}");
        // …and an unencrypted one does not carry a package it has no use for.
        assert!(!system_toml(&answers()).contains("cryptsetup"));
    }

    #[test]
    fn no_passphrase_reaches_the_configuration() {
        let toml = system_toml(&encrypted_answers());
        assert!(
            !toml.contains("correct horse battery staple"),
            "the passphrase is in the generated config; it is piped to \
             `cryptsetup` and nowhere else\n{toml}"
        );
    }

    /// The one test that proves the generated file is a *Kiln* configuration
    /// rather than merely valid TOML.
    ///
    /// `kiln show` runs the whole frontend — discovery, parse, the include
    /// graph, merge, validate, `Manifest` — and needs no network, so it is the
    /// cheapest way to find out that a key was renamed, that `include` ended up
    /// after a table header, or that a module reference no longer resolves.
    /// This program and Kiln are separate repositories with separate release
    /// cycles, which is exactly the arrangement in which that can happen
    /// without anybody noticing.
    #[test]
    fn kiln_accepts_what_the_installer_writes() {
        let Some(shown) = kiln_show(&system_toml(&answers())) else {
            return;
        };
        for expected in ["root=UUID=1234-abcd", "keymap=us", "lang=en_US.UTF-8"] {
            assert!(
                shown.contains(expected),
                "`{expected}` missing from:\n{shown}"
            );
        }
    }

    /// The same check, over the branch `kiln_accepts_what_the_installer_writes`
    /// does not reach: `rd.luks.*`, `dracut_modules`, and `cryptsetup` are a
    /// second way to write a table header or a key name wrong, and the plain
    /// config passing `kiln show` proves nothing about this one.
    #[test]
    fn kiln_accepts_the_encrypted_configuration() {
        let Some(shown) = kiln_show(&system_toml(&encrypted_answers())) else {
            return;
        };
        for expected in [
            "rd.luks.uuid=deadbeef-dead-beef-dead-beefdeadbeef",
            "root=/dev/mapper/root",
            "cryptsetup",
        ] {
            assert!(
                shown.contains(expected),
                "`{expected}` missing from:\n{shown}"
            );
        }
    }

    /// Runs `kiln show` over generated TOML and returns its stdout, or `None`
    /// when there is no real Kiln to check against — see the module doc on
    /// why this program keeps a copy of Kiln's schema instead of depending on
    /// it, and why that copy is checked here rather than trusted.
    fn kiln_show(toml: &str) -> Option<String> {
        let (Some(kiln), Some(modules)) = (
            crate::catalog::probe::binary(),
            crate::catalog::probe::module_root(),
        ) else {
            eprintln!("skipping: no `kiln` binary and module library to check against");
            return None;
        };

        // Thread-unique, not just process-unique: cargo runs this module's
        // tests on separate threads of the same process, and two of them
        // sharing a directory is a race between one's `write` and the
        // other's `remove_dir_all`.
        let dir = std::env::temp_dir().join(format!(
            "terracotta-installer-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let etc = dir.join("etc/kiln");
        std::fs::create_dir_all(&etc).expect("a config directory");
        std::fs::write(etc.join("system.toml"), toml).expect("system.toml");

        let out = std::process::Command::new(&kiln)
            .args(["--config".as_ref(), etc.as_os_str()])
            .args(["--module-root".as_ref(), modules.as_os_str()])
            .arg("show")
            .output()
            .expect("running kiln");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            out.status.success(),
            "`kiln show` rejected the generated configuration:\n{}\n{}",
            String::from_utf8_lossy(&out.stderr),
            toml,
        );
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[test]
    fn no_packages_is_a_bare_empty_list() {
        let mut bare = answers();
        bare.packages.clear();
        let toml = system_toml(&bare);
        assert!(toml.contains("repo = []"), "{toml}");
    }

    #[test]
    fn c_utf8_needs_no_generate() {
        assert_eq!(locale_field("C.UTF-8"), "locale = { lang = \"C.UTF-8\" }\n");
        let de = locale_field("de_DE.UTF-8");
        assert!(de.contains("generate = [\"de_DE.UTF-8 UTF-8\"]"), "{de}");
    }

    #[test]
    fn the_system_table_replaces_the_old_file_and_script() {
        let toml = system_toml(&answers());
        assert!(toml.contains("[system]"), "{toml}");
        assert!(toml.contains("keymap = \"us\""), "{toml}");
        assert!(
            toml.contains("generate = [\"en_US.UTF-8 UTF-8\"]"),
            "{toml}"
        );
        for gone in ["[[file]]", "[[script]]", "locale-gen", "/etc/locale.conf"] {
            assert!(
                !toml.contains(gone),
                "`{gone}` should no longer appear:\n{toml}"
            );
        }
    }
}
