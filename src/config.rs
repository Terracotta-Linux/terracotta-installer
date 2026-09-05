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
//! | locale generation | a `[[script]]` | the archive lives in `/usr/lib/locale`; changing it needs a rebuild |
//! | `LANG`, `KEYMAP` | `[[file]]` | to write a config file, write a file |
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

    s.push_str(&format!(
        "[kernel]\ncmdline = [\"root=UUID={}\"]\n\n",
        a.root_uuid
    ));

    if a.packages.is_empty() {
        s.push_str("[packages]\nrepo = []\n\n");
    } else {
        s.push_str("[packages]\nrepo = [\n");
        for p in &a.packages {
            s.push_str(&format!("  \"{p}\",\n"));
        }
        s.push_str("]\n\n");
    }

    s.push_str(&format!(
        "[[file]]\n\
         target  = \"/etc/locale.conf\"\n\
         content = \"LANG={}\\n\"\n\n\
         [[file]]\n\
         target  = \"/etc/vconsole.conf\"\n\
         content = \"KEYMAP={}\\n\"\n",
        a.locale, a.keymap
    ));

    if let Some(script) = locale_script(&a.locale) {
        s.push('\n');
        s.push_str(&script);
    }

    s
}

/// The one build script the installer writes.
///
/// glibc ships a locale *archive* containing `C.UTF-8` and nothing else; every
/// other locale has to be compiled, and the result lands in `/usr/lib/locale`,
/// which is image content by any reading of the test. Kiln's own guide uses
/// `locale-gen` as its worked example for exactly this reason, including the
/// part where a script rewriting glibc's own `locale.gen` is *reported*
/// rather than refused.
///
/// A multi-line **literal** string (`'''`), not a basic one: `printf '%s\n'`
/// inside `"""` would have its `\n` eaten by TOML before the shell ever saw it.
fn locale_script(locale: &str) -> Option<String> {
    if locale == "C.UTF-8" || locale == "C" {
        return None;
    }
    let charmap = locale.rsplit('.').next().unwrap_or("UTF-8");
    Some(format!(
        "[[script]]\n\
         name    = \"20-locale\"\n\
         after   = \"packages\"\n\
         content = '''\n\
         printf '%s %s\\n' '{locale}' '{charmap}' > /etc/locale.gen\n\
         locale-gen\n\
         '''\n"
    ))
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
            root_uuid: "1234-abcd".into(),
        }
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
        let (Some(kiln), Some(modules)) = (
            crate::catalog::probe::binary(),
            crate::catalog::probe::module_root(),
        ) else {
            eprintln!("skipping: no `kiln` binary and module library to check against");
            return;
        };

        let dir = std::env::temp_dir().join(format!("terracotta-installer-{}", std::process::id()));
        let etc = dir.join("etc/kiln");
        std::fs::create_dir_all(&etc).expect("a config directory");
        std::fs::write(etc.join("system.toml"), system_toml(&answers())).expect("system.toml");

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
            system_toml(&answers()),
        );
        let shown = String::from_utf8_lossy(&out.stdout);
        for expected in ["root=UUID=1234-abcd", "/etc/locale.conf", "20-locale"] {
            assert!(
                shown.contains(expected),
                "`{expected}` missing from:\n{shown}"
            );
        }
    }

    #[test]
    fn no_packages_is_a_bare_empty_list() {
        let mut bare = answers();
        bare.packages.clear();
        let toml = system_toml(&bare);
        assert!(toml.contains("repo = []"), "{toml}");
    }

    #[test]
    fn c_utf8_needs_no_script() {
        assert!(locale_script("C.UTF-8").is_none());
        let de = locale_script("de_DE.UTF-8").expect("a script");
        assert!(de.contains("locale-gen"));
        // A literal string, so `\n` survives TOML and reaches printf.
        assert!(de.contains("'''"), "{de}");
    }
}
