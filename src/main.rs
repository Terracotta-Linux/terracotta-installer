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
//! preflight → interview → review → eleven steps → reboot
//!             (nothing is written before the review screen is accepted)
//! ```

mod block;
mod catalog;
mod config;
mod interview;
mod preflight;
mod run;
mod steps;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;
use tui::{Page, Ui, ACCENT, BOLD, DANGER, DIM, OK, RESET, WARN};

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
    let problems = preflight::check(&module_root);
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

    let answers = match interview::ask(&mut ui, &mut runner, &module_root) {
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

    let mut answers = answers;
    let mut installer = steps::Installer {
        run: &mut runner,
        module_root: module_root.clone(),
    };
    let outcome = installer.install(&mut ui, &mut answers);
    let log = runner.log_path();

    match outcome {
        Ok(()) => {
            let title = if dry_run {
                "Dry run complete"
            } else {
                "Installed"
            };
            let mut done = Page::new((0, 0), title).note(String::new());
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
                    "Remove the installation medium and reboot.".to_string()
                });
            let _ = ui.pause_with(&done, "enter · leave the installer");
            drop(ui);
            ExitCode::SUCCESS
        }
        Err(e) => {
            drop(ui);
            eprintln!("\n{DANGER}{BOLD}The install failed.{RESET}\n");
            eprintln!("  {BOLD}{e}{RESET}\n");
            for line in e.tail.iter().rev().take(12).rev() {
                eprintln!("  {DIM}{line}{RESET}");
            }
            eprintln!(
                "\n  Everything that ran is in {BOLD}{}{RESET}.",
                log.display()
            );
            eprintln!(
                "  {DIM}On a live medium that is tmpfs — copy it somewhere before rebooting.{RESET}"
            );
            eprintln!(
                "\n  {DIM}The disk is in whatever state the failing step left it; running\n  \
                 terracotta-installer again starts from an erase, which is a clean slate.{RESET}\n"
            );
            // The same taxonomy as `kiln` itself: 3 is a build failure,
            // 4 is the system refusing. Everything this program can fail at is
            // one of the two, and a script wrapping it should be able to tell
            // them apart.
            ExitCode::from(if e.what.starts_with("kiln build") {
                3
            } else {
                4
            })
        }
    }
}

fn die(message: &str) -> ExitCode {
    eprintln!("{DANGER}error{RESET} {message}");
    ExitCode::from(4)
}
