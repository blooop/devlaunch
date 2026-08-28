//! `rme`: hanging up the shell `dl` was called from, once the removal is done.
//!
//! One verb reaches this module. `dl <ws> rme` deletes the workspace exactly as
//! `dl <ws> rm` does, and then sends SIGHUP to `dl`'s parent process — which for
//! the line it exists for is an interactive shell, so the shell ends and the
//! terminal tab it was sitting in closes on its own. The workspace tab has nothing
//! left to do at that point but be closed by hand, and the delete it was waiting
//! on is long enough that the hand arrives late.
//!
//! # Three decisions, and none of them is the signal
//!
//! - **It is asked at the end of the whole command, not inside the delete.** A
//!   picked batch is one `rme` and five removals ([`crate::cli::Verb::several_at_once`]),
//!   and the shell is hung up once, after the last of them. So the question lives
//!   here, between [`crate::commands::dispatch`] and the ending it returns, rather
//!   than in `render_remove` — which cannot see how many workspaces this command
//!   is about.
//! - **Only [`Ending::Done`].** The unsaved-work guard refusing, a devpod that
//!   would not finish, a target nothing answers to: every one of those has a
//!   sentence on stderr, and hanging up the terminal it was written to is the one
//!   way to guarantee nobody reads it. A batch is `Done` only when every removal
//!   in it was, since [`crate::commands`] keeps the first ending that was not.
//!
//!   **`--force` widens what `Done` means, and it is the one case where the shell
//!   goes without a removal behind it.** `--force` passes devpod's own
//!   `--ignore-not-found`, so "there was nothing there" succeeds; a path spec is
//!   resolved without asking devpod anything at all. Between them,
//!   `dl ./wrong-directory rme --force` deletes nothing, ends `Done`, and hangs the
//!   shell up — taking with it the `Workspace <id> is gone.` line that
//!   [`crate::render::removed`] words that way precisely because dl cannot tell an
//!   absence from a removal here. That hazard is `rm --force`'s already and is
//!   pinned for it; what `rme` adds is that the receipt has no reader. It stands
//!   rather than being special-cased because absence is what `--force` asks for and
//!   the common forced run is a real workspace whose scratch work you have decided
//!   against, where closing the tab is the whole point. Telling the two apart needs
//!   `target::resolve` to carry whether devpod ever confirmed the workspace, which
//!   it does not.
//! - **The parent process, whatever it is, and that is less predictable than it
//!   looks.** There is no way to ask whether a parent owns a terminal, so `dl`
//!   names the pid it signalled instead of guessing. The reason it must is that a
//!   shell often does not leave a process where you would expect one: a subshell
//!   running a single command is usually *replaced* by it rather than forking, so
//!   `$(dl <ws> rme)` signals the shell that typed the line and closes the
//!   terminal (measured: bash 5 and dash both). Add a redirection or a
//!   `VAR=x` prefix and the subshell survives to take the signal instead
//!   (measured). Nothing in the line says which, so the pid is the only answer
//!   there is. A `nohup` is the one form `dl` refuses outright, above.
//!
//! Nothing here is reachable from `rm`: [`crate::cli::AfterRemoval`] is the only
//! way in, and the grammar builds `HangUpTheShell` for one word.

use std::io::Write as _;

use crate::cli::AfterRemoval;
use crate::commands::Ending;
use crate::render;

/// Whether this command ends with the calling shell hung up.
///
/// Pure, and separate from [`after_the_command`] for exactly one reason: it is the
/// half a test can ask. The other half sends a signal to a process outside the test
/// binary, and the process a `cargo test` child's `getppid()` names is the test
/// runner itself.
pub(crate) fn wanted(after: AfterRemoval, ending: Ending) -> bool {
    match after {
        AfterRemoval::LeaveTheShell => false,
        // `Done` and nothing else, including `Child(Exit::Code(0))`: that arm is a
        // `devpod delete` dl handed the status of back, which is the delete it
        // refused, and a 0 there would still not be a workspace dl saw go.
        //
        // Without `--force`, `Done` is a removal: devpod fails on a workspace it
        // does not have, so the delete path only reaches it having seen one go.
        // With `--force` it is established *absence*, which may be a removal and
        // may be a mistyped path — see the module docs, where that is named rather
        // than special-cased.
        AfterRemoval::HangUpTheShell => matches!(ending, Ending::Done),
    }
}

/// Hang up the calling shell if that is what was asked, and hand the ending back
/// either way.
///
/// The ending is returned unchanged. `dl <ws> rme` exits 0 on a removal that
/// worked, as `rm` does, because the removal is what the exit code is *about* — and
/// on the run this is for nobody reads it anyway, the shell that would have being
/// the one that just went.
pub(crate) fn after_the_command(after: AfterRemoval, ending: Ending) -> Ending {
    if wanted(after, ending) {
        hang_up_the_parent();
    }
    ending
}

/// SIGHUP to whatever started `dl`.
fn hang_up_the_parent() {
    // Something disarmed SIGHUP before this run started, which is what `nohup dl …`
    // does — and a shell runs that by `exec`ing dl in place, so the parent about to
    // be signalled is the terminal `nohup` was typed to outlive. `dl` already
    // treats an inherited ignore as a deliberate statement and leaves the signal
    // disarmed for the whole run (`install_signal_handlers`); sending the one it
    // refuses to act on would be the same process arguing both sides.
    if crate::sighup_arrived_ignored() {
        eprintln!("{}", render::hangup_disarmed());
        return;
    }
    // SAFETY: `getppid` reads a property of this process and touches nothing.
    let parent = unsafe { libc::getppid() };
    // 1 is an orphan's parent: the shell that asked has already gone, so the
    // removal is the whole of what this run did. Refused rather than sent, because
    // pid 1 is init and a root `dl` would be asking it to shut the machine's
    // service manager down.
    if parent <= 1 {
        eprintln!("{}", render::nothing_to_hang_up());
        return;
    }
    eprintln!("{}", render::hanging_up(parent));
    // Before the signal, not after: `main`'s flush is on the other side of this
    // call, and stdout here is a pipe or a file as often as a terminal — which is
    // to say block-buffered, holding lines whose reader is the process about to be
    // killed.
    let _ = std::io::stdout().flush();
    // Our own SIGHUP, disarmed first. A shell that dies of this signal can hang up
    // its own jobs on the way out, and `dl` is the foreground one: the handler
    // `install_signal_handlers` put in would then `_exit(129)` out from under the
    // exit code this run earned. Ignoring it is safe to hold from here on, since
    // everything left is the return path.
    //
    // SAFETY: setting a disposition on a single-threaded process, after the last
    // work this run does and with nothing left to be interrupted.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    // SAFETY: `kill` signals one pid and touches nothing in this process.
    if unsafe { libc::kill(parent, libc::SIGHUP) } != 0 {
        // A parent that exited between `getppid` and here (ESRCH), or one this
        // user may not signal (EPERM). Reported rather than swallowed: the
        // terminal staying open is the visible outcome either way, and this is the
        // only thing that says why.
        eprintln!(
            "{}",
            render::could_not_hang_up(parent, &std::io::Error::last_os_error().to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    //! The decision, as a table. The signal itself is judged at the binary
    //! boundary, from inside a shell that can be hung up without taking the test
    //! runner with it — `dl/tests/lifecycle.rs`.

    use super::*;
    use devlaunch_core::runner::Exit;

    #[test]
    fn rm_never_hangs_up_whatever_happened() {
        for ending in [
            Ending::Done,
            Ending::Refused,
            Ending::DevpodMissing,
            Ending::Child(Exit::Code(0)),
            Ending::Session(0),
        ] {
            assert!(
                !wanted(AfterRemoval::LeaveTheShell, ending),
                "rm hung up the shell after {ending:?}"
            );
        }
    }

    #[test]
    fn rme_hangs_up_on_a_removal_that_worked_and_on_nothing_else() {
        assert!(wanted(AfterRemoval::HangUpTheShell, Ending::Done));
        for ending in [
            // The unsaved-work guard, and every target dl could not address.
            Ending::Refused,
            Ending::DevpodMissing,
            // devpod ran and would not let go of the workspace. Its exit code is
            // handed back, and a 0 here is not the delete path's answer for a
            // delete that happened.
            Ending::Child(Exit::Code(0)),
            Ending::Child(Exit::Code(1)),
            Ending::Session(0),
        ] {
            assert!(
                !wanted(AfterRemoval::HangUpTheShell, ending),
                "rme hung up the shell after {ending:?}, where the reason is on stderr"
            );
        }
    }
}
