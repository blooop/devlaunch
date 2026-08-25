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

    # Global command options (only valid as first arg).
    #
    # Every user-facing flag dl's argument grammar declares, and a test diffs the
    # two: `dl/tests/completion_tables.rs`. Four are deliberately absent —
    # --json, --size, --yes and --force modify a line that already named a
    # command, so none of them is ever the first word — and that test names them
    # with the reason. Anything added below has to be added there too.
    #
    # The retired spellings (--stop, --autorm) are absent by rule rather than by
    # hand: the grammar marks them `hide = true`, and the test drops every hidden
    # flag, so a spelling this build only still answers for is never offered.
    local global_opts="--ls --install --refresh --prune --reconcile --purge --rm --devcontainer --help -h --version"
    if [[ "$cmd" == aid ]]; then
        global_opts="--claude --codex --gemini --devcontainer --help -h --version"
    fi

    # Workspace subcommands
    # `--rm` is not a verb — the `rm` beside it is: the flag rides on `dl <ws>` and
    # `dl <ws> -- <cmd>` and deletes the workspace when that session ends, which is
    # exactly the position this list is offered in. Both are here because they are
    # two different requests, docker's `rm` and `run --rm`.
    local ws_cmds="up stop rm code restart recreate reset dotfiles --rm --"

    # Options that take a value; a variant name or a path follows them.
    local value_opts="--devcontainer"

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

    # First argument: global flags, workspaces, repos, owners, or paths
    if [[ ${word_count} -eq 2 ]]; then
        # Global flags
        if [[ ${cur} == -* ]]; then
            COMPREPLY=( $(compgen -W "${global_opts}" -- ${cur}) )
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
        # These are two namespaces for the same repository, and offering them as
        # one list is what used to make `kin<TAB>` stall. The owner
        # `kinisi-robotics/` and every id derived from `kinisi_ros` share the
        # prefix `kinisi-ro`, because `slug` turns the `_` into a `-` and an id is
        # `<repo-slug>-<ref-slug>-<suffix>`. bash completes to the longest common
        # prefix of its candidates, so it stopped at `kinisi-ro` and offered a
        # screen of ids. One repository is enough to do that: the colliding pair is
        # an owner and the ids of its own repo, so no fork and no second owner is
        # needed, and no amount of typing helps until the two spellings diverge.
        #
        # The owner wins the tie because it continues. `/` is the next keystroke,
        # and the `*/*` branch above completes the repo from there, so `kin` now
        # reaches `kinisi-robotics/kinisi_ros` in two tabs.
        #
        # An id is a whole word with nothing after it, and it stays reachable
        # rather than being dropped: keep typing and the prefix stops matching any
        # owner, which is exactly when this offers the ids instead. That is the
        # case the fallback exists for -- an id pasted or half-typed out of
        # `dl --ls` still completes, and `dl <id>` is a launch arm the grammar
        # keeps (`WorkspaceSpec::ExistingIdOrName`).
        #
        # Nothing here changes what the cache holds, so a `completions.bash`
        # written by an older build works unchanged and vice versa.
        local owners=""
        local owner
        for owner in $DL_OWNERS; do
            owners="$owners ${owner}/"
        done

        # Nothing typed is nothing to disambiguate. Every owner matches the empty
        # prefix, so without this the hold-back below swallows the whole workspace
        # list on the one gesture that means "show me what I have" -- and with a
        # single owner it is worse than a short list, because one candidate under
        # `nospace` is bash rewriting the line to `dl owner/`.
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
        elif [[ -n "$DL_WORKSPACES" ]]; then
            # A space, unlike the old shared branch, which suppressed it for every
            # candidate because some of them were owners. An id is finished when it
            # matches, and the space is what lets a verb be typed straight after.
            COMPREPLY=( $(compgen -W "${DL_WORKSPACES}" -- ${cur}) )
        fi
        return 0
    fi

    # Second argument (after workspace): subcommands. Everything after an aid
    # workspace is the prompt, so there is nothing to offer there.
    if [[ ${word_count} -eq 3 && "$cmd" != aid ]]; then
        # Don't complete after global flags
        # Extract the first argument (word after "dl") from the words array
        local first=""
        if (( ${#words[@]} > 1 )); then
            first="${words[1]}"
        fi
        if [[ "$first" == --* ]]; then
            return 0
        fi

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
