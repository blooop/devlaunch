//! Put the terminal back the way a child found it.
//!
//! A child that inherits this process's stdout inherits the *terminal*, and a
//! terminal is not only a stream: it holds modes an application switches on for
//! its own use and is expected to switch off again on the way out. The kitty
//! keyboard protocol, bracketed paste, mouse reporting and the alternate screen
//! all live in the emulator, not in the connection and not in the tty driver, so
//! nothing between the application and the glass undoes them.
//!
//! An application that exits normally undoes its own. One that is *killed* never
//! gets to, and the terminal is left in a mode nobody asked for. The case this
//! module exists for is the remote one, because that is where the killing
//! happens: `dl` attaches to a workspace over `ssh -t`, the container goes away
//! underneath the session (`devpod` reports `error tunneling to container: exit
//! status 137`), and the agent inside it dies without a chance to clean up. ssh
//! restores the *local* tty settings on its way out — raw mode, echo — so the
//! damage is invisible until it is baffling: echo and line editing work, and yet
//! Ctrl-C does nothing and ordinary keys print `9;133u` at the shell prompt.
//! Those are kitty keyboard-protocol key reports, still switched on. Ctrl-C is
//! among them, arriving as `ESC [ 99;5 u` instead of the byte the line discipline
//! turns into SIGINT.
//!
//! `dl` is the last process holding that terminal, so `dl` is what can repair it.
//!
//! # Every sequence here is a no-op when it is not needed
//!
//! [`RESTORE`] is written after **every** session, not only after one that ended
//! badly, because "did the remote die without cleaning up" is not a question
//! ssh's exit status answers. That is only safe if a restore costs nothing when
//! there was nothing to restore, so each sequence below was chosen for being
//! idempotent, and the one obvious candidate that is *not* was left out:
//!
//! `ESC [ ? 1049 l` is the usual way to leave the alternate screen, and it is
//! also a DECRC — xterm's ctlseqs defines it as "Use Normal Screen Buffer and
//! restore cursor as in DECRC". Sent on a terminal that is already on the normal
//! screen it therefore *moves the cursor* to whatever was last saved, which for a
//! terminal that never saved one is home. That would scramble the display on
//! every clean exit, which is a worse bug than the one being fixed. `ESC [ ?
//! 1047 l` is the half without the cursor restore, and it does the job: measured
//! against a real emulator, on the normal screen it changes nothing, and from the
//! alternate screen it comes back.
//!
//! # What this cannot reach
//!
//! [`crate::Runner::capture`] pipes stdout, so a captured child's escape
//! sequences never reach the terminal in the first place, and the restore
//! (written to descriptor 1) could not reach it either. Nothing is lost: the
//! captured children are `git`, `gh` and `devpod list`, which read the terminal
//! through `/dev/tty` for a prompt but set no modes on it.

/// The sequences that undo what a killed full-screen application leaves behind.
///
/// Ordered outside-in: leave the alternate screen first, so anything after it
/// applies to the screen the user is actually looking at.
///
/// - `ESC [ ? 1047 l` — leave the alternate screen. See the module docs for why
///   this and not `1049`.
/// - `ESC [ < u` — pop one entry off the kitty keyboard-protocol stack, undoing
///   one unmatched push. A pop on an empty stack is defined as doing nothing, and
///   a terminal that does not know the protocol sees a CSI sequence with a
///   private-parameter byte and an unknown final byte, which every conformant
///   parser swallows. Exactly one, because exactly one was stranded: popping
///   further would unwind a push belonging to whatever was running *before*
///   `dl`.
/// - `ESC [ ? 2004 l` — bracketed paste off, or a paste arrives wrapped in
///   `ESC [ 200 ~` markers that the shell prints instead of obeying.
/// - `ESC [ ? 1000 l` … `ESC [ ? 1015 l` — the mouse tracking modes and the three
///   encodings, or a click spits coordinates at the prompt.
/// - `ESC [ ? 25 h` — show the cursor.
/// - `ESC [ ? 7 h` — autowrap on, which is the power-on default.
/// - `ESC [ 0 m` — drop any colour or attribute left half-applied.
pub(crate) const RESTORE: &str = concat!(
    "\x1b[?1047l", // alternate screen off
    "\x1b[<u",     // kitty keyboard protocol: pop one stack entry
    "\x1b[?2004l", // bracketed paste off
    "\x1b[?1000l", // mouse: normal tracking off
    "\x1b[?1002l", // mouse: button-event tracking off
    "\x1b[?1003l", // mouse: any-event tracking off
    "\x1b[?1005l", // mouse: UTF-8 encoding off
    "\x1b[?1006l", // mouse: SGR encoding off
    "\x1b[?1015l", // mouse: urxvt encoding off
    "\x1b[?25h",   // cursor visible
    "\x1b[?7h",    // autowrap on
    "\x1b[0m",     // attributes reset
);

/// Write [`RESTORE`] to the terminal, if there is one to write it to.
///
/// Silent and best-effort by design: this is called on the way out of a session
/// and from the interrupt handler, and there is no useful thing to say to a user
/// whose terminal has just hung up.
///
/// **Async-signal-safe**, which is the constraint that shapes the whole function:
/// [`crate::interrupt::cleanup_and_exit`] calls it. It reads errno and calls
/// `isatty` and `write`, all three of which POSIX lists as safe in a handler, and
/// it allocates nothing — the bytes are a `const`, not a formatted string.
pub(crate) fn restore() {
    // Descriptor 1 alone, and deliberately not the both-descriptors question
    // `ssh::terminal_usable` asks. That one decides whether to give a *child* a
    // pty, which a program with nobody typing at it does not need. This one asks
    // whether there is a device here to repair, and the child was handed
    // descriptor 1 whatever its stdin was: `dl owner/repo < /dev/null` still lets
    // `devpod up` hide the cursor on the terminal and still leaves it hidden if
    // the build is killed.
    //
    // SAFETY: `isatty` reads a property of a descriptor, touches nothing else,
    // and is on POSIX's async-signal-safe list.
    if unsafe { libc::isatty(1) != 1 } {
        return;
    }
    write_all(RESTORE.as_bytes());
}

/// `write(2)` until the whole buffer is gone, or until the descriptor says stop.
///
/// A short write is possible on a terminal and would otherwise truncate a
/// sequence mid-way, leaving the tail to be printed as text — the exact failure
/// this module exists to prevent, self-inflicted.
fn write_all(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `write` to descriptor 1 from a live slice of that length. It is
        // async-signal-safe, and the slice is a `'static` const.
        let wrote = unsafe { libc::write(1, bytes.as_ptr().cast(), bytes.len()) };
        if wrote > 0 {
            // A `write` that returns positive returned at most `bytes.len()`.
            bytes = &bytes[wrote as usize..];
        } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            // A signal landed mid-write; nothing was lost, so go round again.
            // `last_os_error` only reads errno and builds a `Repr::Os`, which
            // allocates nothing.
        } else {
            // EPIPE or EIO: the terminal is gone, and there is nothing a process
            // on its way out can do about that.
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of writing this unconditionally: nothing in it may depend
    /// on the mode actually being set. `1049l` is the one that would, and it was
    /// measured moving the cursor on a terminal that had never entered the
    /// alternate screen, so its absence is pinned rather than left to the comment
    /// beside it.
    #[test]
    fn the_restore_holds_no_sequence_that_acts_when_it_is_not_needed() {
        assert!(
            !RESTORE.contains("1049"),
            "1049l restores the cursor as well as the screen, so it is not a \
             no-op on a terminal that never left the normal screen: {RESTORE:?}"
        );
        assert!(
            RESTORE.contains("\x1b[?1047l"),
            "the alternate screen is still one of the modes a killed child can \
             strand: {RESTORE:?}"
        );
    }

    /// The reported bug, named directly: without this pop, Ctrl-C reaches the
    /// shell as `ESC [ 99;5 u` instead of as the byte that raises SIGINT.
    #[test]
    fn the_restore_pops_the_kitty_keyboard_stack_exactly_once() {
        assert!(RESTORE.contains("\x1b[<u"), "{RESTORE:?}");
        assert_eq!(
            RESTORE.matches("\x1b[<u").count(),
            1,
            "one unmatched push is what a killed child leaves; a second pop would \
             unwind whatever was running before dl: {RESTORE:?}"
        );
    }

    /// Every sequence is a complete CSI: introducer, parameters, final byte. A
    /// truncated one would be printed as text by the terminal, which is the
    /// failure this module exists to prevent.
    #[test]
    fn every_sequence_is_a_whole_csi() {
        assert!(RESTORE.starts_with('\x1b'), "{RESTORE:?}");
        for piece in RESTORE.split('\x1b').skip(1) {
            assert!(
                piece.starts_with('[') && piece.len() >= 2,
                "{piece:?} is not a CSI sequence"
            );
            let final_byte = piece.chars().last().expect("a non-empty piece");
            assert!(
                ('\x40'..='\x7e').contains(&final_byte),
                "{piece:?} has no CSI final byte"
            );
        }
    }
}
