//! The install itself: eleven steps, in a fixed order.
//!
//! The steps split into two columns — "the installer does" and "Kiln
//! provides" — and this module is the left one. Kiln's side is reached only
//! through the `kiln` binary and only through the flags it documents, because
//! the seam being narrow is what makes it a seam: if the installer linked
//! `kiln-ostree` it could deploy a generation without Kiln knowing, and then
//! the machine's build record and its bootloader would disagree.
//!
//! The order matters, and two parts of it are easy to get backwards:
//!
//! - `kiln build --sysroot` creates `ostree/repo` on its own, so building into
//!   an uninitialized target succeeds and *then* fails at deploy, several
//!   hundred megabytes later. `sysroot init` comes first, always.
//! - A `--sysroot` deploy writes **BLS entries and no `grub.cfg`**: libostree's
//!   grub2 backend runs `grub-mkconfig` chrooted into the deployment with a
//!   host-absolute output path, which cannot work from outside `/`. So the
//!   bootloader step is the installer's, and it runs *after* the deploy,
//!   because grub-mkconfig has to read the deployment's own `/etc/grub.d`. Its
//!   output does **not** stay at `/boot/grub/grub.cfg`: libostree maintains
//!   `/boot/loader.N/grub.cfg` and swaps the `loader` symlink, so a regular
//!   file where GRUB looks is a config frozen at one bootversion and a machine
//!   that boots exactly once more. See `bootloader()`.
//! - `/mnt/etc` is **not** the installed machine's `/etc`. It is tempting to
//!   think of the configuration as "just a file at `/mnt/etc/kiln/system.toml`",
//!   which is true of the file `kiln build --config` reads and false of the file
//!   the machine boots with: `/mnt` is the physical root, and from the first
//!   boot onward `/` is the deployment, so anything left in `/mnt/etc` reappears
//!   as `/sysroot/etc` — a path nothing reads. The config root is written there
//!   for the build and **moved into the deployment** afterwards, by `etc()`.

use crate::config;
use crate::interview::Answers;
use crate::run::{Done, Failed, Runner};
use crate::tui::{Progress, Ui};
use std::path::{Path, PathBuf};

/// Where the target is mounted. This is the path Kiln's own documentation
/// names, and an installer that used a different one would be documenting
/// itself out of the manual.
pub const MNT: &str = "/mnt";

/// libostree's name for Kiln's stateroot, `kiln_ostree::deploy::STATEROOT`.
/// Named again here because the installer does not link that crate; the check
/// in `deployment()` is what stops the two from drifting silently.
const STATEROOT: &str = "kiln";

/// The ESP and `/boot`. One gibibyte each: libostree keeps a kernel and an
/// initramfs per distinct boot checksum under `/boot/ostree/`, a dracut
/// initramfs with the `ostree` module is comfortably 80 MiB, and Kiln's default
/// keeps three generations plus the baseline. 512 MiB fits that until it
/// suddenly does not, at which point the failure is a deploy that half-writes
/// a boot entry.
const ESP_MIB: u32 = 1024;
const BOOT_MIB: u32 = 1024;

/// Where the configuration is written for `kiln build` to read: on the physical
/// root, because that is where it exists at the moment the build runs and the
/// deployment does not exist yet. It does not stay here — see `etc()`.
fn staged_config() -> PathBuf {
    PathBuf::from(MNT).join("etc/kiln")
}

pub const STEPS: &[&str] = &[
    "partition",
    "format",
    "mount",
    "sysroot",
    "configure",
    "build",
    "deploy",
    "etc",
    "bootloader",
    "accounts",
    "finish",
];

pub struct Installer<'a> {
    pub run: &'a mut Runner,
    pub module_root: PathBuf,
}

/// Everything an install needs that is not an answer: the paths it made.
struct Layout {
    esp: String,
    boot: String,
    root: String,
    esp_uuid: String,
    boot_uuid: String,
    root_uuid: String,
}

impl Installer<'_> {
    /// Run the whole thing, drawing as it goes. On failure the step list keeps
    /// the ✘ where it happened, which is most of the diagnosis.
    pub fn install(&mut self, ui: &mut Ui, a: &mut Answers) -> Result<(), Failed> {
        let mut p = Progress::new(STEPS);
        macro_rules! step {
            ($ui:expr, $p:expr, $i:expr, $detail:expr, $body:expr) => {{
                $p.at = $i;
                $p.detail = $detail.to_string();
                $ui.progress(&$p);
                let out = $body;
                if out.is_err() {
                    $p.failed = true;
                    $ui.progress(&$p);
                }
                out?
            }};
        }

        let layout = step!(ui, p, 0, a.disk.path.clone(), self.partition(ui, &mut p, a));
        let layout = step!(
            ui,
            p,
            1,
            "vfat, ext4, ext4",
            self.format(ui, &mut p, layout)
        );
        step!(ui, p, 2, MNT, self.mount(ui, &mut p, &layout));
        step!(ui, p, 3, "kiln sysroot init", self.sysroot(ui, &mut p));

        a.root_uuid = layout.root_uuid.clone();
        step!(
            ui,
            p,
            4,
            "etc/kiln/system.toml",
            self.configure(ui, &mut p, a)
        );

        step!(ui, p, 5, "this takes a while", self.build(ui, &mut p));
        step!(ui, p, 6, "generation 1", self.deploy(ui, &mut p));

        let dep = deployment(Path::new(MNT), self.run.dry_run)?;
        step!(ui, p, 7, "fstab, kiln", self.etc(ui, &mut p, &dep, &layout));
        step!(
            ui,
            p,
            8,
            "grub-install, grub-mkconfig",
            self.bootloader(ui, &mut p, &dep)
        );
        step!(
            ui,
            p,
            9,
            a.username.clone(),
            self.accounts(ui, &mut p, &dep, a)
        );
        step!(ui, p, 10, "unmounting", self.finish(ui, &mut p));

        p.at = STEPS.len();
        p.detail.clear();
        ui.progress(&p);
        Ok(())
    }

    /// GPT, three partitions, no swap.
    ///
    /// No swap because a swap partition is not something Kiln needs and not
    /// something an installer should decide for you: `zram-generator` or a swap
    /// *file* are both better defaults on a machine with modern amounts of RAM,
    /// and both are configurable after the fact without repartitioning. A
    /// partition is the one choice that cannot be undone later.
    ///
    /// Type `8300` for the root partition rather than `8304`: `8304` is the
    /// discoverable-partitions type for a root filesystem, which invites
    /// `systemd-gpt-auto-generator` to mount it — and on an OSTree system the
    /// thing that mounts the root is `ostree-prepare-root` from the initramfs,
    /// working from the `root=` karg, which Kiln's fully-declarative kargs
    /// insist on writing down.
    fn partition(&mut self, ui: &mut Ui, p: &mut Progress, a: &Answers) -> Result<Layout, Failed> {
        let disk = a.disk.path.clone();
        let mut say = say(ui, p);
        self.run
            .run("wiping signatures", &["wipefs", "--all", &disk], &mut say)?;
        self.run.run(
            "zapping the partition table",
            &["sgdisk", "--zap-all", &disk],
            &mut say,
        )?;
        self.run.run(
            "creating partitions",
            &[
                "sgdisk",
                &format!("--new=1:0:+{ESP_MIB}M"),
                "--typecode=1:ef00",
                "--change-name=1:EFI system partition",
                &format!("--new=2:0:+{BOOT_MIB}M"),
                "--typecode=2:8300",
                "--change-name=2:kiln-boot",
                "--new=3:0:0",
                "--typecode=3:8300",
                "--change-name=3:kiln-root",
                &disk,
            ],
            &mut say,
        )?;
        // Between `sgdisk` returning and the kernel publishing the new device
        // nodes there is a window in which `mkfs` gets ENOENT. `partprobe` asks
        // for the reread and `udevadm settle` waits for udev to have finished
        // acting on it; skipping either is a race that fails on fast disks.
        self.run.run(
            "rereading the partition table",
            &["partprobe", &disk],
            &mut say,
        )?;
        self.run
            .run("waiting for udev", &["udevadm", "settle"], &mut say)?;
        Ok(Layout {
            esp: a.disk.partition(1),
            boot: a.disk.partition(2),
            root: a.disk.partition(3),
            esp_uuid: String::new(),
            boot_uuid: String::new(),
            root_uuid: String::new(),
        })
    }

    /// vfat for the ESP because UEFI reads nothing else; ext4 for `/boot`
    /// because libostree keeps `/boot/loader` as a symlink pair and vfat has no
    /// symlinks. That pair of sentences is the whole reason there are two boot
    /// partitions instead of one.
    fn format(&mut self, ui: &mut Ui, p: &mut Progress, mut l: Layout) -> Result<Layout, Failed> {
        {
            let mut say = say(ui, p);
            self.run.run(
                "mkfs.fat",
                &["mkfs.fat", "-F", "32", "-n", "KILN-ESP", &l.esp],
                &mut say,
            )?;
            self.run.run(
                "mkfs.ext4 /boot",
                &["mkfs.ext4", "-F", "-L", "kiln-boot", &l.boot],
                &mut say,
            )?;
            self.run.run(
                "mkfs.ext4 /",
                &["mkfs.ext4", "-F", "-L", "kiln-root", &l.root],
                &mut say,
            )?;
            self.run
                .run("waiting for udev", &["udevadm", "settle"], &mut say)?;
        }
        l.esp_uuid = self.uuid(&l.esp)?;
        l.boot_uuid = self.uuid(&l.boot)?;
        l.root_uuid = self.uuid(&l.root)?;
        Ok(l)
    }

    /// The filesystem UUID, which is what goes in `root=` and in `fstab`.
    ///
    /// Not the partition's `PARTUUID` and not `/dev/sda3`: a kernel name is not
    /// stable across a disk being moved to another port, and the karg is written
    /// into a configuration that is meant to outlive this afternoon.
    fn uuid(&mut self, device: &str) -> Result<String, Failed> {
        if self.run.dry_run {
            return Ok(format!("dry-run-uuid-for-{}", device.replace('/', "_")));
        }
        let out = self
            .run
            .capture(&["blkid", "-s", "UUID", "-o", "value", device])?;
        let uuid = out.trim().to_string();
        if uuid.is_empty() {
            return Err(Failed {
                what: format!("reading the UUID of {device}"),
                code: None,
                signal: false,
                tail: vec![format!("blkid printed nothing for {device}")],
            });
        }
        Ok(uuid)
    }

    fn mount(&mut self, ui: &mut Ui, p: &mut Progress, l: &Layout) -> Done {
        let mut say = say(ui, p);
        // A desktop running udisks2 auto-mounts a partition the moment it gets
        // a filesystem, so by the time the previous step's `udevadm settle`
        // returns, `/dev/sda3` can already be at `/run/media/someone/kiln-root`
        // — and then `mount … /mnt` fails on a disk this program formatted
        // itself four seconds ago. Observed, not hypothetical. Best-effort:
        // nothing was mounted in the ordinary case and `umount` says so.
        for part in [&l.esp, &l.boot, &l.root] {
            let _ = self.run.run("umount", &["umount", part], &mut say);
        }
        let boot = format!("{MNT}/boot");
        let efi = format!("{MNT}/boot/efi");
        self.run.run("mkdir", &["mkdir", "-p", MNT], &mut say)?;
        self.run
            .run("mount /", &["mount", &l.root, MNT], &mut say)?;
        self.run
            .run("mkdir /boot", &["mkdir", "-p", &boot], &mut say)?;
        self.run
            .run("mount /boot", &["mount", &l.boot, &boot], &mut say)?;
        self.run
            .run("mkdir /boot/efi", &["mkdir", "-p", &efi], &mut say)?;
        self.run
            .run("mount /boot/efi", &["mount", &l.esp, &efi], &mut say)
    }

    /// Kiln initializing *its own* storage, in the same sense as `git init`.
    /// It is one command because the OSTree repository settings
    /// Kiln depends on — the stateroot, its `/var`, the ref layout — are Kiln's
    /// business and should not leak into a program that has no reason to know
    /// them.
    fn sysroot(&mut self, ui: &mut Ui, p: &mut Progress) -> Done {
        let mut say = say(ui, p);
        self.run.run(
            "kiln sysroot init",
            &["kiln", "sysroot", "init", MNT],
            &mut say,
        )
    }

    fn configure(&mut self, ui: &mut Ui, p: &mut Progress, a: &Answers) -> Done {
        let mut say = say(ui, p);
        let at = staged_config().join("system.toml");
        self.run.write(&at, &config::system_toml(a), &mut say)
    }

    fn build(&mut self, ui: &mut Ui, p: &mut Progress) -> Done {
        let config = staged_config().display().to_string();
        let modules = self.module_root.display().to_string();
        let mut argv = vec!["kiln", "--sysroot", MNT, "--config", &config];
        // Only when it is not the default, so the command in the log is the
        // command a user would type on the installed machine.
        if self.module_root != Path::new(crate::DEFAULT_MODULE_DIR) {
            argv.extend_from_slice(&["--module-root", &modules]);
        }
        argv.push("build");
        let mut say = say(ui, p);
        self.run.run("kiln build", &argv, &mut say)
    }

    /// The "make it bootable" step. `kiln build` commits and does not deploy,
    /// so generation 1 exists only in the repository at this point;
    /// `kiln deploy 1` is the command that deploys a generation which has only
    /// ever been committed, reading its kargs from the commit's own metadata
    /// rather than from whatever `/etc/kiln` says now.
    fn deploy(&mut self, ui: &mut Ui, p: &mut Progress) -> Done {
        let mut say = say(ui, p);
        self.run.run(
            "kiln deploy 1",
            &["kiln", "--sysroot", MNT, "deploy", "1"],
            &mut say,
        )
    }

    /// The two things the installer puts in the deployment's `/etc`: `fstab`,
    /// and the configuration itself.
    ///
    /// Both are written *there* rather than in `/mnt/etc` for the same reason
    /// the accounts are: the deployment's `/etc` is the machine's `/etc`, and
    /// libostree's three-way merge carries what is in it into every generation
    /// after this one. `/mnt/etc` is the physical root's, which the booted
    /// system sees as `/sysroot/etc` and never reads.
    ///
    /// **`/etc/fstab`** is the installer's file and stays the installer's:
    /// `fstab` and `crypttab` are on the short, permanent list of paths Kiln's
    /// drift detection deliberately never reports, because the storage layout
    /// is the installer's business, not Kiln's. So editing it later is a
    /// supported thing to do and Kiln will not argue about it.
    ///
    /// `/` is listed even though `ostree-prepare-root` has already mounted it
    /// from the initramfs — this is what Silverblue does, and systemd treats the
    /// entry as a remount rather than a second mount. `passno` is 0 for the
    /// deployment root: fsck on a root that is a bind of a subdirectory is not
    /// a meaningful thing to ask for.
    ///
    /// **`/etc/kiln`** is *moved* out of the sysroot, not copied into it.
    /// It is tempting to think of the configuration as "just a file at
    /// `/mnt/etc/kiln/system.toml`", and for the build that is exactly right —
    /// `--config` is a host path, read while `kiln build` runs. It is not where
    /// the file can stay. Left there, the installed machine boots with no
    /// `/etc/kiln` at all, `kiln check` answers "no configuration at
    /// /etc/kiln", and the configuration the installer spent an interview
    /// producing looks like it was never written. Observed, not hypothetical.
    ///
    /// Moved rather than copied because two `system.toml`s — one of them
    /// invisible, and stale from the moment the other is edited — is worse than
    /// either one alone. `cp -a` of the whole directory rather than a second
    /// render of `config::system_toml`: the config root is a *directory*,
    /// and the copy that should reach the machine is the one `kiln build`
    /// actually read.
    fn etc(&mut self, ui: &mut Ui, p: &mut Progress, dep: &Path, l: &Layout) -> Done {
        let text = format!(
            "# Written by terracotta-installer. The storage layout is the installer's, not\n\
             # Kiln's — `kiln status` deliberately never reports drift on this file.\n\
             UUID={}  /          ext4  rw,relatime                 0 0\n\
             UUID={}  /boot      ext4  rw,relatime                 0 2\n\
             UUID={}  /boot/efi  vfat  umask=0077,shortname=winnt  0 2\n",
            l.root_uuid, l.boot_uuid, l.esp_uuid
        );
        let mut say = say(ui, p);
        self.run.write(&dep.join("etc/fstab"), &text, &mut say)?;

        let staged = staged_config();
        // `<dir>/.` so the copy is of the config root's *contents*: a plain
        // `cp -a <dir> <dir>` nests a second `kiln/` inside the first if the
        // target somehow already exists.
        let from = staged.join(".").display().to_string();
        let staged = staged.display().to_string();
        let to = dep.join("etc/kiln").display().to_string();
        self.run.run("mkdir", &["mkdir", "-p", &to], &mut say)?;
        self.run
            .run("cp /etc/kiln", &["cp", "-a", &from, &to], &mut say)?;
        self.run
            .run("rm the staged copy", &["rm", "-r", &staged], &mut say)?;
        // The physical root has no other use for `/etc`, and an empty one left
        // behind at `/sysroot/etc` is a place to go looking for a configuration
        // that is not there. Best-effort, and non-empty is not an error: if
        // something else put a file there, it is not this program's to remove.
        let _ = self.run.run(
            "rmdir",
            &["rmdir", "--ignore-fail-on-non-empty", &format!("{MNT}/etc")],
            &mut say,
        );
        Ok(())
    }

    /// GRUB onto the disk, and the first `grub.cfg`.
    ///
    /// All of it runs **chrooted into the deployment**, and that is not a
    /// stylistic choice: `grub-mkconfig` sources `/etc/grub.d/*`, and the two
    /// fragments that matter — libostree's `15_ostree`, which turns BLS entries
    /// into menu entries, and Kiln's `09_kiln_boot_counter`, which is what
    /// makes automatic rollback happen — exist only inside the image.
    ///
    /// `grub-install` runs **twice, fallback first**, and the order is the
    /// point. `--removable` writes `\EFI\BOOT\BOOTX64.EFI` and touches no
    /// firmware variables; the named install writes `\EFI\kiln` *and* an NVRAM
    /// boot entry, which it does by executing `efibootmgr` — from inside the
    /// image, since that is what the chroot means. So the named install is the
    /// one that can fail for reasons that have nothing to do with the disk:
    /// `efibootmgr` missing from the image (`@kiln/boot/grub2` installs it
    /// alongside `grub` for exactly this reason, but a profile could in
    /// principle drop the module that carries it), `efivarfs` not mounted, or
    /// firmware with no room left in NVRAM. A machine with only the fallback
    /// path boots; a machine with neither does not — so the fallback is
    /// required, the NVRAM entry is best-effort and says so in the log, and a
    /// missing `efibootmgr` costs the boot menu entry rather than the whole
    /// install.
    ///
    /// **`grub-mkconfig`'s output then moves, and a symlink takes its place.**
    /// This is the step whose absence cost a machine its second boot, so it is
    /// worth the paragraph. libostree does not maintain `/boot/grub/grub.cfg`.
    /// It regenerates `/boot/loader.N/grub.cfg` on every deploy, alternating N
    /// between 0 and 1 so that a whole set of boot entries is swapped by
    /// renaming one symlink, `/boot/loader`; GRUB reads `$prefix/grub.cfg` and
    /// knows nothing about any of that. Silverblue joins the two with a symlink
    /// and `grub-install` on Arch does not — it writes a regular file, which is
    /// correct exactly once. That file names the bootversion current when it
    /// ran, `ostree=/ostree/boot.1/…`, and the machine's *next* deploy renames
    /// that directory to `boot.0`: GRUB then hands the kernel a path that does
    /// not exist, and the system stops in the initramfs with
    /// `ostree-prepare-root: Couldn't find specified OSTree root`. Kiln's own
    /// automatic rollback cannot save it, because a config generated for a
    /// one-deployment machine has one menu entry, so the `default="1"` the boot
    /// counter selects is the firmware setup entry.
    ///
    /// So the freshly generated config is *moved* into the loader directory
    /// rather than regenerated there: the deploy has already run, that
    /// directory exists, and those bytes are the only correct configuration on
    /// the disk until the first `kiln apply` regenerates them. Kiln repairs
    /// this on its first deploy to `/` if an installer leaves a regular file,
    /// but relying on that ships a machine whose first `kiln apply` is what
    /// makes it bootable twice — one power failure away from a rescue USB.
    ///
    /// **The deploy is not repeated afterwards**, and that is worth writing down
    /// because the obvious worry is wrong in one direction and the obvious fix
    /// is wrong in the other.
    ///
    /// The worry: the boot counter lives in `/boot/grub/grubenv`, `kiln
    /// deploy` armed it a step ago, and `grub-install` writes into
    /// `/boot/grub`. Measured, on grub 2.14 against a real vfat ESP and ext4
    /// `/boot`: an existing `grubenv` survives `grub-install` byte for byte,
    /// contents and all 1024 bytes of it. There is nothing to repair.
    ///
    /// The fix that suggests itself — run `kiln deploy 1` again to be safe —
    /// does the opposite of what it looks like. A generation that is already
    /// deployed takes `set_default`, and `set_default` *disarms* the counter on
    /// purpose: a generation chosen by hand gets no probation, because counting
    /// attempts against a decision the user just made would roll it back
    /// underneath them. Re-deploying here would therefore hand the new machine
    /// its first boot with no counter at all.
    fn bootloader(&mut self, ui: &mut Ui, p: &mut Progress, dep: &Path) -> Done {
        let guard = self.enter(ui, p, dep)?;
        let root = guard.root.display().to_string();
        let out = {
            let mut say = say(ui, p);
            // The fallback path, and the one the install depends on: firmware
            // that ignores its own boot variables, and every machine this disk
            // is ever imaged onto, finds the system through this and nothing
            // else.
            self.run
                .run(
                    "grub-install --removable",
                    &[
                        "chroot",
                        &root,
                        "grub-install",
                        "--target=x86_64-efi",
                        "--efi-directory=/boot/efi",
                        "--boot-directory=/boot",
                        "--removable",
                    ],
                    &mut say,
                )
                .and_then(|()| {
                    // The NVRAM entry. Best-effort, for the reasons above the
                    // function — and said out loud, because "the machine boots
                    // but there is no `kiln` entry in the firmware menu" is
                    // otherwise a mystery a week later.
                    let named = self.run.run(
                        "grub-install",
                        &[
                            "chroot",
                            &root,
                            "grub-install",
                            "--target=x86_64-efi",
                            "--efi-directory=/boot/efi",
                            "--boot-directory=/boot",
                            "--bootloader-id=kiln",
                        ],
                        &mut say,
                    );
                    if named.is_err() {
                        say(
                            "  no UEFI boot entry was written (grub-install needs efibootmgr \
                             and a mounted efivarfs); the machine boots the removable path at \
                             \\EFI\\BOOT\\BOOTX64.EFI",
                        );
                    }
                    self.run.run(
                        "grub-mkconfig",
                        &[
                            "chroot",
                            &root,
                            "grub-mkconfig",
                            "-o",
                            "/boot/grub/grub.cfg",
                        ],
                        &mut say,
                    )
                })
                .and_then(|()| {
                    // The config generated a moment ago is the only correct one
                    // on the disk, so it becomes libostree's rather than being
                    // regenerated: `mv` into the loader directory the deploy
                    // created, and leave the link in its place.
                    self.run.run(
                        "mv grub.cfg into the loader directory",
                        &[
                            "chroot",
                            &root,
                            "mv",
                            "/boot/grub/grub.cfg",
                            "/boot/loader/grub.cfg",
                        ],
                        &mut say,
                    )
                })
                .and_then(|()| {
                    self.run.run(
                        "link grub.cfg to the loader directory",
                        &[
                            "chroot",
                            &root,
                            "ln",
                            "-sf",
                            "../loader/grub.cfg",
                            "/boot/grub/grub.cfg",
                        ],
                        &mut say,
                    )
                })
        };
        self.leave(ui, p, guard);
        out
    }

    /// Hostname, timezone, and the accounts.
    ///
    /// None of this is image content and none of it goes near `/etc/kiln`.
    /// Kiln's schema has no `[[user]]` table on purpose — login accounts are
    /// not something Kiln manages — so they are created here, once, in the
    /// deployment's `/etc`, which the three-way merge carries forward into
    /// every generation after this one.
    ///
    /// `LANG` and `KEYMAP` are **not** set here even though `systemd-firstboot`
    /// would happily do it: `config.rs` writes them as `[[file]]`s, and two
    /// places setting the same value is exactly the ambiguity Kiln refuses
    /// everywhere else.
    fn accounts(&mut self, ui: &mut Ui, p: &mut Progress, dep: &Path, a: &Answers) -> Done {
        let guard = self.enter(ui, p, dep)?;
        let root = guard.root.display().to_string();
        let out = self.in_chroot(ui, p, &root, a);
        self.leave(ui, p, guard);
        out
    }

    fn in_chroot(&mut self, ui: &mut Ui, p: &mut Progress, root: &str, a: &Answers) -> Done {
        let mut say = say(ui, p);
        // `/home` and `/root` are symlinks into `/var`, and `/var` is empty
        // until the first boot's tmpfiles run. `useradd -m` would otherwise
        // create a home directory through a dangling symlink.
        self.run.run(
            "seeding /var",
            &["chroot", root, "mkdir", "-p", "-m", "0755", "/var/home"],
            &mut say,
        )?;
        self.run.run(
            "seeding /var",
            &["chroot", root, "mkdir", "-p", "-m", "0700", "/var/roothome"],
            &mut say,
        )?;
        self.run.run(
            "systemd-firstboot",
            &[
                "chroot",
                root,
                "systemd-firstboot",
                "--force",
                &format!("--hostname={}", a.hostname),
                &format!("--timezone={}", a.timezone),
            ],
            &mut say,
        )?;
        self.run.run(
            "useradd",
            &[
                "chroot",
                root,
                "useradd",
                "--create-home",
                "--groups",
                "wheel",
                "--shell",
                "/bin/bash",
                &a.username,
            ],
            &mut say,
        )?;
        self.run.run_stdin(
            "chpasswd",
            &["chroot", root, "chpasswd"],
            &format!("{}:{}\n", a.username, a.user_password),
            &mut say,
        )?;
        if a.root_password.is_empty() {
            self.run.run(
                "locking root",
                &["chroot", root, "passwd", "--lock", "root"],
                &mut say,
            )
        } else {
            self.run.run_stdin(
                "chpasswd root",
                &["chroot", root, "chpasswd"],
                &format!("root:{}\n", a.root_password),
                &mut say,
            )
        }
    }

    fn finish(&mut self, ui: &mut Ui, p: &mut Progress) -> Done {
        let mut say = say(ui, p);
        // Recursive, and it matters: `/boot` and `/boot/efi` are underneath,
        // and an ESP left mounted is an ESP with dirty pages on it when the
        // machine is powered off.
        self.run
            .run("umount", &["umount", "--recursive", MNT], &mut say)
    }

    /// Bind the four things a chroot into an OSTree deployment needs.
    ///
    /// `/sysroot` is the one that is easy to miss and impossible to work around
    /// afterwards: a deployment's `/ostree` is a symlink to `sysroot/ostree`,
    /// so without the sysroot bound, libostree's own `15_ostree`
    /// fragment cannot see the deployments it is meant to write menu entries
    /// for, and `grub-mkconfig` produces a `grub.cfg` that boots nothing.
    ///
    /// `/boot` is bound **recursively**, and that is the second thing that is
    /// easy to miss: the ESP is a mount *underneath* `/mnt/boot`, and a plain
    /// `--bind` copies one mount, not the subtree beneath it. Bound
    /// non-recursively, the deployment's `/boot/efi` is the empty ext4
    /// directory the mountpoint was made out of — and `grub-install` probes
    /// the filesystem there, finds ext4, and stops with "/boot/efi doesn't
    /// look like an EFI partition". Observed, not hypothetical.
    fn enter(&mut self, ui: &mut Ui, p: &mut Progress, dep: &Path) -> Result<Guard, Failed> {
        let mut mounted: Vec<PathBuf> = Vec::new();
        let var = PathBuf::from(MNT)
            .join("ostree/deploy")
            .join(STATEROOT)
            .join("var");
        let binds: [(PathBuf, PathBuf, bool); 7] = [
            (PathBuf::from(MNT), dep.join("sysroot"), false),
            (PathBuf::from(MNT).join("boot"), dep.join("boot"), true),
            (var, dep.join("var"), false),
            (PathBuf::from("/dev"), dep.join("dev"), true),
            (PathBuf::from("/proc"), dep.join("proc"), true),
            (PathBuf::from("/sys"), dep.join("sys"), true),
            (PathBuf::from("/run"), dep.join("run"), true),
        ];
        for (from, to, recursive) in binds {
            let mut say = say(ui, p);
            let flag = if recursive { "--rbind" } else { "--bind" };
            let (f, t) = (from.display().to_string(), to.display().to_string());
            let out = self
                .run
                .run("mkdir", &["mkdir", "-p", &t], &mut say)
                .and_then(|()| {
                    self.run
                        .run("bind mount", &["mount", flag, &f, &t], &mut say)
                });
            if let Err(e) = out {
                drop(say);
                self.leave(
                    ui,
                    p,
                    Guard {
                        root: dep.to_path_buf(),
                        mounted,
                    },
                );
                return Err(e);
            }
            mounted.push(to);
        }
        Ok(Guard {
            root: dep.to_path_buf(),
            mounted,
        })
    }

    /// Unmount in reverse, and never fail: an unmount that did not happen is
    /// reported on the next line and then dealt with by `umount -R /mnt`, but a
    /// `?` here would abandon the remaining mounts and leave the disk pinned.
    fn leave(&mut self, ui: &mut Ui, p: &mut Progress, guard: Guard) {
        let mut say = say(ui, p);
        for at in guard.mounted.iter().rev() {
            let path = at.display().to_string();
            let _ = self.run.run(
                "umount",
                &["umount", "--recursive", "--lazy", &path],
                &mut say,
            );
        }
    }
}

struct Guard {
    root: PathBuf,
    mounted: Vec<PathBuf>,
}

/// The deployment libostree just wrote, at
/// `<sysroot>/ostree/deploy/<stateroot>/deploy/<checksum>.<serial>`.
///
/// Exactly one is expected — this runs on a disk that was empty four steps ago
/// — and finding none or several is reported rather than guessed at, because
/// both cases mean the deploy did something other than what was asked and
/// picking one at random would put GRUB on a tree nobody chose.
fn deployment(sysroot: &Path, dry_run: bool) -> Result<PathBuf, Failed> {
    let at = sysroot.join("ostree/deploy").join(STATEROOT).join("deploy");
    if dry_run {
        // Nothing was deployed, because nothing was written. Returning a
        // plausible path is what lets `--dry-run` print the last four steps —
        // `grub-install`, `useradd`, the exact `chroot` lines — which are the
        // part of the plan somebody actually wants to read before committing a
        // disk to it.
        return Ok(at.join("0000000000000000000000000000000000000000000000000000000000000000.0"));
    }
    let fail = |message: String| Failed {
        what: "locating the deployment".into(),
        code: None,
        signal: false,
        tail: vec![message],
    };
    let entries =
        std::fs::read_dir(&at).map_err(|e| fail(format!("cannot read {}: {e}", at.display())))?;
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        // libostree writes `<checksum>.<serial>` directories and a `<checksum>.<serial>.origin`
        // file beside each one.
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(fail(format!(
            "no deployment under {} — `kiln deploy 1` reported success but wrote nothing",
            at.display()
        ))),
        n => Err(fail(format!(
            "{n} deployments under {} on a disk that was empty; refusing to guess which one \
             GRUB should be installed against",
            at.display()
        ))),
    }
}

/// A `say` closure that pushes into the progress tail and redraws.
///
/// Written as a function returning a closure so the borrow of `ui` and `p` is
/// scoped to the statement that uses it — every step needs `&mut self.run` at
/// the same time, and `self.run.run(…, |l| ui.progress(…))` would otherwise
/// borrow both across the call.
fn say<'a>(ui: &'a mut Ui, p: &'a mut Progress) -> impl FnMut(&str) + 'a {
    move |line: &str| {
        p.say(line);
        ui.progress(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_has_a_name() {
        assert_eq!(STEPS.len(), 11);
        assert_eq!(STEPS[0], "partition");
        assert_eq!(STEPS[STEPS.len() - 1], "finish");
    }

    #[test]
    fn a_missing_deployment_is_a_message_not_a_panic() {
        let e = deployment(Path::new("/nonexistent-sysroot"), false).unwrap_err();
        assert!(e.tail[0].contains("cannot read"), "{:?}", e.tail);
    }

    /// The configuration is written on the physical root and does not stay
    /// there: `/mnt/etc` is `/sysroot/etc` from the first boot onward, and
    /// `kiln` looks in `/etc/kiln`. The `etc` step is what moves it, so it has
    /// to come after the deploy — there is no deployment to move it into
    /// before then.
    #[test]
    fn the_config_is_staged_on_the_sysroot_and_installed_in_the_deployment() {
        assert_eq!(staged_config(), Path::new("/mnt/etc/kiln"));
        let at = |name: &str| STEPS.iter().position(|s| *s == name).expect(name);
        assert!(at("configure") < at("build"));
        assert!(at("deploy") < at("etc"));
    }

    /// This order is the thing this module exists to get right, so it is
    /// worth asserting rather than only commenting: the bootloader step cannot
    /// precede the deploy, because `grub-mkconfig` reads `/etc/grub.d` out of a
    /// deployment that does not exist until then.
    #[test]
    fn the_bootloader_comes_after_the_deploy() {
        let at = |name: &str| STEPS.iter().position(|s| *s == name).expect(name);
        assert!(at("sysroot") < at("build"));
        assert!(at("build") < at("deploy"));
        assert!(at("deploy") < at("bootloader"));
    }
}
