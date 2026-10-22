//! The prompt editor `dla` opens while a workspace boots: a raw-mode line editor
//! that takes a pasted prompt whole, line breaks and all.
//!
//! It replaced a cooked-mode read, and the reasons are the ones that read had no
//! answer for. The kernel's line discipline holds at most 4096 bytes of one line,
//! so a long paste was cut off. A paste is only whole if it arrives in one piece,
//! so the lines that came a moment late were left in the terminal's queue and
//! reached the agent as keystrokes. A last line with no line break was never read
//! at all. And no line break could be typed, because Enter always submitted.
//!
//! **Bracketed paste is what tells a paste from typing.** The editor asks the
//! terminal for it (`ESC [ ? 2004 h`), and the terminal then wraps every paste in
//! `ESC [ 200 ~` and `ESC [ 201 ~`. A line break between those is text. Outside
//! them, Enter submits, and Alt-Enter or Ctrl-J adds a line. A terminal without
//! bracketed paste still sends a paste faster than anybody types, so an Enter with
//! more input right behind it is read as a line break too.
//!
//! **One byte at a time from descriptor 0**, as the cooked read did, and for the
//! same reason: whatever this does not read stays in the terminal's queue for the
//! agent session `dla` attaches next. Reading stops at the Enter that submits, so
//! keys typed after it are the agent's.
//!
//! The editor only ever appends. There is no cursor to move, so the arrow keys
//! and the other escape sequences are read and dropped rather than printed as
//! `^[[D`. Backspace, Ctrl-U and Ctrl-W edit the end of the text.
//!
//! [`Editor`] and [`render`] are pure, so the tests below read them without a
//! terminal. [`read_prompt`] is the thin loop around them.

use std::io::Write as _;
use std::time::Duration;

use unicode_width::UnicodeWidthStr as _;

/// What the person at the prompt did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// Enter: this text, with trailing whitespace trimmed. Empty is the agent's
    /// plain session.
    Text(String),
    /// Ctrl-C. In raw mode that is a key rather than a signal, so the caller gets
    /// it as an answer and ends the run itself.
    Cancelled,
}

/// What one byte did to the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Continue,
    Submit,
    Cancel,
}

/// Where the byte parser is inside an escape sequence or a UTF-8 character.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Parse {
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC [`, with the parameter bytes so far.
    Control(Vec<u8>),
    /// After `ESC O`: one more byte, then back to ground.
    Shift,
    /// Inside a UTF-8 character: the bytes so far, and how many it has in all.
    Character(Vec<u8>, usize),
}

/// The text being edited, and where the parser is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Editor {
    text: String,
    parse: Parse,
    /// Between `ESC [ 200 ~` and `ESC [ 201 ~`.
    pasting: bool,
    /// The last byte was a carriage return, so a line feed next is the second half
    /// of one line break rather than a second one.
    after_return: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            text: String::new(),
            parse: Parse::Ground,
            pasting: false,
            after_return: false,
        }
    }
}

impl Editor {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Take one byte. `more_follows` says whether more input was already waiting
    /// behind it, which is how an unbracketed paste's Enter is told from a typed
    /// one. It only matters for a carriage return.
    pub(crate) fn feed(&mut self, byte: u8, more_follows: bool) -> Step {
        let after_return = std::mem::replace(&mut self.after_return, false);
        match std::mem::replace(&mut self.parse, Parse::Ground) {
            Parse::Ground => self.ground(byte, more_follows, after_return),
            Parse::Escape => {
                match byte {
                    b'[' => self.parse = Parse::Control(Vec::new()),
                    b'O' => self.parse = Parse::Shift,
                    // Alt-Enter: a terminal sends ESC before the key.
                    b'\r' | b'\n' => self.text.push('\n'),
                    // Alt-Backspace: the word, as Ctrl-W.
                    0x7f | 0x08 => self.delete_word(),
                    // ESC ESC, or Alt with any other key: nothing to do.
                    _ => {}
                }
                Step::Continue
            }
            Parse::Control(mut parameters) => {
                if (0x40..=0x7e).contains(&byte) {
                    match (parameters.as_slice(), byte) {
                        (b"200", b'~') => self.pasting = true,
                        (b"201", b'~') => self.pasting = false,
                        // Arrows, Home, End, function keys: there is no cursor to
                        // move, so they are dropped.
                        _ => {}
                    }
                } else if parameters.len() < 16 {
                    parameters.push(byte);
                    self.parse = Parse::Control(parameters);
                }
                Step::Continue
            }
            Parse::Shift => Step::Continue,
            Parse::Character(mut bytes, length) => {
                if byte & 0xc0 == 0x80 {
                    bytes.push(byte);
                    if bytes.len() == length {
                        self.text.push_str(&String::from_utf8_lossy(&bytes));
                    } else {
                        self.parse = Parse::Character(bytes, length);
                    }
                    Step::Continue
                } else {
                    // A broken character: say so once, and read this byte afresh.
                    self.text.push(char::REPLACEMENT_CHARACTER);
                    self.ground(byte, more_follows, after_return)
                }
            }
        }
    }

    /// Nothing followed an `ESC` within the gap a terminal leaves inside one
    /// sequence, so it was the Esc key on its own. It is dropped, and the next
    /// byte is read as a key of its own rather than as Alt with it.
    pub(crate) fn escape_timed_out(&mut self) {
        if self.parse == Parse::Escape {
            self.parse = Parse::Ground;
        }
    }

    fn ground(&mut self, byte: u8, more_follows: bool, after_return: bool) -> Step {
        match byte {
            0x1b => self.parse = Parse::Escape,
            b'\r' => {
                self.after_return = true;
                if self.pasting || more_follows {
                    self.text.push('\n');
                } else {
                    return Step::Submit;
                }
            }
            // The second half of a CR LF, or a line break on its own: in a paste,
            // or Ctrl-J typed.
            b'\n' => {
                if !after_return {
                    self.text.push('\n');
                }
            }
            b'\t' => self.text.push('\t'),
            _ if self.pasting && byte < 0x20 => {}
            0x03 => return Step::Cancel,
            0x04 if self.text.is_empty() => return Step::Submit,
            0x7f | 0x08 => {
                self.text.pop();
            }
            0x15 => {
                let start = self.text.rfind('\n').map_or(0, |at| at + 1);
                self.text.truncate(start);
            }
            0x17 => self.delete_word(),
            0x00..=0x1f => {}
            0x20..=0x7e => self.text.push(char::from(byte)),
            0xc0..=0xdf => self.parse = Parse::Character(vec![byte], 2),
            0xe0..=0xef => self.parse = Parse::Character(vec![byte], 3),
            0xf0..=0xf7 => self.parse = Parse::Character(vec![byte], 4),
            _ => self.text.push(char::REPLACEMENT_CHARACTER),
        }
        Step::Continue
    }

    /// The last word on the current line and the spaces after it, as a shell's
    /// Ctrl-W does. Never crosses a line break.
    fn delete_word(&mut self) {
        let start = self.text.rfind('\n').map_or(0, |at| at + 1);
        let line = &self.text[start..];
        let kept = line.trim_end_matches([' ', '\t']);
        let kept = kept.trim_end_matches(|c: char| c != ' ' && c != '\t');
        let cut = start + kept.len();
        self.text.truncate(cut);
    }
}

/// The prefix of the first line, and of every line after it.
const PROMPT: &str = "> ";
const MORE: &str = "  ";

/// One drawing of the editor: the text to write, and how many rows above the
/// last one it takes, which is how far up the next drawing has to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) text: String,
    pub(crate) rows_above: usize,
}

/// Draw `text` for a terminal `columns` wide with `height` rows to spare.
///
/// **Never taller than the space it has.** The next drawing starts by moving the
/// cursor up over this one, and a cursor cannot move above the top of the screen,
/// so a drawing that scrolled would be redrawn in the wrong place, line after
/// line. A prompt too tall for the screen shows its last lines under a line that
/// counts the ones not shown. The text is all still there, and all of it is sent.
///
/// A tab is drawn as four spaces and sent as a tab.
pub(crate) fn render(text: &str, columns: usize, height: usize) -> Frame {
    let columns = columns.max(1);
    let budget = height.max(1);
    let lines: Vec<String> = text
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { PROMPT } else { MORE };
            format!("{prefix}{}", line.replace('\t', "    "))
        })
        .collect();
    let rows_of = |line: &str| line.width().div_ceil(columns).max(1);

    let mut shown: Vec<String> = Vec::new();
    let mut used = 0;
    let mut hidden = lines.len();
    for line in lines.iter().rev() {
        // One row is held back for the count, unless this is the first line and
        // nothing would be hidden.
        let room = if hidden == 1 { budget } else { budget - 1 };
        let rows = rows_of(line);
        if used + rows <= room {
            shown.push(line.clone());
            used += rows;
            hidden -= 1;
            continue;
        }
        if shown.is_empty() {
            // One line taller than the screen: its end, which is where typing is.
            let keep = (room.max(1) * columns).saturating_sub(2).max(1);
            shown.push(format!("{MORE}{}", tail(line, keep)));
            used += rows_of(&shown[0]);
            hidden -= 1;
        }
        break;
    }
    shown.reverse();
    if hidden > 0 {
        shown.insert(0, count_line(hidden, columns));
        used += 1;
    }
    Frame {
        text: shown.join("\r\n"),
        rows_above: used - 1,
    }
}

/// The line that counts `hidden` lines, in the longest wording that fits one row
/// of `columns`: it is held to one row, because `render` reserves one row for it.
fn count_line(hidden: usize, columns: usize) -> String {
    let word = if hidden == 1 { "line" } else { "lines" };
    let wordings = [
        format!("{MORE}({hidden} earlier {word} not shown)"),
        format!("{MORE}({hidden} not shown)"),
        format!("{MORE}(+{hidden})"),
        format!("(+{hidden})"),
    ];
    match wordings.iter().find(|line| line.width() <= columns) {
        Some(line) => line.clone(),
        None => wordings[3].chars().take(columns).collect(),
    }
}

/// The end of `line` that fits in `width` columns.
fn tail(line: &str, width: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in line.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    kept.into_iter().rev().collect()
}

/// The terminal's settings as they were, put back when this is dropped.
///
/// A guard rather than a call at the end, so an early return or a panic still
/// leaves the terminal the way the person's shell expects it. `TCSANOW` rather
/// than `TCSAFLUSH` on the way out: flushing would throw away keys typed after
/// the Enter, which belong to the agent.
struct Raw {
    before: libc::termios,
}

impl Raw {
    /// Raw input on descriptor 0, or `None` if its settings cannot be read.
    ///
    /// Output keeps its processing (`OPOST`), so anything else that writes to the
    /// terminal meanwhile still gets its carriage returns. `ISIG` is off, so
    /// Ctrl-C arrives as a byte and the editor restores the terminal before the
    /// run ends.
    fn enter() -> Option<Self> {
        // SAFETY: `tcgetattr` fills one `termios` this scope owns.
        let mut before: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut before) } != 0 {
            return None;
        }
        let mut raw = before;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        raw.c_iflag &= !(libc::ICRNL | libc::INLCR | libc::IXON);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: as above, setting what was read and changed.
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Raw { before })
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        write_out(BRACKETED_PASTE_OFF);
        // SAFETY: restoring the settings `enter` read.
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.before);
        }
    }
}

const BRACKETED_PASTE_ON: &str = "\x1b[?2004h";
const BRACKETED_PASTE_OFF: &str = "\x1b[?2004l";

/// How long an Enter waits to see whether more input follows it. Far under the
/// gap between two keys a person types, far over the gap inside one paste.
const PASTE_GAP: Duration = Duration::from_millis(15);

/// Read one prompt from the terminal. See the module's documentation.
///
/// Falls back to the cooked read it replaced when descriptor 0's settings cannot
/// be changed, which is nothing a person at a terminal meets.
pub fn read_prompt() -> Submission {
    let Some(raw) = Raw::enter() else {
        return Submission::Text(read_cooked());
    };
    write_out(BRACKETED_PASTE_ON);
    let mut editor = Editor::default();
    let mut rows_above = draw(&editor, 0);
    let ended = loop {
        let Some(byte) = read_stdin_byte() else {
            break Step::Submit;
        };
        let more_follows = byte == b'\r' && stdin_readable_within(PASTE_GAP);
        let step = editor.feed(byte, more_follows);
        if byte == 0x1b && !stdin_readable_within(PASTE_GAP) {
            editor.escape_timed_out();
        }
        match step {
            // Drawn once the input so far is read, so a paste of a thousand lines
            // is one drawing and not a thousand.
            Step::Continue if !stdin_readable_within(Duration::ZERO) => {
                rows_above = draw(&editor, rows_above);
            }
            Step::Continue => {}
            ended => break ended,
        }
    };
    draw(&editor, rows_above);
    write_out("\r\n");
    drop(raw);
    match ended {
        Step::Cancel => Submission::Cancelled,
        _ => Submission::Text(editor.text().trim_end().to_owned()),
    }
}

/// Draw over the last drawing, which ended `rows_above` rows below its top, and
/// say how far this one reaches.
fn draw(editor: &Editor, rows_above: usize) -> usize {
    let (height, columns) = terminal_size().unwrap_or((24, 80));
    // Two rows kept free: one for the banner line above, one for the terminal's
    // own last row.
    let frame = render(
        editor.text(),
        usize::from(columns),
        usize::from(height).saturating_sub(2),
    );
    let mut out = String::from("\r");
    if rows_above > 0 {
        out.push_str(&format!("\x1b[{rows_above}A"));
    }
    out.push_str("\x1b[J");
    out.push_str(&frame.text);
    write_out(&out);
    frame.rows_above
}

fn write_out(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Rows and columns of the terminal on descriptor 1, which is what is drawn on.
fn terminal_size() -> Option<(u16, u16)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ fills one `winsize` this scope owns.
    let read = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) };
    (read == 0 && size.ws_row > 0 && size.ws_col > 0).then_some((size.ws_row, size.ws_col))
}

/// The cooked-mode read this module replaced, kept as its fallback: the line up
/// to Enter, plus whatever complete lines were already queued behind it.
fn read_cooked() -> String {
    let mut bytes: Vec<u8> = Vec::new();
    let mut ended_with_newline = false;
    loop {
        match read_stdin_byte() {
            None => break,
            Some(b'\n') => {
                ended_with_newline = true;
                break;
            }
            Some(byte) => bytes.push(byte),
        }
    }
    if ended_with_newline && stdin_readable_within(Duration::ZERO) {
        bytes.push(b'\n');
        while stdin_readable_within(Duration::ZERO) {
            match read_stdin_byte() {
                None => break,
                Some(byte) => bytes.push(byte),
            }
        }
    }
    String::from_utf8_lossy(&bytes).trim_end().to_owned()
}

/// One byte from descriptor 0, or `None` on EOF or an unreadable stdin.
fn read_stdin_byte() -> Option<u8> {
    let mut byte: u8 = 0;
    loop {
        // SAFETY: reading one byte into a stack buffer of that size.
        let read = unsafe { libc::read(0, std::ptr::from_mut(&mut byte).cast(), 1) };
        match read {
            1 => return Some(byte),
            0 => return None,
            // A signal that did not kill the process (SIGWINCH, a stopped and
            // resumed job) interrupts the read without ending the input.
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
            _ => return None,
        }
    }
}

/// Whether descriptor 0 has bytes to read within `wait`.
fn stdin_readable_within(wait: Duration) -> bool {
    let mut asked = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: polling one descriptor; the struct outlives the call.
    let ready = unsafe { libc::poll(&mut asked, 1, timeout) };
    ready > 0 && (asked.revents & libc::POLLIN) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `bytes`, each with nothing following it, and say how it ended.
    fn typed(bytes: &[u8]) -> (Editor, Step) {
        let mut editor = Editor::default();
        let mut last = Step::Continue;
        for byte in bytes {
            last = editor.feed(*byte, false);
            if last != Step::Continue {
                break;
            }
        }
        (editor, last)
    }

    /// Feed `bytes` as one burst: every byte but the last has more behind it.
    fn burst(bytes: &[u8]) -> (Editor, Step) {
        let mut editor = Editor::default();
        let mut last = Step::Continue;
        for (at, byte) in bytes.iter().enumerate() {
            last = editor.feed(*byte, at + 1 < bytes.len());
            if last != Step::Continue {
                break;
            }
        }
        (editor, last)
    }

    #[test]
    fn enter_submits_what_was_typed() {
        let (editor, ended) = typed(b"fix the bug\r");
        assert_eq!(ended, Step::Submit);
        assert_eq!(editor.text(), "fix the bug");
    }

    #[test]
    fn a_bracketed_paste_keeps_its_line_breaks_and_does_not_submit() {
        let (editor, ended) = typed(b"\x1b[200~line one\r\nline two\r\n\x1b[201~");
        assert_eq!(ended, Step::Continue);
        assert_eq!(editor.text(), "line one\nline two\n");
        let (editor, ended) = typed(b"\x1b[200~a\rb\nc\x1b[201~\r");
        assert_eq!(ended, Step::Submit);
        assert_eq!(editor.text(), "a\nb\nc");
    }

    #[test]
    fn a_paste_longer_than_the_kernels_line_limit_arrives_whole() {
        let long = "x".repeat(10_000);
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(long.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~\r");
        let (editor, ended) = typed(&bytes);
        assert_eq!(ended, Step::Submit);
        assert_eq!(editor.text(), long);
    }

    #[test]
    fn an_unbracketed_paste_is_told_apart_by_the_input_behind_each_enter() {
        // A terminal without bracketed paste: the Enters inside the paste have more
        // input right behind them, and the last one has none.
        let (editor, ended) = burst(b"one\rtwo\r");
        assert_eq!(ended, Step::Submit);
        assert_eq!(editor.text(), "one\ntwo");
    }

    #[test]
    fn alt_enter_and_ctrl_j_add_a_line_when_typing() {
        let (editor, ended) = typed(b"first\x1b\rsecond\nthird\r");
        assert_eq!(ended, Step::Submit);
        assert_eq!(editor.text(), "first\nsecond\nthird");
    }

    #[test]
    fn a_lone_esc_is_dropped_and_the_next_key_is_read_afresh() {
        let mut editor = Editor::default();
        assert_eq!(editor.feed(0x1b, false), Step::Continue);
        editor.escape_timed_out();
        assert_eq!(editor.feed(b'f', false), Step::Continue);
        assert_eq!(editor.feed(b'\r', false), Step::Submit);
        assert_eq!(editor.text(), "f");

        let mut editor = Editor::default();
        editor.feed(b'x', false);
        editor.feed(0x1b, false);
        editor.escape_timed_out();
        assert_eq!(editor.feed(b'\r', false), Step::Submit);
        assert_eq!(editor.text(), "x");

        // The timeout only ends a bare ESC: Alt-Enter in one burst still adds a
        // line.
        let (editor, ended) = burst(b"a\x1b\rb");
        assert_eq!(ended, Step::Continue);
        assert_eq!(editor.text(), "a\nb");
    }

    #[test]
    fn the_editing_keys_edit_the_end_of_the_text() {
        assert_eq!(typed(b"abc\x7f").0.text(), "ab");
        assert_eq!(typed(b"one\ntwo three\x15").0.text(), "one\n");
        assert_eq!(typed(b"one two  \x17").0.text(), "one ");
        assert_eq!(typed(b"one\ntwo\x17\x17").0.text(), "one\n");
        // Backspace at the start of a line joins it to the line above.
        assert_eq!(typed(b"one\n\x7f").0.text(), "one");
    }

    #[test]
    fn arrow_and_other_keys_are_dropped_rather_than_printed() {
        let (editor, _) = typed(b"a\x1b[D\x1b[1;5C\x1bOA\x1b[3~b");
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn ctrl_c_cancels_and_ctrl_d_on_nothing_submits_nothing() {
        assert_eq!(typed(b"half a prompt\x03").1, Step::Cancel);
        let (editor, ended) = typed(b"\x04");
        assert_eq!((editor.text(), ended), ("", Step::Submit));
        // With text there, Ctrl-D is not an answer.
        assert_eq!(typed(b"text\x04").1, Step::Continue);
    }

    #[test]
    fn a_ctrl_c_inside_a_paste_is_text_not_a_cancel() {
        let (editor, ended) = typed(b"\x1b[200~a\x03b\x1b[201~");
        assert_eq!(ended, Step::Continue);
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn utf8_is_read_whole_and_a_broken_character_is_marked() {
        assert_eq!(typed("héllo ✓ 日本".as_bytes()).0.text(), "héllo ✓ 日本");
        assert_eq!(typed(b"a\xc3b").0.text(), "a\u{fffd}b");
    }

    #[test]
    fn a_drawing_counts_its_rows_so_the_next_can_start_over_it() {
        let frame = render("one\ntwo", 80, 20);
        assert_eq!(frame.text, "> one\r\n  two");
        assert_eq!(frame.rows_above, 1);
        // A line wider than the terminal takes the rows it wraps onto.
        assert_eq!(render(&"x".repeat(100), 80, 20).rows_above, 1);
        // Wide characters take two columns each.
        assert_eq!(render(&"日".repeat(40), 80, 20).rows_above, 1);
        assert_eq!(
            render("", 80, 20),
            Frame {
                text: "> ".to_owned(),
                rows_above: 0
            }
        );
    }

    #[test]
    fn a_prompt_taller_than_the_screen_shows_its_end_under_a_count() {
        let text: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let frame = render(&text.join("\n"), 80, 5);
        assert_eq!(
            frame.text,
            "  (26 earlier lines not shown)\r\n  line 27\r\n  line 28\r\n  line 29\r\n  line 30"
        );
        assert_eq!(frame.rows_above, 4);
    }

    #[test]
    fn the_count_line_is_counted_at_the_rows_it_really_takes() {
        let text: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let text = text.join("\n");
        for (columns, height) in [(20, 5), (5, 3)] {
            let frame = render(&text, columns, height);
            let rows: usize = frame
                .text
                .split("\r\n")
                .map(|line| line.width().div_ceil(columns).max(1))
                .sum();
            assert_eq!(rows, frame.rows_above + 1, "{columns}x{height}: {frame:?}");
            assert!(rows <= height, "{columns}x{height}: {frame:?}");
        }
    }

    #[test]
    fn one_line_taller_than_the_screen_shows_its_end() {
        let frame = render(&"x".repeat(1000), 10, 3);
        assert!(frame.rows_above < 3, "{frame:?}");
        assert!(frame.text.ends_with("xxxx"), "{frame:?}");
    }
}
