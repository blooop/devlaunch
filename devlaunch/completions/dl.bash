# dl completion
_dl_completion() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    # Command options
    opts="--ls --repos --stop --rm --code --status --recreate --reset --install --help"

    # Flag completion
    if [[ ${cur} == -* ]]; then
        COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
        return 0
    fi

    # Cache file location (honors XDG_CACHE_HOME)
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/dl"
    local cache_file="$cache_dir/completions.bash"

    # Initialize completion variables
    local DL_WORKSPACES=""
    local DL_REPOS=""
    local DL_OWNERS=""

    # Source the bash cache file (fast, no jq needed)
    if [[ -f "$cache_file" ]]; then
        source "$cache_file"
    fi

    # Commands that need workspace completion
    if [[ "$prev" == "--stop" || "$prev" == "--rm" || "$prev" == "--code" || "$prev" == "--status" || "$prev" == "--recreate" || "$prev" == "--reset" ]]; then
        if [[ -n "$DL_WORKSPACES" ]]; then
            COMPREPLY=( $(compgen -W "${DL_WORKSPACES}" -- ${cur}) )
        fi
        return 0
    fi

    # First positional argument: workspace, owner/repo, or path
    if [[ ${COMP_CWORD} -eq 1 ]]; then
        # Don't add space after completion to allow @branch suffix
        compopt -o nospace

        # If typing a path, complete files/directories
        if [[ "$cur" == ./* || "$cur" == /* || "$cur" == ~/* ]]; then
            compopt +o nospace
            COMPREPLY=( $(compgen -d -- ${cur}) )
            return 0
        fi

        # Check if completing owner/repo format (contains /)
        if [[ "$cur" == */* ]]; then
            # Complete from known repos
            if [[ -n "$DL_REPOS" ]]; then
                COMPREPLY=( $(compgen -W "${DL_REPOS}" -- ${cur}) )
            fi
            return 0
        fi

        # Default: complete workspace names and offer owner/ completion
        local completions="$DL_WORKSPACES"

        # Add owners with trailing slash
        for owner in $DL_OWNERS; do
            completions="$completions ${owner}/"
        done

        if [[ -n "$completions" ]]; then
            COMPREPLY=( $(compgen -W "${completions}" -- ${cur}) )
        fi
        return 0
    fi

    return 0
}

complete -F _dl_completion dl
# end dl completion
