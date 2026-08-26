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
    local ws_cmds="up stop kill rm code restart recreate reset dotfiles --rm --"

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
