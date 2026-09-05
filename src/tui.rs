//! The terminal: raw mode, the alternate screen, and six widgets.
//!
//! Styling is hand-written ANSI rather than crossterm's `style` module, for the
//! same reason `kiln-cli` writes `\x1b[1;31merror\x1b[0m` by hand: a line is a
//! `String` that can be built, measured and tested, and a widget that returns
//! one is easier to read than fifteen `queue!` calls. crossterm is here for the
//! three things that genuinely need a library — raw mode, the alternate screen,
//! and decoding key events.
//!
//! Every widget returns `Answer<T>`, which is `Result<T, Nav>`: `Nav::Back`
//! from Esc and `Nav::Quit` from Ctrl-C. That is what makes the interview in
//! `interview.rs` a state machine with a working back button instead of a
//! straight line of prompts you cannot correct.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use crossterm::tty::IsTty;
use crossterm::{cursor, execute, queue};
use std::collections::VecDeque;
use std::io::{self, Stdout, Write};

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const ACCENT: &str = "\x1b[36m";
pub const OK: &str = "\x1b[32m";
pub const WARN: &str = "\x1b[33m";
pub const DANGER: &str = "\x1b[31m";

/// Left margin. Everything is drawn two columns in; nothing is centred, because
/// a screen that reflows as you type is harder to read than one that does not.
const PAD: &str = "  ";

/// The smallest terminal the screens fit in. A serial console is 80x24 and an
/// installer that cannot run on one is not an installer.
const MIN_COLS: u16 = 60;
const MIN_ROWS: u16 = 18;

/// How the user left a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// Esc — go back one screen.
    Back,
    /// Ctrl-C — leave the installer without touching the disk.
    Quit,
}

pub type Answer<T> = Result<T, Nav>;

/// A screen's chrome: where it is in the interview, what it is asking, and any
/// standing explanation the question needs.
pub struct Page {
    pub step: (usize, usize),
    pub title: String,
    pub note: Vec<String>,
}

impl Page {
    pub fn new(step: (usize, usize), title: impl Into<String>) -> Page {
        Page {
            step,
            title: title.into(),
            note: Vec::new(),
        }
    }

    pub fn note(mut self, line: impl Into<String>) -> Page {
        self.note.push(line.into());
        self
    }
}

/// One row in a list.
pub struct Opt {
    pub label: String,
    pub note: String,
    /// A heading is drawn and skipped over; it cannot be selected. The module
    /// list is twelve namespaces, and a flat list of thirty-five entries with
    /// no structure is a worse question than the same entries under headings.
    pub heading: bool,
    /// A disk that is currently mounted, say. Shown, explained, not choosable.
    pub enabled: bool,
    /// Pre-checked when a `multiselect` screen opens.
    pub checked: bool,
}

impl Opt {
    pub fn new(label: impl Into<String>, note: impl Into<String>) -> Opt {
        Opt {
            label: label.into(),
            note: note.into(),
            heading: false,
            enabled: true,
            checked: false,
        }
    }

    pub fn heading(label: impl Into<String>) -> Opt {
        Opt {
            label: label.into(),
            note: String::new(),
            heading: true,
            enabled: false,
            checked: false,
        }
    }

    pub fn disabled(mut self) -> Opt {
        self.enabled = false;
        self
    }

    pub fn checked(mut self) -> Opt {
        self.checked = true;
        self
    }
}

/// The terminal, for as long as this is alive.
pub struct Ui {
    out: Stdout,
    cols: u16,
    rows: u16,
}

impl Ui {
    /// Take the terminal. Fails rather than degrading when there is not one:
    /// an installer that half-works over a pipe is a way to lose a disk.
    pub fn open() -> Result<Ui, String> {
        let mut out = io::stdout();
        if !out.is_tty() {
            return Err(
                "terracotta-installer needs a terminal; it is interactive by design".into(),
            );
        }
        let (cols, rows) =
            terminal::size().map_err(|e| format!("cannot size the terminal: {e}"))?;
        if cols < MIN_COLS || rows < MIN_ROWS {
            return Err(format!(
                "the terminal is {cols}x{rows}; terracotta-installer needs at least {MIN_COLS}x{MIN_ROWS}"
            ));
        }
        terminal::enable_raw_mode().map_err(|e| format!("cannot enter raw mode: {e}"))?;
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)
            .map_err(|e| format!("cannot take the screen: {e}"))?;
        Ok(Ui { out, cols, rows })
    }

    /// Give the terminal back. Idempotent, and called from `Drop` and from the
    /// panic hook — a panic inside raw mode otherwise leaves a shell that does
    /// not echo, which is a far worse bug than whatever caused the panic.
    pub fn restore() {
        let mut out = io::stdout();
        let _ = execute!(out, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        let _ = out.flush();
    }

    fn resized(&mut self) {
        if let Ok((c, r)) = terminal::size() {
            self.cols = c;
            self.rows = r;
        }
    }

    /// Paint a whole screen. Lines already carry their own escapes.
    fn paint(&mut self, lines: &[String]) {
        let _ = queue!(
            self.out,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        );
        for line in lines.iter().take(self.rows as usize) {
            let _ = write!(self.out, "{line}\r\n");
        }
        let _ = self.out.flush();
    }

    fn header(&self, page: &Page, out: &mut Vec<String>) {
        let (n, of) = page.step;
        let right = if of == 0 {
            String::new()
        } else {
            format!("step {n} of {of}")
        };
        let left = format!("{BOLD}terracotta install{RESET}");
        let gap = (self.cols as usize)
            .saturating_sub(PAD.len() * 2 + "terracotta install".len() + right.len())
            .max(1);
        out.push(String::new());
        out.push(format!(
            "{PAD}{left}{:gap$}{DIM}{right}{RESET}",
            "",
            gap = gap
        ));
        out.push(format!(
            "{PAD}{DIM}{}{RESET}",
            "─".repeat((self.cols as usize).saturating_sub(PAD.len() * 2))
        ));
        out.push(String::new());
        out.push(format!("{PAD}{BOLD}{}{RESET}", page.title));
        for line in &page.note {
            // A note that styles itself — a red warning, an accented key name —
            // is left alone. Wrapping it in DIM would end at its first inner
            // reset and leave the rest of the line undimmed, which is worse
            // than either choice made consistently.
            if line.contains('\x1b') {
                out.push(format!("{PAD}{line}"));
            } else {
                out.push(format!("{PAD}{DIM}{line}{RESET}"));
            }
        }
        out.push(String::new());
    }

    fn footer(&self, help: &str, out: &mut Vec<String>) {
        // Pinned to the bottom, so the key legend does not move as a list
        // filters down to two entries.
        while out.len() + 2 < self.rows as usize {
            out.push(String::new());
        }
        out.push(format!("{PAD}{DIM}{help}{RESET}"));
    }

    /// Pick one. Typing filters, which is what makes six hundred timezones a
    /// usable question rather than a scrolling exercise.
    pub fn select(&mut self, page: &Page, options: &[Opt]) -> Answer<usize> {
        let width = column(options);
        let mut filter = String::new();
        let mut cursor = 0usize;
        let mut top = 0usize;
        loop {
            let shown: Vec<usize> = (0..options.len())
                .filter(|&i| {
                    options[i].heading
                        || filter.is_empty()
                        || options[i]
                            .label
                            .to_lowercase()
                            .contains(&filter.to_lowercase())
                })
                .collect();
            // A heading whose whole group filtered away is noise.
            let shown: Vec<usize> = shown
                .iter()
                .copied()
                .enumerate()
                .filter(|&(pos, i)| {
                    !options[i].heading || shown[pos + 1..].iter().any(|&j| !options[j].heading)
                })
                .map(|(_, i)| i)
                .collect();

            let selectable: Vec<usize> = shown
                .iter()
                .copied()
                .filter(|&i| !options[i].heading && options[i].enabled)
                .collect();
            if selectable.is_empty() && !filter.is_empty() {
                cursor = 0;
            } else if !selectable.is_empty() {
                cursor = cursor.min(selectable.len() - 1);
            }

            let mut lines = Vec::new();
            self.header(page, &mut lines);
            if !filter.is_empty() {
                lines.push(format!("{PAD}{ACCENT}/{filter}{RESET}"));
                lines.push(String::new());
            }

            let room = (self.rows as usize).saturating_sub(lines.len() + 3).max(3);
            let here = selectable
                .get(cursor)
                .and_then(|&i| shown.iter().position(|&j| j == i))
                .unwrap_or(0);
            if here < top {
                top = here;
            }
            if here >= top + room {
                top = here + 1 - room;
            }
            top = top.min(shown.len().saturating_sub(room));

            if shown.is_empty() {
                lines.push(format!("{PAD}{DIM}  nothing matches `{filter}`{RESET}"));
            }
            for (pos, &i) in shown.iter().enumerate().skip(top).take(room) {
                lines.push(self.row(&options[i], pos == here && !selectable.is_empty(), width));
            }
            if shown.len() > top + room {
                lines.push(format!(
                    "{PAD}{DIM}  … {} more{RESET}",
                    shown.len() - top - room
                ));
            }

            self.footer(
                "↑↓ move · type to filter · enter select · esc back · ctrl-c quit",
                &mut lines,
            );
            self.paint(&lines);

            match self.key()? {
                Key::Up => cursor = cursor.saturating_sub(1),
                Key::Down => {
                    if !selectable.is_empty() {
                        cursor = (cursor + 1).min(selectable.len() - 1)
                    }
                }
                Key::Enter => {
                    if let Some(&i) = selectable.get(cursor) {
                        return Ok(i);
                    }
                }
                Key::Backspace => {
                    filter.pop();
                    cursor = 0;
                }
                Key::Char(c) => {
                    filter.push(c);
                    cursor = 0;
                }
                Key::Space => {
                    filter.push(' ');
                    cursor = 0;
                }
                Key::Resize => self.resized(),
                _ => {}
            }
        }
    }

    /// Pick any number. No filter: the only multi-select in the interview is
    /// the module library, which is grouped by namespace and meant to be read.
    pub fn multiselect(&mut self, page: &Page, options: &[Opt]) -> Answer<Vec<usize>> {
        let width = column(options);
        let mut chosen: Vec<bool> = options.iter().map(|o| o.checked).collect();
        let selectable: Vec<usize> = (0..options.len())
            .filter(|&i| !options[i].heading && options[i].enabled)
            .collect();
        let mut cursor = 0usize;
        let mut top = 0usize;
        loop {
            let mut lines = Vec::new();
            self.header(page, &mut lines);
            let room = (self.rows as usize).saturating_sub(lines.len() + 3).max(3);
            let here = selectable.get(cursor).copied().unwrap_or(0);
            if here < top {
                top = here;
            }
            if here >= top + room {
                top = here + 1 - room;
            }
            top = top.min(options.len().saturating_sub(room));

            for (i, opt) in options.iter().enumerate().skip(top).take(room) {
                if opt.heading {
                    lines.push(format!("{PAD}{DIM}{}{RESET}", opt.label));
                    continue;
                }
                let mark = if chosen[i] {
                    format!("{OK}[x]{RESET}")
                } else {
                    format!("{DIM}[ ]{RESET}")
                };
                let sel = i == here;
                let arrow = if sel {
                    format!("{ACCENT}▸{RESET}")
                } else {
                    " ".into()
                };
                let label = if sel {
                    format!("{BOLD}{}{RESET}", opt.label)
                } else {
                    opt.label.clone()
                };
                let note = if opt.note.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{:pad$}  {DIM}{}{RESET}",
                        "",
                        opt.note,
                        pad = pad(&opt.label, width)
                    )
                };
                lines.push(format!("{PAD}{arrow} {mark} {label}{note}"));
            }
            if options.len() > top + room {
                lines.push(format!(
                    "{PAD}{DIM}  … {} more{RESET}",
                    options.len() - top - room
                ));
            }
            self.footer(
                "↑↓ move · space toggle · enter continue · esc back · ctrl-c quit",
                &mut lines,
            );
            self.paint(&lines);

            match self.key()? {
                Key::Up => cursor = cursor.saturating_sub(1),
                Key::Down => cursor = (cursor + 1).min(selectable.len().saturating_sub(1)),
                Key::Space => chosen[here] = !chosen[here],
                Key::Enter => {
                    return Ok((0..options.len()).filter(|&i| chosen[i]).collect());
                }
                Key::Resize => self.resized(),
                _ => {}
            }
        }
    }

    /// A line of text, with validation shown under the field rather than after
    /// it is accepted.
    pub fn text(
        &mut self,
        page: &Page,
        label: &str,
        initial: &str,
        check: impl Fn(&str) -> Result<(), String>,
    ) -> Answer<String> {
        self.field(page, label, initial, false, check)
    }

    /// A password, entered twice. `allow_empty` is how "no root password, use
    /// sudo" is expressed without a separate screen.
    pub fn secret(&mut self, page: &Page, label: &str, allow_empty: bool) -> Answer<String> {
        loop {
            let first = self.field(page, label, "", true, move |s: &str| {
                if s.is_empty() && !allow_empty {
                    Err("a password is required".into())
                } else if !s.is_empty() && s.len() < 6 {
                    Err("at least six characters".into())
                } else {
                    Ok(())
                }
            })?;
            if first.is_empty() {
                return Ok(first);
            }
            let again = self.field(page, "repeat it", "", true, |_: &str| Ok(()))?;
            if again == first {
                return Ok(first);
            }
            let p = Page::new(page.step, page.title.clone())
                .note(format!("{DANGER}they do not match — try again{RESET}"));
            self.pause(&p)?;
        }
    }

    fn field(
        &mut self,
        page: &Page,
        label: &str,
        initial: &str,
        mask: bool,
        check: impl Fn(&str) -> Result<(), String>,
    ) -> Answer<String> {
        let mut value = initial.to_string();
        let mut error: Option<String> = None;
        loop {
            let mut lines = Vec::new();
            self.header(page, &mut lines);
            let shown = if mask {
                "•".repeat(value.chars().count())
            } else {
                value.clone()
            };
            lines.push(format!("{PAD}{label}"));
            lines.push(format!("{PAD}{ACCENT}▸{RESET} {shown}{ACCENT}▏{RESET}"));
            if let Some(e) = &error {
                lines.push(String::new());
                lines.push(format!("{PAD}{DANGER}{e}{RESET}"));
            }
            self.footer(
                "enter continue · ctrl-u clear · esc back · ctrl-c quit",
                &mut lines,
            );
            self.paint(&lines);

            match self.key()? {
                Key::Enter => match check(&value) {
                    Ok(()) => return Ok(value),
                    Err(e) => error = Some(e),
                },
                Key::Backspace => {
                    value.pop();
                    error = None;
                }
                Key::ClearLine => {
                    value.clear();
                    error = None;
                }
                Key::Char(c) => {
                    value.push(c);
                    error = None;
                }
                Key::Space => {
                    value.push(' ');
                    error = None;
                }
                Key::Resize => self.resized(),
                _ => {}
            }
        }
    }

    /// The one screen that cannot be answered by leaning on the keyboard: the
    /// user types the disk's own name. `sgdisk --zap-all` has no undo, and a
    /// y/n on that question is a coin flip somebody loses.
    pub fn typed_confirm(&mut self, page: &Page, word: &str) -> Answer<()> {
        let want = word.to_string();
        self.field(
            page,
            &format!("type {BOLD}{word}{RESET} to confirm"),
            "",
            false,
            move |s: &str| {
                if s == want {
                    Ok(())
                } else {
                    Err(format!("that is not `{want}`"))
                }
            },
        )?;
        Ok(())
    }

    /// The review screen: everything that was answered, and the last chance.
    pub fn review(&mut self, page: &Page, rows: &[(String, String)]) -> Answer<bool> {
        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        loop {
            let mut lines = Vec::new();
            self.header(page, &mut lines);
            for (k, v) in rows {
                lines.push(format!("{PAD}{DIM}{k:width$}{RESET}  {v}", width = width));
            }
            lines.push(String::new());
            lines.push(format!(
                "{PAD}{DANGER}▸{RESET} {BOLD}enter{RESET} begins the install. Nothing has been \
                 written yet."
            ));
            self.footer("enter install · esc back · ctrl-c quit", &mut lines);
            self.paint(&lines);
            match self.key()? {
                Key::Enter => return Ok(true),
                Key::Resize => self.resized(),
                _ => {}
            }
        }
    }

    /// A message with nothing to answer.
    pub fn pause(&mut self, page: &Page) -> Answer<()> {
        self.pause_with(page, "enter continue · esc back · ctrl-c quit")
    }

    /// The same, with its own key legend. The last screen of the install has no
    /// "esc back" to offer, and a footer that says otherwise is a footer people
    /// stop reading.
    pub fn pause_with(&mut self, page: &Page, help: &str) -> Answer<()> {
        loop {
            let mut lines = Vec::new();
            self.header(page, &mut lines);
            self.footer(help, &mut lines);
            self.paint(&lines);
            match self.key()? {
                Key::Enter => return Ok(()),
                Key::Resize => self.resized(),
                _ => {}
            }
        }
    }

    fn row(&self, opt: &Opt, selected: bool, width: usize) -> String {
        if opt.heading {
            return format!("{PAD}{DIM}{}{RESET}", opt.label);
        }
        let arrow = if selected {
            format!("{ACCENT}▸{RESET}")
        } else {
            " ".to_string()
        };
        let label = match (selected, opt.enabled) {
            (_, false) => format!("{DIM}{}{RESET}", opt.label),
            (true, _) => format!("{BOLD}{}{RESET}", opt.label),
            _ => opt.label.clone(),
        };
        let note = if opt.note.is_empty() {
            String::new()
        } else {
            // Padding goes after the styled label, computed from the label's
            // own length: the escape codes have no width, and `{:width$}` on a
            // styled string counts them.
            format!(
                "{:pad$}  {DIM}{}{RESET}",
                "",
                opt.note,
                pad = pad(&opt.label, width)
            )
        };
        format!("{PAD}{arrow} {label}{note}")
    }

    /// Draw the install itself: the step list with its verdicts, and a tail of
    /// what the running command is saying. `run.rs` calls this on every line.
    pub fn progress(&mut self, state: &Progress) {
        let mut lines = Vec::new();
        let page = Page::new((0, 0), "Installing");
        self.header(&page, &mut lines);
        for (i, step) in state.steps.iter().enumerate() {
            let (mark, style) = match i {
                _ if i < state.at => (format!("{OK}✔{RESET}"), DIM),
                _ if i == state.at && state.failed => (format!("{DANGER}✘{RESET}"), BOLD),
                _ if i == state.at => (format!("{ACCENT}▸{RESET}"), BOLD),
                _ => (" ".to_string(), DIM),
            };
            let detail = if i == state.at && !state.detail.is_empty() {
                format!("  {DIM}{}{RESET}", state.detail)
            } else {
                String::new()
            };
            lines.push(format!("{PAD}{mark} {style}{}{RESET}{detail}", step));
        }
        lines.push(String::new());
        let room = (self.rows as usize).saturating_sub(lines.len() + 3).max(1);
        for line in state.tail.iter().rev().take(room).rev() {
            let cut: String = line.chars().take(self.cols as usize - 6).collect();
            lines.push(format!("{PAD}{DIM}│ {cut}{RESET}"));
        }
        self.footer("ctrl-c abort", &mut lines);
        self.paint(&lines);
    }

    /// Block on a key, translating Ctrl-C and Esc into navigation.
    fn key(&mut self) -> Answer<Key> {
        loop {
            match event::read() {
                Ok(Event::Resize(..)) => return Ok(Key::Resize),
                Ok(Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                })) => {
                    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
                    return match code {
                        KeyCode::Char('c') if ctrl => Err(Nav::Quit),
                        KeyCode::Char('u') if ctrl => Ok(Key::ClearLine),
                        KeyCode::Char('p') if ctrl => Ok(Key::Up),
                        KeyCode::Char('n') if ctrl => Ok(Key::Down),
                        KeyCode::Char(_) if ctrl => continue,
                        KeyCode::Esc => Err(Nav::Back),
                        KeyCode::Up => Ok(Key::Up),
                        KeyCode::Down => Ok(Key::Down),
                        KeyCode::Enter => Ok(Key::Enter),
                        KeyCode::Backspace => Ok(Key::Backspace),
                        KeyCode::Char(' ') => Ok(Key::Space),
                        KeyCode::Char(c) => Ok(Key::Char(c)),
                        _ => continue,
                    };
                }
                Ok(_) => continue,
                // A closed stdin cannot be recovered from and must not spin.
                Err(_) => return Err(Nav::Quit),
            }
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        Ui::restore();
    }
}

/// Spaces needed after `label` to reach the notes column.
fn pad(label: &str, width: usize) -> usize {
    width.saturating_sub(label.chars().count())
}

/// The widest selectable label in a list, so notes line up in one column.
/// Headings are excluded: a namespace name is not in the same column as the
/// modules under it.
fn column(options: &[Opt]) -> usize {
    options
        .iter()
        .filter(|o| !o.heading)
        .map(|o| o.label.chars().count())
        .max()
        .unwrap_or(0)
}

enum Key {
    Up,
    Down,
    Enter,
    Backspace,
    ClearLine,
    Space,
    Char(char),
    Resize,
}

/// What the install screen is drawing. Owned by `steps.rs`, rendered here.
pub struct Progress {
    pub steps: Vec<String>,
    pub at: usize,
    pub failed: bool,
    pub detail: String,
    pub tail: VecDeque<String>,
}

impl Progress {
    /// Twelve lines of scrollback. Enough to see what a command is doing;
    /// short enough that the step list stays on screen, which is the part that
    /// says how far along the install is.
    const TAIL: usize = 12;

    pub fn new(steps: &[&str]) -> Progress {
        Progress {
            steps: steps.iter().map(|s| s.to_string()).collect(),
            at: 0,
            failed: false,
            detail: String::new(),
            tail: VecDeque::new(),
        }
    }

    pub fn say(&mut self, line: impl Into<String>) {
        if self.tail.len() == Self::TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(line.into());
    }
}
