//! Running commands, and being able to say what happened afterwards.
//!
//! Every command the installer runs goes through here, and every line either
//! stream produces is written to the log **and** offered to a callback that
//! draws it. Two rules the rest of the program depends on:
//!
//! 1. **Nothing is silent.** An installer whose `mkfs` failed with a message
//!    nobody kept is an installer you cannot debug from a live ISO.
//! 2. **stdout and stderr interleave.** `kiln build` writes progress to one and
//!    diagnostics to the other; reading them in sequence would put the error
//!    twenty lines away from the thing that caused it.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Where a failed install goes to be read. On a live ISO this is tmpfs, so the
/// last thing the installer prints is a suggestion to copy it somewhere.
pub const LOG: &str = "/var/log/terracotta-installer.log";

/// How much of a failing command's output is kept, and shown. One number, so
/// the failure screen prints everything that was collected rather than most of
/// it.
const TAIL: usize = 12;

pub struct Runner {
    log: Option<File>,
    /// `None` when the log could not be opened — a live medium with a
    /// read-only `/var/log`, say. Kept so the failure screen can say that
    /// instead of pointing at a file that was never written.
    log_path: Option<PathBuf>,
    /// `--dry-run`: print the plan, touch nothing. Read commands still run —
    /// the point is to be able to see the exact `sgdisk` line before it is real.
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct Failed {
    pub what: String,
    pub code: Option<i32>,
    /// The process died on a signal rather than exiting. Distinguished from
    /// "there was no process" because the two read identically as
    /// `code: None` and mean very different things to whoever is debugging.
    pub signal: bool,
    /// The last `TAIL` lines the command produced, so the failure screen can
    /// say something better than "exit status 1".
    pub tail: Vec<String>,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (Some(c), _) => write!(f, "{} exited {}", self.what, c),
            (None, true) => write!(f, "{} was killed by a signal", self.what),
            (None, false) => write!(f, "{} failed", self.what),
        }
    }
}

pub type Done = Result<(), Failed>;

impl Runner {
    pub fn new(dry_run: bool) -> Runner {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG)
            .ok()
            .map(|mut f| {
                let _ = writeln!(f, "\n=== terracotta-installer {} ===", stamp());
                f
            });
        Runner {
            log_path: log.is_some().then(|| PathBuf::from(LOG)),
            log,
            dry_run,
        }
    }

    fn note(&mut self, line: &str) {
        if let Some(f) = &mut self.log {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Run a command, streaming both its streams to `say` in the order they
    /// arrive. `what` is what the user sees if it fails.
    pub fn run(&mut self, what: &str, argv: &[&str], mut say: impl FnMut(&str)) -> Done {
        self.run_in(what, argv, None, &mut say)
    }

    /// Run a command and write `stdin` to it.
    ///
    /// The callers are `chpasswd` and `cryptsetup`, and the reason this exists
    /// rather than `usermod -p` or a `cryptsetup` prompt is the same for both:
    /// a secret on a command line is visible in `ps` to every process on the
    /// machine for as long as the call takes, and a secret cryptsetup reads
    /// from the real terminal would fight the raw mode and alternate screen
    /// this program already holds. What goes in on stdin is **never logged**
    /// — the log records that something was piped and how many bytes, and
    /// nothing else.
    pub fn run_stdin(
        &mut self,
        what: &str,
        argv: &[&str],
        stdin: &str,
        say: &mut impl FnMut(&str),
    ) -> Done {
        self.exec(what, argv, None, Some(stdin), say)
    }

    /// The same, with a working directory.
    pub fn run_in(
        &mut self,
        what: &str,
        argv: &[&str],
        cwd: Option<&Path>,
        say: &mut impl FnMut(&str),
    ) -> Done {
        self.exec(what, argv, cwd, None, say)
    }

    fn exec(
        &mut self,
        what: &str,
        argv: &[&str],
        cwd: Option<&Path>,
        stdin: Option<&str>,
        say: &mut impl FnMut(&str),
    ) -> Done {
        let printed = match stdin {
            Some(text) => format!("{} <<< ({} bytes, not logged)", argv.join(" "), text.len()),
            None => argv.join(" "),
        };
        self.note(&format!("$ {printed}"));
        say(&format!("$ {printed}"));
        if self.dry_run && !read_only(argv) {
            say("  (dry run — not executed)");
            return Ok(());
        }

        let (program, rest) = argv.split_first().expect("a command with no argv[0]");
        let mut cmd = Command::new(program);
        cmd.args(rest)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        // Deterministic, parseable output from anything that localizes itself,
        // and no pager or colour codes landing in the log.
        cmd.env("LC_ALL", "C").env("TERM", "dumb");

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let line = format!("cannot run {program}: {e}");
                self.note(&line);
                say(&line);
                return Err(Failed {
                    what: what.into(),
                    signal: false,
                    code: None,
                    tail: vec![line],
                });
            }
        };

        // On its own thread, and started before the readers below: a child
        // that writes more than a pipe buffer before draining its stdin would
        // otherwise deadlock against a parent that has not begun reading.
        // `chpasswd` and `cryptsetup` never do, but the shape is the bug.
        let feed = stdin.map(|text| {
            let text = text.to_string();
            let pipe = child.stdin.take();
            std::thread::spawn(move || {
                if let Some(mut pipe) = pipe {
                    let _ = pipe.write_all(text.as_bytes());
                }
            })
        });

        // One channel, two threads: whichever stream produces a line first is
        // the line that gets shown first.
        let (tx, rx) = mpsc::channel::<String>();
        let mut pumps = Vec::new();
        if let Some(o) = child.stdout.take() {
            pumps.push(pump(o, tx.clone()));
        }
        if let Some(e) = child.stderr.take() {
            pumps.push(pump(e, tx.clone()));
        }
        drop(tx);

        let mut tail: VecDeque<String> = VecDeque::new();
        for line in rx {
            self.note(&line);
            say(&line);
            if tail.len() == TAIL {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        for p in pumps {
            let _ = p.join();
        }
        if let Some(feed) = feed {
            let _ = feed.join();
        }

        match child.wait() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => {
                self.note(&format!("!! {what} exited {status}"));
                Err(Failed {
                    what: what.into(),
                    signal: status.code().is_none(),
                    code: status.code(),
                    tail: tail.into(),
                })
            }
            Err(e) => Err(Failed {
                what: what.into(),
                signal: false,
                code: None,
                tail: vec![e.to_string()],
            }),
        }
    }

    /// Run something and keep its stdout, for the handful of questions the
    /// installer asks the system — `blkid`, `lsblk`, `localectl`. These run
    /// under `--dry-run` too: they change nothing, and the plan is wrong
    /// without their answers.
    ///
    /// Anything *not* on the `read_only` list is refused under `--dry-run`
    /// rather than quietly spawned, so the allowlist is the one place that
    /// decides what a dry run may execute. A caller that needs an answer a dry
    /// run cannot have — `cryptsetup luksUUID` of a container that was never
    /// formatted — supplies its own placeholder before calling.
    pub fn capture(&mut self, argv: &[&str]) -> Result<String, Failed> {
        let (program, rest) = argv.split_first().expect("a command with no argv[0]");
        if self.dry_run && !read_only(argv) {
            return Err(Failed {
                what: argv.join(" "),
                signal: false,
                code: None,
                tail: vec![format!(
                    "--dry-run: `{program}` is not on the read-only list, so it was not run"
                )],
            });
        }
        let out = Command::new(program)
            .args(rest)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(o) => Err(Failed {
                what: argv.join(" "),
                signal: false,
                code: o.status.code(),
                tail: last_lines(&String::from_utf8_lossy(&o.stderr)),
            }),
            Err(e) => Err(Failed {
                what: argv.join(" "),
                signal: false,
                code: None,
                tail: vec![e.to_string()],
            }),
        }
    }

    /// Write a file, logging what went into it. Under `--dry-run` the content
    /// is logged and the file is not written, which is how you check a
    /// generated `system.toml` without a disk.
    pub fn write(&mut self, at: &Path, content: &str, say: &mut impl FnMut(&str)) -> Done {
        self.note(&format!("--- {} ---\n{content}--- end ---", at.display()));
        say(&format!("write {}", at.display()));
        if self.dry_run {
            say("  (dry run — not written)");
            return Ok(());
        }
        let write = at
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| std::fs::write(at, content));
        write.map_err(|e| Failed {
            what: format!("writing {}", at.display()),
            signal: false,
            code: None,
            tail: vec![e.to_string()],
        })
    }

    /// Where the log went, or `None` if it could not be opened at all.
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }
}

/// Commands that only ask questions, and therefore run even under `--dry-run`.
/// The list is short and explicit on purpose: the failure mode of getting it
/// wrong is a "dry" run that partitions a disk.
fn read_only(argv: &[&str]) -> bool {
    matches!(argv.first().copied(), Some("lsblk" | "blkid" | "localectl"))
}

/// The last `TAIL` lines of `text`, so every `Failed` carries the same amount
/// of context however it was produced.
fn last_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL)..]
        .iter()
        .map(|l| (*l).to_string())
        .collect()
}

fn pump(
    stream: impl std::io::Read + Send + 'static,
    tx: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// Seconds since the epoch. A timestamp in the log wants to be sortable and to
/// need no dependency; a human reading it has `date -d @…`.
fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
