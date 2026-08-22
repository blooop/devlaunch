# "A branch is a container." The second GIF: the point of the tool, and the
# part a reader cannot guess from the command line alone.
#
# See 1-launch.tape for how to run these, why the pre-warm is hidden and why
# the paths are quoted.

Require dl
Source "_common.tape"
Output "2-branches.gif"

# Taller than its siblings, and after the Source so it wins: two launches and a
# selector is nineteen lines, and the shared height would scroll the first one
# off the top before the second lands.
Set Height 380

# Two workspaces of the SAME repo, warmed off camera. Both have to exist before
# the selector is worth showing -- a selector with one row in it demonstrates
# nothing.
#
# `fix/42` does not exist on the remote, and that is the point twice over. `dl`
# creates a branch it cannot find, so the GIF shows that; and a branch that was
# never pushed cannot be merged and deleted out from under this tape, which is
# what happened to the real branch it used to name. Short, too -- every line
# here carries a workspace id built out of it, and the in-container prompt
# carries it twice.
Hide
Type "dl blooop/devlaunch -- true" Enter
Wait@240s
Type "dl blooop/devlaunch@fix/42 -- true" Enter
Wait@240s
Type "clear" Enter
Wait
Show

# Same repo, different branch: a second container, not a checkout in the first.
Type "dl blooop/devlaunch@fix/42"
Sleep 900ms
Enter
Sleep 5s

# `git branch` rather than `--show-current`: it fits in the 93 columns next to
# that prompt, and its second line is `main` -- the container running beside
# this one.
Type "git branch"
Sleep 300ms
Enter
Sleep 2500ms

Type "exit"
Sleep 300ms
Enter
Wait
Sleep 1200ms

# No arguments: the built-in selector over what you already have. Both branches
# are listed, which is the frame this GIF exists to show.
Type "dl"
Sleep 700ms
Enter
Sleep 2500ms

# Filter to one row, take it.
#
# Check that it really is one row when you re-record. The picker is skim with
# `no_sort` (rust/dl/src/select.rs), so it filters but never re-ranks: if both
# rows survive the query, Enter takes the cursor's, and the cursor is on the
# workspace you just left -- devpod lists most-recently-used first. `main`
# narrows to one here because a row is `owner | id` and nothing in
# `blooop | devlaunch-fix-42-...` spells m-a-i-n.
Type "main"
Sleep 1800ms
Enter
Sleep 5s

Type "git branch"
Sleep 300ms
Enter
Sleep 2500ms

Type "exit"
Sleep 300ms
Enter
Wait
Sleep 1200ms
