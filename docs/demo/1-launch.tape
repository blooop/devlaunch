# "One command and you are inside." The first GIF on the README.
#
#   pixi run demo        # records this and its siblings
#
# vhs films the RELEASED `dl`, which needs devpod and docker, so a host is the
# easy place to record from; scripts/record_demo.sh covers doing it inside this
# repo's devcontainer instead. `Require dl` fails the recording in a line
# rather than filming a `command not found`.
#
# Re-record when the Quickstart's commands change. That is the whole reason
# this is a file: the diff says what the demo now claims.
#
# Quote the paths: vhs's lexer reads a bare `_common.tape` or `1-launch.gif` as
# a chain of commands, and the six parse errors it prints say nothing about it.

Require dl
Source "_common.tape"
Output "1-launch.gif"

# Pre-warm off camera. A repo's FIRST launch builds a devcontainer and takes
# minutes; the launch worth showing is the second one, which is the one you do
# every day. Hiding the build keeps the GIF short without faking it -- every
# frame after Show is a real, unedited `dl`.
#
# Wait@240s is a ceiling, not a pause -- a warm workspace answers in about three
# seconds. The 240 is there for the first run on a machine, which builds an
# image. Bare `Wait` uses vhs's default `/>$/`, and the shell it films is vhs's
# own, so that matches.
Hide
Type "dl blooop/devlaunch -- true" Enter
Wait@240s
Type "clear" Enter
Wait
Show

Type "dl blooop/devlaunch"
Sleep 900ms
Enter

# Sleeps rather than Wait for the in-container prompt: the prompt inside an
# arbitrary repo's container is whatever that repo's dotfiles make it, so there
# is no pattern to wait on. Overshooting costs a still frame; undershooting
# costs nothing either, since bash reads buffered keystrokes once it is up.
Sleep 5s

Type "pwd && git branch --show-current"
Sleep 300ms
Enter
Sleep 2500ms

Type "exit"
Sleep 300ms
Enter
Wait
Sleep 1500ms
