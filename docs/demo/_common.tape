# Shared look for every demo GIF, Sourced by the tapes next to this file.
#
# One place for it so the GIFs on the README look like one set rather than
# three separate recordings, and so a change to the width or the font is a
# one-line diff instead of a three-file one.
#
# No Output line here on purpose -- each tape names its own, and a Source that
# set one would silently decide where its caller writes.

Set Shell bash

# 900px wide is a deliberate ceiling, not a taste call. VHS has no colour or
# file-size setting (README, "Settings"), so the only levers on the weight of a
# GIF are its dimensions, its length and its framerate -- and a README that
# autoplays three of them on load is a page that has to stay small. GitHub
# renders the front page at about 900px anyway, so anything wider is downscaled
# bytes nobody sees.
Set Width 900

# 14 and 12 buy 93 columns in those 900px. That is measured, not chosen: the
# prompt inside a workspace is `vscode -> /workspaces/<id> (<branch>) $` before
# you type anything, and a line that wraps mid-word is what makes a terminal
# recording look broken. Smaller font, more columns, less legible at GitHub's
# 1:1; bigger and long lines wrap. `tput cols` in a tape tells you what you
# actually got, and the branch the tapes use is short for the same reason.
Set FontSize 14
Set Padding 12

# About eleven lines, which is what the short tapes need. The two that do more
# set a Height of their own.
Set Height 240

Set Framerate 24

# Named rather than left to the default, so the same tape recorded on two
# machines produces the same colours.
Set Theme "Dracula"
Set WindowBar Colorful
Set BorderRadius 8

# Slower than a real typist. These GIFs autoplay with no controls, no scrubber
# and no pause -- the reader gets one pass at whatever speed we chose, so the
# speed is part of the writing.
Set TypingSpeed 55ms
