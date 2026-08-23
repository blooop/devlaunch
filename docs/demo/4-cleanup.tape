# "Clearing them out." The last GIF: the same selector, opened by a verb, with
# TAB marking more than one row.
#
# This is the end of the workflow, and the only tape that destroys what it
# films -- so it runs last, and the next run of `pixi run demo` pays for a cold
# rebuild in 1-launch.tape's pre-warm.
#
# It needs the clones clean, since `dl rm` refuses work that is nowhere else.
# 3-agent.tape leaves the agent running for two or three seconds, which is long
# enough to read files and not to edit one; if you lengthen it, check
# `git status` in that workspace before blaming this tape.
#
# See 1-launch.tape for how to run these.

Require dl
Source "_common.tape"
Output "4-cleanup.gif"

# Two workspaces to delete, and the deletion log is three lines each.
Set Height 380

Hide
Type "dl blooop/devlaunch -- true" Enter
Wait@240s
Type "dl blooop/devlaunch@fix/42 -- true" Enter
Wait@240s
Type "clear" Enter
Wait
Show

# A verb with no workspace named opens the picker for it.
Type "dl rm"
Sleep 700ms
Enter
Sleep 2500ms

# TAB marks and steps down, so two of them mark both rows. The count in the
# corner goes [1], [2].
Tab
Sleep 1000ms
Tab
Sleep 1500ms
Enter

# Wait, not Sleep: deleting two containers took four seconds one run and seven
# the next, and a fixed sleep that lands short types the next line into the
# middle of the log.
Wait@60s
Sleep 1500ms

Type "dl --ls"
Sleep 300ms
Enter
Sleep 3s
