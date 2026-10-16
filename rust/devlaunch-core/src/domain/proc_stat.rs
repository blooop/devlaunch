//! One line of `/proc/<pid>/stat`, read the one way every caller shares.
//!
//! Two flows read process tables, on the host and inside containers: the agent
//! lock check (`flows::agent_worktrees`) and the refresh of stale workspaces
//! (`flows::refresh_stale`). The line has one shape, so it has one parser.

/// The fields dl reads out of one `/proc/<pid>/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcStat {
    pub(crate) pid: u32,
    /// The command name, as the kernel keeps it: at most 15 bytes, and any
    /// bytes at all, spaces and parentheses included.
    pub(crate) comm: String,
    /// A zombie: exited, and not yet reaped by its parent.
    pub(crate) zombie: bool,
    pub(crate) ppid: u32,
    /// `PF_KTHREAD` is in the flags: a thread of the kernel's own.
    pub(crate) kernel_thread: bool,
    /// The start time, in clock ticks after boot.
    pub(crate) start: u64,
}

/// `PF_KTHREAD`, in the flags field of `/proc/<pid>/stat`. Set by the kernel on
/// its own threads, and nothing in user space can set it.
const PF_KTHREAD: u64 = 0x0020_0000;

/// Read one `/proc/<pid>/stat` line, or nothing when it is not that shape.
///
/// The command name sits in parentheses and may hold anything, `) ` included,
/// so the name ends at the *last* `)` and the fields are counted from there.
/// After the name: the state is the 1st field, the parent the 2nd, the flags
/// the 7th (field 9 of the line) and the start time the 20th (field 22).
pub(crate) fn parse(line: &str) -> Option<ProcStat> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let (head, rest) = line.split_once(" (")?;
    let pid = digits(head, 10)?.parse().ok()?;
    let close = rest.rfind(')')?;
    let after: Vec<&str> = rest[close + 1..].split_whitespace().collect();
    let flags: u64 = digits(after.get(6)?, 20)?.parse().ok()?;
    Some(ProcStat {
        pid,
        comm: rest[..close].to_owned(),
        zombie: *after.first()? == "Z",
        ppid: digits(after.get(1)?, 10)?.parse().ok()?,
        kernel_thread: flags & PF_KTHREAD != 0,
        start: digits(after.get(19)?, 20)?.parse().ok()?,
    })
}

/// `text` when it is all ASCII digits, at least one and at most `at_most`.
pub(crate) fn digits(text: &str, at_most: usize) -> Option<&str> {
    (!text.is_empty() && text.len() <= at_most && text.bytes().all(|it| it.is_ascii_digit()))
        .then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real line, from a process that named itself `a) b` with `prctl`.
    const ODD_NAME: &str = "1234 (a) b) S 1 1234 1234 0 -1 4194560 155 0 0 0 0 0 0 0 20 0 1 0 \
                            329153 1000 10 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 \
                            0 0 0 0\n";

    #[test]
    fn a_name_that_holds_a_closing_parenthesis_ends_at_the_last_one() {
        assert_eq!(
            parse(ODD_NAME),
            Some(ProcStat {
                pid: 1234,
                comm: "a) b".to_owned(),
                zombie: false,
                ppid: 1,
                kernel_thread: false,
                start: 329_153,
            })
        );
    }

    #[test]
    fn a_line_cut_short_or_not_a_stat_line_is_nothing() {
        assert_eq!(parse("8621 (short) S 1"), None);
        assert_eq!(parse("garbage"), None);
        assert_eq!(parse("x (y"), None);
    }
}
