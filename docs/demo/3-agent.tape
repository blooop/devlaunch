# "An agent instead of a shell." The third GIF: name a repo, get asked for the
# prompt while the container boots, press Enter, land in the container with the
# agent running in the checkout.
#
# It cuts a few seconds after the agent takes the screen. Left running it would
# put whatever the model said that afternoon on the front page, and eventually
# the account's own status line; what teaches the workflow is the handover, not
# the answer.
#
# See 1-launch.tape for how to run these.

Require aid
Source "_common.tape"
Output "3-agent.gif"

# The agent draws a full-screen UI, so give it something to draw in.
Set Height 380

Hide
Type "dl blooop/devlaunch@fix/42 -- true" Enter
Wait@240s
Type "clear" Enter
Wait
Show

Type "aid blooop/devlaunch@fix/42"
Sleep 900ms
Enter

# The question arrives while the container is still coming up.
Sleep 3s
Type "add a --json flag to the timing output"
Sleep 2s
Enter

# Attach, then the agent's screen. The window between it painting and the model
# writing its first line is about two seconds, so this is timed to land inside
# it -- check the tail of the GIF when you re-record.
Sleep 8s
