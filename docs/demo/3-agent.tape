# "An agent instead of a shell." The third GIF: name a repo, get asked for the
# prompt while the container boots, press Enter, land in the container with the
# agent running in the checkout.
#
# It cuts a few seconds after the agent takes the screen. Left running it would
# put whatever the model said that afternoon on the front page, and eventually
# the account's own status line; what teaches the workflow is the handover, not
# the answer.
#
# A third workspace, and a branch of its own: the agent gets the flaky test
# while the feature branch from 2-branches.tape stays where it was. 4-cleanup
# then has three rows to mark.
#
# See 1-launch.tape for how to run these.

Require dla
Source "_common.tape"
Output "3-agent.gif"

# The agent draws a full-screen UI, so give it something to draw in.
Set Height 380

Hide
Type "dl blooop/devlaunch@fix/flaky-test -- true" Enter
Wait@240s
Type "clear" Enter
Wait
Show

Type "dla blooop/devlaunch@fix/flaky-test"
Sleep 900ms
Enter

# The question arrives while the container is still coming up. Wait for it
# rather than sleep: dla can take over three seconds to ask, and keys typed
# before it does echo above the question instead of inside it.
Wait+Screen@60s /Booting/
Sleep 2s
Type "fix the flaky test in the launch suite"
Sleep 2s
Enter

# Attach, then the agent's screen. Attach time moves from run to run -- two
# seconds on a warm ssh connection, six on a cold one -- so a fixed Sleep either
# cuts before the agent paints or runs on into its answer. Wait for the banner
# instead; the transcript starts scrolling about two seconds after it, taking
# the banner with it, so this ends while the banner is still whole.
Wait+Screen@60s /Claude Code v/
Sleep 1500ms
