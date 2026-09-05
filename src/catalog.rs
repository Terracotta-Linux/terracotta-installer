//! The choices the interview offers.
//!
//! The module names are a **copy** of Kiln's module library, not a read of it.
//! That is deliberate: the notes beside each entry are written for somebody
//! installing a system for the first time, and no comment inside a module
//! file is going to be the right sentence for that. Kiln's own test suite is
//! what keeps the library honest; what keeps *this* honest is the check in
//! `steps.rs`, which refuses to write an include for a module the host's
//! library does not actually have.
//!
//! Locales, timezones and keymaps are read from the running system instead,
//! because those lists are glibc's and systemd's and copying them would mean
//! shipping a snapshot that goes stale.

use crate::run::Runner;
use std::path::Path;

pub struct Profile {
    pub module: &'static str,
    pub label: &'static str,
    pub note: &'static str,
}

/// Profiles are the only modules that compose, and every one of them
/// already picks a kernel — which is why the interview does not ask about
/// kernels separately. Swapping one is a five-line edit to the generated
/// configuration, and the guide says so.
pub const PROFILES: &[Profile] = &[
    Profile {
        module: "@kiln/profiles/workstation",
        label: "workstation",
        note: "desktop machine — networking, audio, bluetooth, printing, sudo",
    },
    Profile {
        module: "@kiln/profiles/server",
        label: "server",
        note: "headless — linux-lts, sshd, nftables, nothing else",
    },
    Profile {
        module: "@kiln/profiles/minimal",
        label: "minimal",
        note: "the smallest image that boots; no network stack at all",
    },
];

pub struct Module {
    pub module: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    /// Pre-checked on the modules screen. Reserved for modules whose absence
    /// would leave the installed system unable to reproduce or update itself.
    pub default: bool,
}

pub enum Entry {
    Group(&'static str),
    Module(Module),
}

macro_rules! m {
    ($path:literal, $label:literal, $note:literal) => {
        Entry::Module(Module {
            module: $path,
            label: $label,
            note: $note,
            default: false,
        })
    };
}

/// Same as `m!`, but pre-checked on the modules screen.
macro_rules! d {
    ($path:literal, $label:literal, $note:literal) => {
        Entry::Module(Module {
            module: $path,
            label: $label,
            note: $note,
            default: true,
        })
    };
}

/// Everything worth adding on top of a profile, grouped the way Kiln's module
/// library groups it.
///
/// Absent on purpose: `@kiln/kernel/*` (a profile picked one, and two is a
/// conflict by design), `@kiln/boot/grub2` and `@kiln/hardware/firmware` (every
/// profile has them already).
pub const EXTRAS: &[Entry] = &[
    Entry::Group("terracotta"),
    m!(
        "@kiln/terracotta/installer",
        "installer",
        "keeps terracotta-installer available on the deployed system"
    ),
    d!(
        "@kiln/terracotta/kiln",
        "kiln",
        "the build tool itself, on the deployed system — highly recommended"
    ),
    d!(
        "@kiln/terracotta/branding",
        "branding",
        "Terracotta Linux branding — highly recommended"
    ),
    Entry::Group("desktop"),
    m!(
        "@kiln/desktop/gnome-minimal",
        "gnome (minimal)",
        "session, shell and GDM — no other apps"
    ),
    m!(
        "@kiln/desktop/gnome",
        "gnome",
        "gnome-minimal plus a normal app set"
    ),
    m!(
        "@kiln/desktop/gnome-full",
        "gnome (full)",
        "gnome plus the gnome-extra group"
    ),
    m!(
        "@kiln/desktop/plasma-minimal",
        "plasma (minimal)",
        "session, portal and SDDM — no other apps"
    ),
    m!(
        "@kiln/desktop/plasma",
        "plasma",
        "plasma-minimal plus a normal app set"
    ),
    m!(
        "@kiln/desktop/plasma-full",
        "plasma (full)",
        "plasma plus plasma-meta"
    ),
    m!(
        "@kiln/desktop/cosmic-minimal",
        "cosmic (minimal)",
        "session only"
    ),
    m!("@kiln/desktop/cosmic", "cosmic", "the full cosmic group"),
    m!(
        "@kiln/desktop/xfce-minimal",
        "xfce (minimal)",
        "session and LightDM — no other apps"
    ),
    m!(
        "@kiln/desktop/xfce",
        "xfce",
        "the full xfce4 and xfce4-goodies groups"
    ),
    Entry::Group("window managers"),
    m!("@kiln/wm/hyprland", "hyprland", "Wayland, dynamic tiling"),
    m!("@kiln/wm/sway", "sway", "Wayland, i3-compatible"),
    m!("@kiln/wm/niri", "niri", "Wayland, scrollable tiling"),
    m!("@kiln/wm/i3", "i3", "X11, manual tiling"),
    Entry::Group("graphics"),
    m!("@kiln/gpu/amd", "amd", "Mesa, RADV"),
    m!("@kiln/gpu/amd-rocm", "amd-rocm", "ROCm, on top of amd"),
    m!("@kiln/gpu/intel", "intel", "Mesa, ANV"),
    m!(
        "@kiln/gpu/nvidia-open",
        "nvidia-open",
        "built against `linux`"
    ),
    m!(
        "@kiln/gpu/nvidia-open-lts",
        "nvidia-open-lts",
        "built against `linux-lts` — pair with the server profile"
    ),
    m!(
        "@kiln/gpu/nvidia-cuda",
        "nvidia-cuda",
        "CUDA and cuDNN, on top of an nvidia-open driver"
    ),
    Entry::Group("firmware"),
    m!("@kiln/hardware/amd-ucode", "amd-ucode", "AMD microcode"),
    m!(
        "@kiln/hardware/intel-ucode",
        "intel-ucode",
        "Intel microcode"
    ),
    Entry::Group("hardware"),
    m!("@kiln/hardware/bluetooth", "bluetooth", "bluez"),
    m!(
        "@kiln/hardware/laptop",
        "laptop",
        "audio DSP firmware, power management"
    ),
    m!("@kiln/hardware/printing", "printing", "CUPS"),
    Entry::Group("network"),
    m!(
        "@kiln/net/networkmanager",
        "networkmanager",
        "the desktop answer"
    ),
    m!(
        "@kiln/net/systemd-networkd",
        "systemd-networkd",
        "the server answer"
    ),
    m!(
        "@kiln/net/iwd",
        "iwd",
        "wifi daemon — pair with systemd-networkd"
    ),
    m!("@kiln/net/sshd", "sshd", "OpenSSH, key auth only"),
    m!("@kiln/net/nftables", "nftables", "a default-deny firewall"),
    m!(
        "@kiln/net/tailscale",
        "tailscale",
        "enabled at boot; join the tailnet yourself"
    ),
    Entry::Group("audio"),
    m!(
        "@kiln/audio/pipewire",
        "pipewire",
        "PipeWire and WirePlumber"
    ),
    Entry::Group("development"),
    m!(
        "@kiln/dev/base-devel",
        "base-devel",
        "toolchain, needed to build AUR packages in-image"
    ),
    m!("@kiln/dev/rust", "rust", "rustup-free Rust toolchain"),
    m!("@kiln/dev/go", "go", "Go toolchain"),
    Entry::Group("virtualization"),
    m!("@kiln/virt/podman", "podman", "rootless containers"),
    m!("@kiln/virt/docker", "docker", "Docker"),
    m!("@kiln/virt/libvirt", "libvirt", "QEMU/KVM"),
    Entry::Group("security"),
    m!(
        "@kiln/security/wheel-sudo",
        "wheel-sudo",
        "members of `wheel` may sudo"
    ),
    m!("@kiln/security/apparmor", "apparmor", "AppArmor, enforcing"),
];

/// `@kiln/desktop/gnome` → `<module root>/desktop/gnome.toml`.
pub fn module_file(root: &Path, reference: &str) -> std::path::PathBuf {
    let rest = reference.trim_start_matches("@kiln/");
    root.join(format!("{rest}.toml"))
}

/// The profiles the host's library actually has.
///
/// The lists above are a copy of Kiln's module library, and a copy can go stale — a module
/// renamed in the library would otherwise become an `include` that fails
/// validation *after* the disk has been erased. Filtering here turns that into
/// an option that is quietly not offered, which is the failure mode you want
/// from a stale catalog.
pub fn profiles(root: &Path) -> Vec<&'static Profile> {
    PROFILES
        .iter()
        .filter(|p| module_file(root, p.module).is_file())
        .collect()
}

/// The same for the extras, dropping any group heading left with nothing under
/// it.
pub fn extras(root: &Path) -> Vec<&'static Entry> {
    let kept: Vec<&'static Entry> = EXTRAS
        .iter()
        .filter(|e| match e {
            Entry::Group(_) => true,
            Entry::Module(m) => module_file(root, m.module).is_file(),
        })
        .collect();
    kept.iter()
        .enumerate()
        .filter(|(i, e)| match e {
            Entry::Module(_) => true,
            Entry::Group(_) => matches!(kept.get(i + 1), Some(Entry::Module(_))),
        })
        .map(|(_, e)| *e)
        .collect()
}

/// Every UTF-8 locale glibc knows about, `en_US.UTF-8` first if it is there.
///
/// From `/usr/share/i18n/SUPPORTED`, which is glibc's own list and the file
/// `locale-gen` reads its charmaps out of. The fallback exists so a stripped
/// image still gets a working question rather than an empty screen.
pub fn locales() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_to_string("/usr/share/i18n/SUPPORTED")
        .map(|text| {
            text.lines()
                .filter_map(|l| l.split_whitespace().next())
                .filter(|l| l.ends_with(".UTF-8"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty() {
        out = ["en_US.UTF-8", "en_GB.UTF-8", "de_DE.UTF-8", "fr_FR.UTF-8"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    out.sort();
    out.dedup();
    prefer(&mut out, "en_US.UTF-8");
    out
}

/// `Area/City` for every zone in the tzdata tree.
pub fn timezones() -> Vec<String> {
    let root = Path::new("/usr/share/zoneinfo");
    let mut out = Vec::new();
    let Ok(areas) = std::fs::read_dir(root) else {
        return vec!["UTC".into()];
    };
    for area in areas.flatten() {
        let name = area.file_name().to_string_lossy().into_owned();
        // The tzdata tree also holds `posix/`, `right/`, `zone.tab` and the
        // legacy single-word zones; only the `Area/City` directories are the
        // names `timedatectl` and `systemd-firstboot` accept.
        if !area.path().is_dir() || matches!(name.as_str(), "posix" | "right" | "SystemV") {
            continue;
        }
        if !name.chars().next().is_some_and(char::is_uppercase) {
            continue;
        }
        for city in std::fs::read_dir(area.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let city_name = city.file_name().to_string_lossy().into_owned();
            if city.path().is_dir() {
                for sub in std::fs::read_dir(city.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    out.push(format!(
                        "{name}/{city_name}/{}",
                        sub.file_name().to_string_lossy()
                    ));
                }
            } else {
                out.push(format!("{name}/{city_name}"));
            }
        }
    }
    out.push("UTC".into());
    out.sort();
    out.dedup();
    prefer(&mut out, "UTC");
    out
}

/// Console keymaps, from systemd — which is the thing that will consume the
/// answer, so its idea of the list is the only one that matters.
pub fn keymaps(run: &mut Runner) -> Vec<String> {
    let mut out: Vec<String> = run
        .capture(&["localectl", "list-keymaps", "--no-pager"])
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty() {
        out = ["us", "uk", "de", "fr", "es", "it", "dvorak", "colemak"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    out.sort();
    out.dedup();
    prefer(&mut out, "us");
    out
}

/// Move one entry to the front, if it is present. The first row of a filtered
/// list is what most people press enter on, and it should be the answer that is
/// right most often rather than whatever sorts first.
fn prefer(list: &mut Vec<String>, first: &str) {
    if let Some(i) = list.iter().position(|x| x == first) {
        let it = list.remove(i);
        list.insert(0, it);
    }
}

/// Finding a real Kiln to test against.
///
/// `terracotta-installer` and `kiln` are separate repositories with separate release
/// cycles, which is the whole point of keeping installation out of Kiln — and
/// also exactly the arrangement in which a module can be renamed or a schema key
/// moved without anybody here noticing. So the tests that can check against a
/// real Kiln do, in whichever of the two situations this crate gets built in:
/// somebody pointing `KILN_MODULE_DIR` at a checkout, or a sibling `../kiln`
/// clone in the ordinary development layout.
#[cfg(test)]
pub(crate) mod probe {
    use std::path::PathBuf;

    fn sibling() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../kiln")
    }

    pub(crate) fn module_root() -> Option<PathBuf> {
        std::env::var_os("KILN_MODULE_DIR")
            .map(PathBuf::from)
            .into_iter()
            .chain([
                PathBuf::from(crate::DEFAULT_MODULE_DIR),
                sibling().join("modules"),
            ])
            .find(|d| d.join("profiles/minimal.toml").is_file())
    }

    pub(crate) fn binary() -> Option<PathBuf> {
        crate::preflight::which("kiln").or_else(|| {
            ["target/debug/kiln", "target/release/kiln"]
                .iter()
                .map(|p| sibling().join(p))
                .find(|p| p.is_file())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_references_map_to_files() {
        let p = module_file(
            Path::new("/usr/share/kiln/modules"),
            "@kiln/gpu/nvidia-open",
        );
        assert_eq!(p, Path::new("/usr/share/kiln/modules/gpu/nvidia-open.toml"));
    }

    #[test]
    fn every_offered_module_is_a_kiln_reference() {
        for e in EXTRAS {
            if let Entry::Module(m) = e {
                assert!(m.module.starts_with("@kiln/"), "{}", m.module);
            }
        }
        for p in PROFILES {
            assert!(p.module.starts_with("@kiln/profiles/"), "{}", p.module);
        }
    }

    /// The catalog is a copy of Kiln's module library, and a copy in another
    /// repository is a copy that goes stale. Every reference it offers must
    /// name a module that is actually there.
    #[test]
    fn every_reference_resolves_against_a_real_library() {
        let Some(root) = probe::module_root() else {
            eprintln!("skipping: no Kiln module library to read");
            return;
        };
        let mut missing: Vec<&str> = Vec::new();
        for p in PROFILES {
            if !module_file(&root, p.module).is_file() {
                missing.push(p.module);
            }
        }
        for e in EXTRAS {
            if let Entry::Module(m) = e {
                if !module_file(&root, m.module).is_file() {
                    missing.push(m.module);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "the catalog offers modules {} does not have: {missing:?}",
            root.display()
        );
    }

    #[test]
    fn an_empty_library_offers_nothing() {
        let none = Path::new("/nonexistent-module-root");
        assert!(profiles(none).is_empty());
        assert!(extras(none).is_empty());
    }

    /// A heading whose whole namespace filtered away would render as a label
    /// with no rows under it.
    #[test]
    fn headings_do_not_outlive_their_groups() {
        let Some(root) = probe::module_root() else {
            eprintln!("skipping: no Kiln module library to read");
            return;
        };
        let kept = extras(&root);
        for (i, e) in kept.iter().enumerate() {
            if matches!(e, Entry::Group(_)) {
                assert!(
                    matches!(kept.get(i + 1), Some(Entry::Module(_))),
                    "a group heading with nothing under it"
                );
            }
        }
    }

    #[test]
    fn prefer_moves_without_duplicating() {
        let mut v: Vec<String> = ["a", "b", "us"].iter().map(|s| s.to_string()).collect();
        prefer(&mut v, "us");
        assert_eq!(v, vec!["us", "a", "b"]);
        prefer(&mut v, "missing");
        assert_eq!(v, vec!["us", "a", "b"]);
    }
}
