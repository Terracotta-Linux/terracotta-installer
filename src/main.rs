//! `terracotta-installer` — put a Kiln system on a disk.
//!
//! Kiln does not install anything itself; installation belongs to a separate
//! program, with its own release cycle, that owns disks, partition tables,
//! filesystems, the ESP, locale and keyboard prompts, network setup, and the
//! first user account. This is that program: its own repository, its own
//! version, its own tags. It is deliberately **not** a Kiln subcommand, links
//! no `kiln-*` crate, and reaches Kiln only through the `kiln` binary and the
//! `--sysroot` seam.
//!
//! An installer built on that seam is a few hundred lines of orchestration.
//! That is the correct size for it, and it is somebody else's few hundred
//! lines.
//!
//! The shape:
//!
//! ```text
//! preflight → interview → eleven steps → reboot
//!             (the last screen of the interview is the review, and nothing
//!              is written before it is accepted)
//! ```

mod block;
mod catalog;
mod config;
mod interview;
mod preflight;
mod run;
mod steps;
mod tui;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tui::{Opt, Page, Ui, ACCENT, BOLD, DANGER, DIM, OK, RESET, WARN};

/// `kiln-config`'s `discover::DEFAULT_MODULE_DIR`. Named again rather than
/// linked, for the reason the whole program is written this way.
pub const DEFAULT_MODULE_DIR: &str = "/usr/share/kiln/modules";

const USAGE: &str = "\
terracotta-installer — install Terracotta Linux on a disk

  terracotta-installer                    run the installer
  terracotta-installer --dry-run          ask everything, write nothing, print the plan
  terracotta-installer --module-root DIR  a module library other than /usr/share/kiln/modules
  terracotta-installer --version
  terracotta-installer --help

It partitions one disk, initializes a Kiln sysroot on it, builds generation 1,
deploys it, installs GRUB and creates the first account. Every question is asked
before anything is written, and Esc goes back a screen.

Kiln itself has no `install` verb and never will; this program drives the
`kiln` binary through --sysroot, which is the whole of the contract between
them.
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut dry_run = false;
    let mut module_root = std::env::var_os("KILN_MODULE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MODULE_DIR));

    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--version" => {
                println!("terracotta-installer {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--dry-run" => dry_run = true,
            "--module-root" => match it.next() {
                Some(v) => module_root = PathBuf::from(v),
                None => return die("--module-root needs a path"),
            },
            other => return die(&format!("unknown option `{other}`; try --help")),
        }
    }

    // Before the screen is taken, so a preflight failure is a message in the
    // scrollback rather than one that vanishes with the alternate screen.
    let problems = preflight::check(&module_root, dry_run);
    if !problems.is_empty() && !dry_run {
        eprintln!("\n{BOLD}terracotta-installer cannot start.{RESET}\n");
        for p in &problems {
            eprintln!("  {DANGER}·{RESET} {BOLD}{}{RESET}", p.what);
            eprintln!("    {DIM}{}{RESET}\n", p.fix);
        }
        return ExitCode::from(4);
    }
    if !problems.is_empty() {
        eprintln!(
            "{DIM}--dry-run: continuing past {} preflight problem(s).{RESET}",
            problems.len()
        );
    }

    // Raw mode must be given back even when something panics, or the shell the
    // user lands in does not echo what they type.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        Ui::restore();
        previous(info);
    }));

    let mut runner = run::Runner::new(dry_run);
    let mut ui = match Ui::open() {
        Ok(ui) => ui,
        Err(e) => return die(&e),
    };

    let mut answers = match interview::ask(&mut ui, &mut runner, &module_root) {
        Ok(Some(a)) => a,
        // Ctrl-C before the review screen. Nothing was written, so there is
        // nothing to say beyond that.
        Ok(None) => {
            drop(ui);
            println!("Nothing was written. The disk is untouched.");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            drop(ui);
            return die(&e);
        }
    };

    let mut installer = steps::Installer {
        run: &mut runner,
        module_root: module_root.clone(),
    };
    let started = Instant::now();
    let outcome = installer.install(&mut ui, &mut answers);
    let elapsed = started.elapsed();
    let log = runner.log_path();

    match outcome {
        Ok(()) => {
            let title = if dry_run {
                "Dry run complete"
            } else {
                "Installed"
            };
            let mut done = Page::new((0, 0), title)
                .note(format!(
                    "{DIM}finished in {}{RESET}",
                    format_duration(elapsed)
                ))
                .note(String::new());
            if dry_run {
                done = done
                    .note(format!(
                        "{WARN}Nothing was written. Every command above is what a real run{RESET}"
                    ))
                    .note(format!("{WARN}would have executed, in order.{RESET}"))
                    .note(String::new());
            }
            let done = done
                .note(if dry_run {
                    format!(
                        "{DIM}Generation 1 would be on {} and set to boot.{RESET}",
                        answers.disk.path
                    )
                } else {
                    format!(
                        "{OK}Generation 1 is on {} and set to boot.{RESET}",
                        answers.disk.path
                    )
                })
                .note(String::new())
                .note(format!(
                    "  {DIM}configuration{RESET}  /etc/kiln/system.toml"
                ))
                .note(format!(
                    "  {DIM}first user{RESET}     {} (wheel)",
                    answers.username
                ))
                .note(format!("  {DIM}hostname{RESET}       {}", answers.hostname))
                .note(String::new())
                .note("Once it is up:".to_string())
                .note(format!(
                    "  {ACCENT}kiln status{RESET}    what is booted, and whether /etc has drifted"
                ))
                .note(format!(
                    "  {ACCENT}kiln check{RESET}     what a rebuild would change"
                ))
                .note(format!(
                    "  {ACCENT}kiln apply{RESET}     build the next generation and stage it"
                ))
                .note(format!(
                    "  {ACCENT}kiln rollback{RESET}  if that was a mistake"
                ))
                .note(String::new())
                .note(format!(
                    "{DIM}Generation 1 is the baseline: `kiln clean` will not remove it,{RESET}"
                ))
                .note(format!(
                    "{DIM}and a machine that fails to boot three times returns to it.{RESET}"
                ))
                .note(String::new())
                .note(if dry_run {
                    "Run without --dry-run to do it for real.".to_string()
                } else {
                    "Remove the installation medium, then choose below.".to_string()
                });

            // The last screen: there is no screen to go back to, and Esc here
            // would leave the installer rather than return anywhere, so neither
            // legend offers it.
            if dry_run {
                let _ = ui.pause(&done.help("enter · leave the installer"));
                drop(ui);
                return ExitCode::SUCCESS;
            }

            let choice = ui.select(
                &done.help("↑↓ move · enter select"),
                &[
                    Opt::new("Reboot now", "into the system just installed"),
                    Opt::new("Exit to a shell", "stay on the installation medium"),
                ],
            );
            drop(ui);
            match choice {
                Ok(0) => reboot(),
                _ => exit_to_shell(),
            }
        }
        Err(e) => {
            let summary = |out: &mut Vec<String>| {
                out.push(format!("{BOLD}{e}{RESET}"));
                out.push(String::new());
                for line in &e.tail {
                    out.push(format!("{DIM}{line}{RESET}"));
                }
                out.push(String::new());
                // The log is the only artefact a failed install leaves, so
                // pointing at one that was never opened is the worst possible
                // moment to be wrong about a path.
                match &log {
                    Some(at) => {
                        out.push(format!(
                            "Everything that ran is in {BOLD}{}{RESET}.",
                            at.display()
                        ));
                        out.push(format!(
                            "{DIM}On a live medium that is tmpfs — copy it somewhere before \
                             rebooting.{RESET}"
                        ));
                    }
                    None => out.push(format!(
                        "{WARN}Nothing was logged: {} could not be opened for writing.{RESET}",
                        run::LOG
                    )),
                }
                out.push(String::new());
                out.push(format!(
                    "{DIM}The disk is in whatever state the failing step left it; running{RESET}"
                ));
                out.push(format!(
                    "{DIM}terracotta-installer again starts from an erase, which is a clean slate.{RESET}"
                ));
            };

            // The same taxonomy as `kiln` itself: 3 is a build failure,
            // 4 is the system refusing. Everything this program can fail at is
            // one of the two, and a script wrapping it should be able to tell
            // them apart.
            let code = ExitCode::from(if e.what.starts_with(steps::KILN_BUILD) {
                3
            } else {
                4
            });

            if dry_run {
                drop(ui);
                eprintln!("\n{DANGER}{BOLD}The install failed.{RESET}\n");
                let mut lines = Vec::new();
                summary(&mut lines);
                for line in lines {
                    eprintln!("  {line}");
                }
                return code;
            }

            let mut failed = Page::new((0, 0), "The install failed").note(String::new());
            let mut lines = Vec::new();
            summary(&mut lines);
            for line in lines {
                failed = failed.note(line);
            }
            let choice = ui.select(
                &failed.help("↑↓ move · enter select"),
                &[
                    Opt::new("Reboot", "the disk may be incomplete or unbootable"),
                    Opt::new("Exit to a shell", "leave the disk exactly as it is"),
                    Opt::new(
                        "Unmount and exit to a shell",
                        "best-effort umount of /mnt first",
                    ),
                ],
            );
            drop(ui);
            match choice {
                Ok(0) => reboot(),
                Ok(2) => {
                    unmount_best_effort(answers.encrypt);
                    exit_to_shell();
                }
                _ => exit_to_shell(),
            }
        }
    }
}

/// Reboot the machine. Best-effort: if neither works, fall through and let
/// the caller's exit code stand — a machine that cannot reboot itself was
/// already going to need a hand.
fn reboot() -> ! {
    let _ = std::process::Command::new("systemctl")
        .arg("reboot")
        .status();
    let _ = std::process::Command::new("reboot").status();
    std::process::exit(1);
}

/// `umount --recursive --lazy /mnt`, errors ignored. Not the precise,
/// only-what-was-mounted unmount `finish()` does when every step succeeds —
/// this runs after an arbitrary step failed partway through, so it does not
/// know what is actually mounted. `--lazy` is what makes that safe to call
/// unconditionally: a target that was never mounted just says so.
///
/// The LUKS mapping goes with it, for the reason `finish()` closes it too: the
/// failure screen offers a second run as a clean slate, and a container left
/// open under a fixed name is what stops that second run from erasing the disk
/// it just failed on.
fn unmount_best_effort(encrypted: bool) {
    let _ = std::process::Command::new("umount")
        .args(["--recursive", "--lazy", steps::MNT])
        .status();
    if encrypted {
        let _ = std::process::Command::new("cryptsetup")
            .args(["close", steps::LUKS_NAME])
            .status();
    }
}

/// Replace this process with an interactive shell. The installer runs as
/// tty1's login shell on the live medium (`exec terracotta-installer` in
/// `.bash_profile`), so an ordinary `exit` here would hand control back to
/// `agetty`, which would just log in and `exec` the installer again — this
/// is what actually leaves it, by taking over the process the way it was
/// taken over.
fn exit_to_shell() -> ! {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let err = std::process::Command::new(&shell).exec();
    eprintln!("{DANGER}error{RESET} cannot exec {shell}: {err}");
    std::process::exit(1);
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

fn die(message: &str) -> ExitCode {
    eprintln!("{DANGER}error{RESET} {message}");
    ExitCode::from(4)
}
