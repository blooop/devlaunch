//! The picker, judged on a terminal rather than on its options.
//!
//! `select.rs`'s unit tests can reach everything about the picker except the one
//! thing a user sees: they assert the [`SkimOptions`] `dl` asks for, and an option
//! that is spelled right and does nothing passes every one of them. That is not a
//! hypothetical failure mode here — skim carries a `reverse: bool` next to
//! `layout`, documented as shorthand for exactly the layout this file is about, and
//! it is expanded by a `build()` that `Skim::run_with` never calls. Setting it
//! compiles, reads as the fix, and draws the old picture.
//!
//! So this file opens a pty, runs the real binary on it, and reads back the screen.
//! The assertions are the user's own sentences — the search bar is above the
//! matches; the line that explains the rows is on the screen while the rows are —
//! rather than the names of fields.
//!
//! The same reasoning admits the two tests that are not about the layout: a pick has
//! to travel *out* through skim as well, as the label bytes `select::chosen` then
//! matches against, and a label skim does not hand back verbatim reaches no
//! workspace at all. That is the same class of failure — everything on dl's side
//! spelled right, and the picker doing nothing — and it is equally invisible to a
//! unit test, which feeds `chosen` the very strings it built. TAB is the same seam
//! again and worse: skim keys a marked row by an index the row has to carry itself,
//! so a batch is one place where marking three rows and acting on one is a defect no
//! unit test can reach.
//!
//! The cost is that these are the only tests in the crate that need a terminal and
//! a spawned process, which is why there are six of them and not a suite: what
//! the picker *offers* and the order it offers it in are pure functions tested in
//! `select.rs`, and neither is re-tested here.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// The terminal the picker is given. Fixed, because the assertions are about rows.
const ROWS: u16 = 20;
const COLS: u16 = 100;

/// How long to wait for the picker to finish drawing before giving up on it.
///
/// Generous: this waits on a process spawn and a debug-profile binary's startup,
/// and the loop below stops as soon as the screen has what it is waiting for, so
/// the number costs nothing when things are working.
const DEADLINE: Duration = Duration::from_secs(30);

/// The three workspaces the fake devpod lists, in the order it lists them.
const LISTED: [&str; 3] = ["blooop-devlaunch", "blooop-wayfinder", "myproject"];

#[test]
fn the_search_bar_is_drawn_above_the_matches() {
    // The whole feature, said as the user would say it. Both arities, because the
    // layout is not the multi-select's business: the picker `dl stop` opens is
    // drawn exactly like the one `dl -- <cmd>` opens.
    for verb in [vec!["stop"], vec!["--", "true"]] {
        let spelled = verb.join(" ");
        let screen = Screen::of(&verb);
        let prompt = screen.prompt_row(&spelled);

        for workspace in LISTED {
            let row = screen.row_of(workspace).unwrap_or_else(|| {
                panic!("`{workspace}` was never drawn by `dl {spelled}`:\n{screen}")
            });
            assert!(
                prompt < row,
                "the search bar must be above the matches, but `dl {spelled}` drew the prompt on \
                 row {} and `{workspace}` on row {}:\n{screen}",
                prompt + 1,
                row + 1,
            );
        }
    }
}

#[test]
fn the_search_bar_is_the_first_line_and_the_matches_read_downward_from_it() {
    // The same claim, tightened to the exact shape: prompt on line one, and the
    // matches in devpod's order reading *downward* — not merely above the list, and
    // not the list reversed under it, which is what skim's other non-default layout
    // (`reverse-list`) would have drawn.
    let screen = Screen::of(&["stop"]);

    assert_eq!(
        screen.prompt_row("stop"),
        0,
        "the prompt belongs on the first line:\n{screen}"
    );
    let rows: Vec<usize> = LISTED
        .iter()
        .map(|workspace| {
            screen
                .row_of(workspace)
                .unwrap_or_else(|| panic!("`{workspace}` was never drawn:\n{screen}"))
        })
        .collect();
    let mut downward = rows.clone();
    downward.sort_unstable();
    assert_eq!(
        rows, downward,
        "the matches must read downward in devpod's order:\n{screen}"
    );
}

#[test]
fn what_is_typed_appears_on_the_top_line_and_narrows_the_list_below_it() {
    // The row above the matches is the *search bar* and not a decoration: typing
    // puts the query on it, and the list underneath narrows to what matches. This
    // is what rules out the reading the two tests above cannot — that skim happens
    // to draw a `>` on line one while the real input line is somewhere else.
    let screen = Screen::after(&["stop"], "wayfinder", |screen| {
        screen.row_of("blooop-devlaunch").is_none()
    });

    assert_eq!(
        screen.rows[0], "> wayfinder",
        "the query belongs on the top line:\n{screen}"
    );
    let matched = screen
        .row_of("blooop-wayfinder")
        .unwrap_or_else(|| panic!("the one match should still be drawn:\n{screen}"));
    assert!(
        0 < matched,
        "the match belongs below the search bar:\n{screen}"
    );
    assert_eq!(
        screen.row_of("myproject"),
        None,
        "`myproject` does not match `wayfinder` and should have gone:\n{screen}"
    );
}

#[test]
fn the_invitation_is_on_the_picker_s_screen_and_never_on_the_one_it_covers() {
    // The line that says what the rows are, judged where it has to be read: on the
    // picker's screen, while the picker is up. `dl` printed it to stdout just
    // before skim started, and skim's first act is to switch to the alternate
    // screen — which replaces the visible screen wholesale — so the sentence was
    // gone for the whole time it had a job to do and came back only once the picker
    // had exited. No test of the options can see that, and the source cannot show
    // it either: the `println!` is there, it is spelled right, and it is unreadable.
    //
    // For `Arity::Several` this line is the whole of TAB's documentation. `dl rm`,
    // `stop`, `up`, `code` and `dotfiles` all take any number of marked rows, and a
    // user who never learns that marks one row at a time forever.
    //
    // Both halves are asserted, and the second is the one with teeth. Drawing the
    // header while *also* keeping the old print would satisfy the first half
    // completely — the sentence would be on the picker, the picker would look
    // right, and every other test in this crate would stay green, because a grid
    // that models `?1049` by blanking itself structurally cannot see a line written
    // before the switch. That is the shape of the defect being fixed, so it is the
    // shape a regression would take: not a missing header, but a print that came
    // back. `underneath` is the only place that can be seen, so the claim is made
    // there — the invitation is written on the screen that survives the switch, and
    // on no other.
    for (verb, invitation) in [
        (
            vec!["stop"],
            "Select workspaces (type to filter, TAB to mark several):",
        ),
        (vec!["--", "true"], "Select workspace (type to filter):"),
    ] {
        let spelled = verb.join(" ");
        let screen = Screen::of(&verb);

        let row = screen.row_of(invitation).unwrap_or_else(|| {
            panic!("`dl {spelled}` never showed `{invitation}` on the picker:\n{screen}")
        });
        // And where it explains something: under the search bar, above the rows it
        // is describing.
        let prompt = screen.prompt_row(&spelled);
        assert!(
            prompt < row,
            "the invitation belongs below the search bar, not above it:\n{screen}"
        );
        for workspace in LISTED {
            let match_row = screen
                .row_of(workspace)
                .unwrap_or_else(|| panic!("`{workspace}` was never drawn:\n{screen}"));
            assert!(
                row < match_row,
                "the invitation belongs above the rows it describes, but `{workspace}` is on \
                 row {} and the invitation on row {}:\n{screen}",
                match_row + 1,
                row + 1,
            );
        }

        // Said once, and on the readable screen. A run with a terminal has a header
        // to carry the sentence, so stdout must not carry it too: that line would
        // land on the screen the picker is about to cover, be unreadable for as long
        // as it had anything to say, and then sit in the scrollback afterwards
        // describing a choice already made.
        assert!(
            !screen.underneath.contains(invitation),
            "`dl {spelled}` wrote the invitation onto the screen the picker covers, where it \
             cannot be read — it belongs in the picker's header and nowhere else. What was \
             sent before the switch: {:?}",
            screen.underneath,
        );
    }
}

#[test]
fn taking_a_row_acts_on_the_workspace_that_rows_label_names() {
    // The seam every other test here stops short of: skim hands a *label* back,
    // and `select::chosen` finds the workspace by matching that label against the
    // ones it offered. Both sides are dl's, but the bytes travel through skim in
    // between — so a label skim does not return verbatim maps back to nothing, and
    // the picker silently does nothing at all. The unit tests in `select.rs` cannot
    // see this: they feed `chosen` the very strings they built.
    //
    // Worth a terminal since the owner column arrived, because the label is no
    // longer a bare id: it carries a separator and a run of padding spaces, and
    // anything a later change adds to it — colour especially, which would put
    // escape sequences in `SkimItem::text` — travels this same path.
    let (screen, calls, said) = Screen::run(
        &["stop"],
        "wayfinder",
        |screen| screen.row_of("blooop-devlaunch").is_none(),
        Dismiss::Take,
    );

    assert!(
        calls.iter().any(|call| call == "stop blooop-wayfinder"),
        "the pick should have stopped `blooop-wayfinder`, but devpod was asked \
         {calls:?}, from this screen:\n{screen}"
    );
    // And the pick says which row it took, on the screen the picker gave back. The
    // row is what was on screen while it was being chosen; the id is what every line
    // after this names, and an id carries no owner, so neither half stands in for the
    // other.
    assert!(
        said.contains("Picked blooop | blooop-wayfinder -> blooop-wayfinder"),
        "a pick that named nothing: {said:?}"
    );
}

/// The batch. `dl rm` is the verb TAB exists for, and the heading is the only thing
/// in the run that says how many rows it took — devpod's own lines arrive one at a
/// time and say nothing about the extent of what was asked for.
///
/// On a terminal rather than in `select.rs` for the reason the pick above is: TAB
/// travels through skim, whose multi-select keys marked rows by an index the rows
/// have to carry themselves, and every unit test here feeds `chosen` the strings it
/// built rather than the ones skim handed back.
#[test]
fn a_batch_of_marked_rows_is_named_and_counted_before_the_first_one_goes() {
    // TAB marks the row under the cursor and steps down, so two of them take the
    // first two rows in devpod's order. Enter then takes the marked set rather than
    // the row the cursor ended on, which is the whole difference between a batch and
    // a pick.
    let (screen, calls, said) = Screen::run(&["rm"], "\t\t", |_| true, Dismiss::Take);

    // Asserted as one block rather than line by line, because the order is part of
    // the claim: the rows are listed in the order they were marked, which is the
    // order they are then acted on. `contains` and not equality because skim hands
    // the terminal back with a title sequence of its own, and it lands on the front
    // of the first line dl writes afterwards.
    assert!(
        said.contains(
            "Picked 2 workspaces for rm:\r\n  blooop | blooop-devlaunch -> \
             blooop-devlaunch\r\n  blooop | blooop-wayfinder -> blooop-wayfinder\r\n"
        ),
        "the batch was not named: {said:?}\nfrom this screen:\n{screen}"
    );
    // Both were removed, in the order they were marked, and each one said so. A
    // heading over a batch that then acted on one row would be the worse failure of
    // the two, which is why the calls are asserted beside the words.
    for workspace in ["blooop-devlaunch", "blooop-wayfinder"] {
        assert!(
            calls
                .iter()
                .any(|call| *call == format!("delete {workspace}")),
            "`{workspace}` was marked and never deleted; devpod was asked {calls:?}"
        );
        assert!(
            said.contains(&format!("Removed workspace {workspace}.")),
            "`{workspace}` was deleted without saying so: {said:?}"
        );
    }
    // And the third row, which was never marked, was left alone.
    assert!(
        !calls.iter().any(|call| call == "delete myproject"),
        "an unmarked workspace was deleted: {calls:?}"
    );
}

/// How the picker is left once the screen has been taken.
#[derive(Clone, Copy)]
enum Dismiss {
    /// Esc: the picker's quit, and the shortest way to let the child go.
    Quit,
    /// Enter: the row under the cursor is taken, and `dl` goes on to act on it.
    Take,
}

impl Dismiss {
    fn key(self) -> &'static [u8] {
        match self {
            Dismiss::Quit => b"\x1b",
            Dismiss::Take => b"\r",
        }
    }
}

/// What the picker had drawn on its terminal when it settled — and what had been
/// written on the screen it covered up to draw it.
struct Screen {
    rows: Vec<String>,
    /// Everything the terminal received *before* the picker switched to the
    /// alternate screen, decoded but not laid out.
    ///
    /// The screen underneath, in other words: bytes that were on the terminal and
    /// then were not, for as long as the picker was up. `rows` cannot hold them —
    /// modelling `?1049` means blanking the grid — so a claim about what `dl` does
    /// *not* write there has nowhere else to be made.
    underneath: String,
}

impl Screen {
    /// Run `dl <args>` on a pty against three fake workspaces, and take the screen.
    ///
    /// The snapshot is taken *before* the picker is dismissed, which is the only
    /// ordering that works: quitting sends `dl` on to print the help over the very
    /// screen this is about.
    fn of(args: &[&str]) -> Self {
        Self::after(args, "", |_| true)
    }

    /// The same, after typing `keys` — taken once `settled` says the screen has
    /// caught up with them.
    fn after(args: &[&str], keys: &str, settled: impl Fn(&Screen) -> bool) -> Self {
        Self::run(args, keys, settled, Dismiss::Quit).0
    }

    /// The screen, every call the fake devpod took before the child was gone, and
    /// what `dl` wrote once the picker had given the screen back.
    ///
    /// `dismiss` is how the picker is left: quit, or the pick taken. The calls and
    /// the trailing output are read after the child has exited, which is what makes
    /// the ones a pick causes visible — the screen is snapshotted before, since
    /// dismissing draws over it.
    fn run(
        args: &[&str],
        keys: &str,
        settled: impl Fn(&Screen) -> bool,
        dismiss: Dismiss,
    ) -> (Self, Vec<String>, String) {
        let world = World::new();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pty");

        let mut child = pair
            .slave
            .spawn_command(world.command(args))
            .expect("the dl binary runs");
        // The slave is the child's now: held open here, the reader below would never
        // see EOF.
        drop(pair.slave);

        let drawn = Arc::new(Mutex::new(Vec::new()));
        let mut reader = pair.master.try_clone_reader().expect("a pty reader");
        let collecting = Arc::clone(&drawn);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                collecting
                    .lock()
                    .expect("the collected bytes")
                    .extend_from_slice(&buffer[..read]);
            }
        });

        // Drawn when every workspace is on the screen: polled rather than slept on,
        // so a fast machine does not wait and a slow one is not cut off. A screen
        // that never arrives is returned rather than panicked over — the assertions
        // print it, and an empty grid says more there than a timeout does here.
        let waiting = |until: &dyn Fn(&Screen) -> bool| {
            let deadline = Instant::now() + DEADLINE;
            loop {
                let screen = Self::rendered(&drawn.lock().expect("the collected bytes").clone());
                if until(&screen) || Instant::now() >= deadline {
                    break screen;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        };

        let mut writer = pair.master.take_writer().expect("a pty writer");
        let mut screen = waiting(&|screen: &Screen| {
            LISTED
                .iter()
                .all(|workspace| screen.row_of(workspace).is_some())
        });
        if !keys.is_empty() {
            let _ = writer.write_all(keys.as_bytes());
            let _ = writer.flush();
            screen = waiting(&settled);
        }

        let _ = writer.write_all(dismiss.key());
        let _ = writer.flush();
        let gone = Instant::now() + DEADLINE;
        while Instant::now() < gone {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = child.kill();

        let afterwards = Self::afterwards(&drawn.lock().expect("the collected bytes").clone());
        (screen, world.devpod_calls(), afterwards)
    }

    /// The bytes a terminal received, as the grid it would be showing.
    ///
    /// Enough of a terminal for the question being asked and no more: absolute
    /// cursor moves, the two erases skim uses, and the alternate-screen switch it
    /// draws the picker on. Colour and cursor-visibility sequences are consumed and
    /// dropped, which is what keeps the assertions about rows instead of bytes.
    fn rendered(raw: &[u8]) -> Self {
        let (rows, cols) = (ROWS as usize, COLS as usize);
        let mut grid = vec![vec![' '; cols]; rows];
        let (mut row, mut col) = (0usize, 0usize);
        let mut at = 0usize;
        while at < raw.len() {
            let byte = raw[at];
            if byte == 0x1b && raw.get(at + 1) == Some(&b'[') {
                let mut end = at + 2;
                while end < raw.len() && !(0x40..=0x7e).contains(&raw[end]) {
                    end += 1;
                }
                if end >= raw.len() {
                    break;
                }
                let params = String::from_utf8_lossy(&raw[at + 2..end]).to_string();
                match raw[end] {
                    // Absolute move. Terminals count from 1; this grid from 0.
                    b'H' => {
                        let mut numbers = params
                            .split(';')
                            .map(|number| number.parse::<usize>().unwrap_or(1));
                        row = numbers.next().unwrap_or(1).saturating_sub(1);
                        col = numbers.next().unwrap_or(1).saturating_sub(1);
                    }
                    // Erase to the end of the screen, from the cursor.
                    b'J' => {
                        if row < rows {
                            for cell in grid[row].iter_mut().skip(col) {
                                *cell = ' ';
                            }
                        }
                        for line in grid.iter_mut().skip(row + 1) {
                            line.fill(' ');
                        }
                    }
                    // Erase to the end of the line, from the cursor.
                    b'K' => {
                        if row < rows {
                            for cell in grid[row].iter_mut().skip(col) {
                                *cell = ' ';
                            }
                        }
                    }
                    // Into the alternate screen: the picker's own, and a blank one.
                    // Modelled rather than ignored because it is the boundary every
                    // assertion in this file is on one side or the other of. What
                    // follows it is the picker's own screen, which is the only screen
                    // a user looking at the picker can see, so that is what `rows`
                    // holds. What precedes it is the screen the picker covers, kept
                    // whole in `underneath` rather than laid out, because the claim
                    // made about it is a negative one: `dl` writes the invitation on
                    // the screen that survives the switch and never on the one that
                    // does not. Blanking the grid here is what makes the two halves
                    // separable — and is exactly why `underneath` has to exist
                    // alongside it rather than be read back off these rows.
                    b'h' if params == "?1049" => {
                        for line in grid.iter_mut() {
                            line.fill(' ');
                        }
                        row = 0;
                        col = 0;
                    }
                    _ => {}
                }
                at = end + 1;
                continue;
            }
            // `ESC ( B` and friends select a character set: three bytes, not two.
            if byte == 0x1b {
                at += match raw.get(at + 1) {
                    Some(b'(') | Some(b')') => 3,
                    _ => 2,
                };
                continue;
            }
            if byte == b'\r' {
                col = 0;
                at += 1;
                continue;
            }
            if byte == b'\n' {
                row += 1;
                col = 0;
                at += 1;
                continue;
            }
            if byte >= 0x20 {
                let width = match byte {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                let end = (at + width).min(raw.len());
                if let Some(character) = String::from_utf8_lossy(&raw[at..end]).chars().next() {
                    if row < rows && col < cols {
                        grid[row][col] = character;
                    }
                    col += 1;
                }
                at = end;
                continue;
            }
            at += 1;
        }
        Screen {
            rows: grid
                .into_iter()
                .map(|line| line.into_iter().collect::<String>().trim_end().to_string())
                .collect(),
            underneath: Self::underneath(raw),
        }
    }

    /// What the terminal was sent before the picker took the screen.
    ///
    /// Nothing is laid out: the question asked of it is only whether a given
    /// sentence is in there, and a byte written to a screen that is about to be
    /// replaced counts wherever on it it landed. Everything is "underneath" when the
    /// switch never happened — which is the honest reading, since a run that drew no
    /// picker covered nothing, and it makes the assertion fail loudly rather than
    /// vacuously pass on a picker that never opened.
    fn underneath(raw: &[u8]) -> String {
        const ALTERNATE: &[u8] = b"\x1b[?1049h";
        let covered = raw
            .windows(ALTERNATE.len())
            .position(|window| window == ALTERNATE)
            .unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..covered]).into_owned()
    }

    /// What the terminal was sent once the picker gave the screen back.
    ///
    /// [`Screen::underneath`]'s mirror, and the only place a line `dl` prints *after*
    /// a pick can be read: those bytes arrive after skim has left the alternate
    /// screen, so they are on neither the picker's grid nor the screen it covered.
    /// Not laid out, for `underneath`'s reason — the question asked of it is whether
    /// a sentence is in there.
    ///
    /// A run whose picker never opened has nothing after it, which makes an
    /// assertion about these bytes fail rather than pass vacuously.
    fn afterwards(raw: &[u8]) -> String {
        const RESTORED: &[u8] = b"\x1b[?1049l";
        let given_back = raw
            .windows(RESTORED.len())
            .position(|window| window == RESTORED)
            .map_or(raw.len(), |at| at + RESTORED.len());
        String::from_utf8_lossy(&raw[given_back..]).into_owned()
    }

    /// The first row showing `text`, counted from 0.
    fn row_of(&self, text: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.contains(text))
    }

    /// The row the search bar is on, counted from 0.
    ///
    /// Found by shape rather than by looking for `> `, because skim draws the very
    /// same glyph as the cursor on the selected match — searching for it finds
    /// whichever comes first, which is the bug this file exists to catch answering
    /// the question asked about it. What tells them apart is the label: a match row
    /// carries `owner | id` and the prompt carries only what was typed.
    fn prompt_row(&self, spelled: &str) -> usize {
        self.rows
            .iter()
            .position(|row| row.starts_with('>') && !row.contains('|'))
            .unwrap_or_else(|| panic!("no search bar on the picker `dl {spelled}` drew:\n{self}"))
    }
}

impl std::fmt::Display for Screen {
    /// The screen as it looked, numbered — this is what a failure prints, and the
    /// only way to tell "drawn upside down" from "never drawn at all".
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, row) in self.rows.iter().enumerate() {
            writeln!(out, "{:>3} |{row}", index + 1)?;
        }
        Ok(())
    }
}

/// A scratch world with a `devpod` that lists three workspaces.
struct World {
    root: std::path::PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("dlt")
            .rand_bytes(6)
            .tempdir_in("/tmp")
            .expect("a scratch directory under /tmp");
        let root = scratch.path().to_path_buf();
        for directory in ["bin", "home", "cache", "config", "devpod"] {
            std::fs::create_dir_all(root.join(directory)).expect("a scratch directory");
        }
        let devpod = root.join("bin/devpod");
        std::fs::write(
            &devpod,
            r#"#!/bin/sh
# Every call is recorded, so a test can ask which workspace a pick reached.
echo "$@" >> "$DL_TEST_DEVPOD_LOG"
if [ "$1" = "list" ]; then
  cat <<'JSON'
[{"id": "blooop-devlaunch", "source": {"gitRepository": "https://github.com/blooop/devlaunch.git"},
  "lastUsed": "2026-08-22T10:00:00Z", "provider": {"name": "docker"},
  "ide": {"name": "none"}, "context": "default"},
 {"id": "blooop-wayfinder", "source": {"gitRepository": "https://github.com/blooop/wayfinder.git"},
  "lastUsed": "2026-08-21T10:00:00Z", "provider": {"name": "docker"},
  "ide": {"name": "none"}, "context": "default"},
 {"id": "myproject", "source": {"localFolder": "/home/dev/myproject"},
  "lastUsed": "2026-08-20T10:00:00Z", "provider": {"name": "docker"},
  "ide": {"name": "none"}, "context": "default"}]
JSON
  exit 0
fi
exit 0
"#,
        )
        .expect("a fake devpod");
        let mut mode = std::fs::metadata(&devpod)
            .expect("the fake devpod")
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&devpod, mode).expect("an executable fake devpod");

        World {
            root,
            _scratch: scratch,
        }
    }

    /// Where the fake devpod writes one line per call.
    fn devpod_log(&self) -> std::path::PathBuf {
        self.root.join("devpod-calls")
    }

    /// Every call the fake devpod was made, one per line, in the order they came.
    ///
    /// The background refresh `dl` spawns drives devpod too, so this is read for
    /// what it *contains* rather than compared whole.
    fn devpod_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.devpod_log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `dl <args>` against this world, environment and all.
    ///
    /// The same scratch `HOME`/`XDG_*`/`DEVPOD_HOME` shape `tests/read_side.rs`
    /// builds, for the same reason: nothing here may reach the real cache or the
    /// real devpod. `TERM` is the one addition, since this run has a terminal.
    fn command(&self, args: &[&str]) -> CommandBuilder {
        let root = self.root.display().to_string();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_dl"));
        for argument in args {
            command.arg(argument);
        }
        command.env_clear();
        // `KeepingCoverage` is a `std::process::Command` trait and this is not one,
        // so the one variable it would have re-admitted is passed through by hand.
        // Without it a coverage run records nothing for the picker.
        if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.env("PATH", format!("{root}/bin:/usr/bin:/bin"));
        command.env(
            "DL_TEST_DEVPOD_LOG",
            self.devpod_log().display().to_string(),
        );
        command.env("HOME", format!("{root}/home"));
        command.env("XDG_CACHE_HOME", format!("{root}/cache"));
        command.env("XDG_CONFIG_HOME", format!("{root}/config"));
        command.env("DEVPOD_HOME", format!("{root}/devpod"));
        command.env("TERM", "xterm-256color");
        command.env("GIT_SSH_COMMAND", "false");
        command.env("GIT_CONFIG_GLOBAL", "/dev/null");
        command.env("GIT_CONFIG_SYSTEM", "/dev/null");
        command.env(
            "DEVLAUNCH_COMPLETION_FILE",
            format!("{root}/home/completions.sh"),
        );
        command
    }
}
