# dl completion
# Note: This completion function does not support quoted arguments or escaped spaces.
# All arguments are treated as literal strings separated by whitespace.
# This is acceptable because GitHub usernames, repo names, and workspace names
# do not contain spaces or special characters that would require quoting.
#
# Implementation note: We parse COMP_LINE directly instead of adjusting COMP_WORDBREAKS
# because temporary COMP_WORDBREAKS modification can have side effects with bash's
# internal completion state and doesn't reliably prevent word splitting in all
# bash versions. Direct parsing gives us full control over word boundaries.
#
# The same function serves `aid`, whose first argument is a dl workspace spec
# too. Only the flag list and what follows the spec differ, so the two places
# that care branch on $cmd rather than the script being copied for aid.
_dl_completion() {
    local cur prev opts
    COMPREPLY=()

    # Extract current line from COMP_LINE instead of COMP_WORDS
    # This avoids issues with COMP_WORDBREAKS treating dashes as word boundaries
    local line="${COMP_LINE:0:COMP_POINT}"

    # Parse the line into an array of words (pure bash, no external processes)
    local words
    read -r -a words <<< "$line"
    local word_count=${#words[@]}

    # Extract current and previous words from the parsed array
    if (( word_count > 0 )); then
        cur="${words[word_count-1]}"
    else
        cur=""
    fi

    if (( word_count > 1 )); then
        prev="${words[word_count-2]}"
    else
        prev=""
    fi

    # If line ends with whitespace, we're starting a new word
    if [[ "$line" =~ [[:space:]]$ ]]; then
        ((word_count++))
        cur=""
        # Update prev when starting a new word
        if (( ${#words[@]} > 0 )); then
            prev="${words[-1]}"
        fi
    fi

    # The command being completed: dl or aid.
    local cmd=""
    if (( ${#words[@]} > 0 )); then
        cmd="${words[0]##*/}"
    fi

    # The options offered where a workspace spec goes.
    #
    # Every user-facing flag dl's argument grammar declares, and a test diffs the
    # two: `dl/tests/completion_tables.rs`. Five are deliberately absent —
    # --json, --size, --yes, --force and --force-worktrees each need something
    # already on the line, so none is ever the first word, and that test names
    # them with the reason. Anything added below has to be added there too.
    #
    # "Where the spec goes" is not "the second word": a modifying flag can come
    # first, so these are offered after one too (`dl --rm --<tab>`). Which word
    # that is, is the scan further down.
    #
    # The retired spellings (--stop, --autorm) are absent by rule rather than by
    # hand: the grammar marks them `hide = true`, and the test drops every hidden
    # flag, so a spelling this build only still answers for is never offered.
    local global_opts="--ls --install --refresh --prune --reconcile --purge --herdr-shell --herdr-setup --herdr-env --herdr-workspace --rm --devcontainer --claude-profile --claude-profiles --help -h --version"
    if [[ "$cmd" == aid ]]; then
        global_opts="--claude --codex --gemini --devcontainer --claude-profile --help -h --version"
    fi

    # Workspace subcommands
    # `--rm` is not a verb — the `rm` beside it is: the flag rides on `dl <ws>` and
    # `dl <ws> -- <cmd>` and deletes the workspace when that session ends, which is
    # exactly the position this list is offered in. Both are here because they are
    # two different requests, docker's `rm` and `run --rm`.
    local ws_cmds="up stop kill rm rme code restart recreate reset dotfiles --rm --"

    # Options that take a value; a variant name, a profile name or a path follows.
    local value_opts="--devcontainer --claude-profile --herdr-workspace"

    # The flags a workspace spec may still follow: they modify a launch instead
    # of being one. Every other flag ends the line, and that direction is the
    # load-bearing half -- listing the flags that *end* it instead put ten flags
    # in the wrong arm at once, because the ones nobody thinks to list are all on
    # that side: the command group's hidden members (--repos, --update-cache,
    # --completion-data), the five that modify a command already on the line
    # (--json, --size, --yes, --force, --force-worktrees), and the two retired
    # spellings. `dl --json my-workspace` is a clap error and `dl --repos
    # my-workspace` answers "--repos takes no workspace", so completing a name
    # after either is completing onto a refusal.
    #
    # Derived rather than judged, from three tables that are each already pinned:
    # the grammar's flags, minus clap's `what` group, minus the hidden ones,
    # minus `NOT_OFFERED_FIRST`. `dl/tests/completion_tables.rs` does that
    # subtraction and diffs the answer against this line.
    #
    # "Ends the line" means no *workspace* follows, which is not quite the same
    # as no word: `dl --install [<rc-file>]` takes an optional path. Nothing
    # completes that path, here or before this scan existed, and offering it
    # would mean a second exception rather than a wider `spec_follows` -- the
    # thing that follows is not a spec, and the branch below that handles `./`
    # is inside the spec position.
    local spec_follows="--rm --devcontainer --claude-profile"
    if [[ "$cmd" == aid ]]; then
        # aid's own, from `parse_aid_args`: it reads an agent flag, a remote
        # control flag or a dl value option and keeps looking for the spec. The
        # three it answers itself (--help, -h, --version) are absent because they
        # end the line, and so is an unknown flag -- aid cannot tell whether one
        # takes a value, so on `aid --unknown-taking-a-value foo owner/repo` it
        # calls `foo` the spec, and completing a slot aid itself cannot place is
        # worse than completing nothing.
        spec_follows="--claude --codex --gemini --remote-control --remote --no-remote-control --no-remote --devcontainer --claude-profile"
    fi

    if [[ "${prev}" == "--herdr-workspace" ]]; then
        return 0
    fi
    if [[ "${prev}" == "--herdr-env" ]]; then
        COMPREPLY=( $(compgen -W "set unset profile show clear" -- "${cur}") )
        return 0
    fi

    # After --claude-profile, offer the profile directories that exist. Read off the
    # disk rather than out of the completion cache, deliberately: profiles are
    # created by hand and rarely, the cache is rebuilt by commands that change
    # *workspaces*, and a profile you made a minute ago has to complete now. It is one
    # readdir of a directory holding a handful of entries.
    #
    # A mistyped name is a hard refusal at launch rather than a fallback to the default
    # login, which is what makes completing these worth the readdir.
    if [[ "${prev}" == "--claude-profile" || ( "${prev}" == "profile" && " ${COMP_WORDS[*]} " == *" --herdr-env "* ) ]]; then
        # The same three sources `domain::xdg::claude_profiles_root` reads, in the same
        # order: devlaunch's own scratch override, then claude-as's own variable, then
        # its default directory. `default` is offered because it is a name the resolver
        # answers for without any directory existing.
        local profiles_root="${DEVLAUNCH_CLAUDE_PROFILES_DIR:-${CLAUDE_PROFILES_DIR:-$HOME/.claude-profiles}}"
        local profiles="default" pdir pname
        if [[ -d "${profiles_root}" ]]; then
            for pdir in "${profiles_root}"/*/; do
                [[ -d "$pdir" ]] || continue
                pdir="${pdir%/}"
                pname="${pdir##*/}"
                # `ProfileName::parse`'s grammar, a second time: one directory
                # component of ASCII letters, digits, '.', '_' and '-', not starting
                # with '.' or '-'. Offering more than that is offering a completion the
                # launch refuses -- press tab, get `-flag` or `my profile`, and the
                # refusal is about a name you did not type by hand.
                #
                # The glob already hides the leading dot (it matches no dot-directory),
                # so the visible half of this is the leading '-' and the character set.
                # Both are checked anyway rather than relying on the glob, because the
                # rule is what has to agree and not the accident that enforces part of
                # it. `test_the_completion_offers_only_names_a_launch_accepts` in
                # test_bash_completion.py is the diff that keeps the two in step.
                # The character set is spelled out rather than written as ranges,
                # and that is not fussiness: `[[ =~ ]]` honours LC_COLLATE, so
                # `[A-Za-z]` matches `é` in a UTF-8 locale and this offered
                # `unicode-é` while `ProfileName::parse` -- which asks
                # `is_ascii_alphanumeric` -- refuses it. The test below caught it.
                [[ "$pname" =~ ^[abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-]+$ ]] || continue
                [[ "$pname" == [-.]* ]] && continue
                profiles+=" ${pname}"
            done
        fi
        COMPREPLY=( $(compgen -W "${profiles}" -- ${cur}) )
        return 0
    fi

    # After --devcontainer, offer the repo's variant directories (and paths).
    if [[ " ${value_opts} " == *" ${prev} "* ]]; then
        local variants=""
        if [[ -d .devcontainer ]]; then
            local d
            for d in .devcontainer/*/devcontainer.json; do
                [[ -f "$d" ]] || continue
                d="${d#.devcontainer/}"
                variants+=" ${d%/devcontainer.json}"
            done
        fi
        COMPREPLY=( $(compgen -W "${variants}" -- ${cur}) )
        compopt -o default 2>/dev/null
        return 0
    fi

    # Cache file location (honors XDG_CACHE_HOME)
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/devlaunch"
    local cache_file="$cache_dir/completions.bash"

    # Initialize completion variables
    local DL_WORKSPACES=""
    local DL_REPOS=""
    local DL_OWNERS=""
    local DL_BRANCHES=""

    # Source the bash cache file (fast, no jq needed)
    if [[ -f "$cache_file" ]]; then
        source "$cache_file"
    fi

    # Which positional slot the word being completed sits in: the spec is the
    # first, a verb the second.
    #
    # Read off the words before it rather than counted from the command, because
    # a flag can precede the spec in both grammars and counting cannot see that.
    # `aid --codex owner/repo` is the line that reported this: the spec was
    # offered at word two alone, so every agent-flag line completed nothing, and
    # so did `dl --devcontainer robot owner/repo`. dl's rule is clap's -- options
    # sit anywhere among the positional words -- and aid's is `parse_aid_args`,
    # which reads the leading flags and calls the first word that is not one the
    # spec.
    #
    # The words strictly before `cur` are indices 1 through word_count-2, which
    # holds whether or not the line ends in a space: the trailing-space branch
    # above incremented word_count without appending to `words`.
    local position=0 ends_here=0 modified=0 scan=1 scanned
    while (( scan <= word_count - 2 )); do
        scanned="${words[scan]}"
        if [[ "$scanned" == "--" ]]; then
            # Everything past it is the command run inside the workspace, which
            # is the user's shell to complete and not ours.
            return 0
        fi
        if [[ "$scanned" == -* ]]; then
            if [[ " ${spec_follows} " == *" ${scanned} "* ]]; then
                modified=1
            else
                ends_here=1
            fi
            if [[ " ${value_opts} " == *" ${scanned} "* ]]; then
                # Its value is not a positional word, so step over the pair.
                (( scan += 2 ))
            else
                (( scan++ ))
            fi
            continue
        fi
        (( position++ ))
        (( scan++ ))
    done

    # A line carrying a flag no spec follows takes neither a workspace nor a
    # verb. This is what used to be a guard on the first word starting with
    # `--`, which could not tell `dl --ls` from `dl --rm`.
    if (( ends_here )); then
        return 0
    fi

    # The spec's position: flags, workspaces, repos, owners, or paths.
    if (( position == 0 )); then
        # Flags. Once one modifier is on the line the line is a launch, so the
        # only flags that can still precede the spec are the other modifiers:
        # `dl --rm --ls` is refused, and `aid --codex --help` is not aid's help
        # (that is `argv[0]`) but an unknown option handed to dl.
        if [[ ${cur} == -* ]]; then
            if (( modified )); then
                COMPREPLY=( $(compgen -W "${spec_follows}" -- ${cur}) )
            else
                COMPREPLY=( $(compgen -W "${global_opts}" -- ${cur}) )
            fi
            return 0
        fi

        # If typing a path, complete files/directories
        if [[ "$cur" == ./* || "$cur" == /* || "$cur" == ~/* ]]; then
            COMPREPLY=( $(compgen -d -- ${cur}) )
            return 0
        fi

        # Check if completing branch (contains @)
        if [[ "$cur" == *@* ]]; then
            # Use cached branches (format: owner/repo@branch)
            if [[ -n "$DL_BRANCHES" ]]; then
                COMPREPLY=( $(compgen -W "${DL_BRANCHES}" -- ${cur}) )
            fi
            return 0
        fi

        # Check if completing owner/repo format (contains /)
        if [[ "$cur" == */* ]]; then
            # Don't add space - allow @branch suffix
            compopt -o nospace
            # Complete from known repos
            if [[ -n "$DL_REPOS" ]]; then
                COMPREPLY=( $(compgen -W "${DL_REPOS}" -- ${cur}) )
            fi
            return 0
        fi

        # Default: owners first, workspace ids only when no owner matches.
        #
        # The two namespaces collide for a single repository. An id is
        # `<repo-slug>-<ref-slug>-<suffix>` and `slug` turns `_` into `-`, so
        # `kinisi-robotics/kinisi_ros` derives ids that all begin `kinisi-ros`,
        # against an owner named `kinisi-robotics`. In one list bash completes to
        # the nine characters they share and stops. No fork and no second owner
        # is needed, which is why this is a precedence rule and not a filter.
        #
        # The owner wins because it continues: `/` is the next keystroke and the
        # `*/*` branch above completes the repo from there. An id is a finished
        # word, so it is held back rather than dropped -- `dl <id>` is a launch
        # arm (`WorkspaceSpec::ExistingIdOrName`), and the two guards below are
        # what keep an id copied out of `dl --ls` completable.
        local owners=""
        local owner
        for owner in $DL_OWNERS; do
            owners="$owners ${owner}/"
        done

        # Every owner matches the empty prefix, so without this the hold-back
        # swallows the workspace list on the one gesture that means "show me what
        # I have" -- and under `nospace` a lone owner is not a short list, it is
        # bash rewriting the line to `dl owner/`.
        if [[ -z "$cur" ]]; then
            compopt -o nospace
            COMPREPLY=( $(compgen -W "${owners} ${DL_WORKSPACES}" -- "") )
            return 0
        fi

        if [[ -n "$owners" ]]; then
            COMPREPLY=( $(compgen -W "${owners}" -- ${cur}) )
        fi

        if (( ${#COMPREPLY[@]} > 0 )); then
            # No trailing space: the `/` is a continuation, not the end of a word.
            compopt -o nospace
            # Holding an id back is only defensible while it is a delay, and for an
            # id typed out in full there is no longer prefix to reach. Reachable
            # because `DL_WORKSPACES` is every devpod workspace, hand-made names
            # included, so one really can be called after an owner.
            local workspace
            for workspace in $DL_WORKSPACES; do
                if [[ "$workspace" == "$cur" ]]; then
                    COMPREPLY+=( "$workspace" )
                    break
                fi
            done
        elif [[ -n "$DL_WORKSPACES" ]]; then
            # A space here, where the old shared branch suppressed it for every
            # candidate because some of them were owners.
            COMPREPLY=( $(compgen -W "${DL_WORKSPACES}" -- ${cur}) )
        fi
        return 0
    fi

    # The verb's position, after the spec. Everything after an aid workspace is
    # the prompt, so there is nothing to offer there.
    if (( position == 1 )) && [[ "$cmd" != aid ]]; then
        COMPREPLY=( $(compgen -W "${ws_cmds}" -- ${cur}) )
        return 0
    fi

    # After "--": no completion (user types shell command)
    return 0
}

# Use -o default for better completion behavior
complete -o default -F _dl_completion dl
complete -o default -F _dl_completion aid
# end dl completion
