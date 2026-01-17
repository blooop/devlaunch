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

    # Cache file location (honor XDG_CACHE_HOME)
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/dl"
    local cache_file="$cache_dir/completions.json"

    # Read from cache (fast path) or fall back to CLI
    local workspaces=""
    local known_repos=""
    local owners=""

    if command -v jq >/dev/null 2>&1 && [[ -f "$cache_file" ]]; then
        # Fast path: use cached JSON if jq is available
        workspaces=$(jq -r '.workspaces[]?' "$cache_file" 2>/dev/null | tr '\n' ' ')
        known_repos=$(jq -r '.repos[]?' "$cache_file" 2>/dev/null | tr '\n' ' ')
        owners=$(jq -r '.owners[]?' "$cache_file" 2>/dev/null | tr '\n' ' ')
    elif command -v dl >/dev/null 2>&1; then
        # Fallback: use dl CLI directly (slower but works without jq)
        local _dl_repos
        _dl_repos=$(dl --repos 2>/dev/null || true)
        if [[ -n "$_dl_repos" ]]; then
            known_repos=$(printf '%s\n' "$_dl_repos" | tr '\n' ' ')
            owners=$(printf '%s\n' "$_dl_repos" | cut -d'/' -f1 | sort -u | tr '\n' ' ')
        fi
        # For workspaces, fall back to dl --ls parsing (basic)
        workspaces=$(dl --ls 2>/dev/null | tail -n +3 | awk '{print $1}' | tr '\n' ' ' || true)
    fi

    # Commands that need workspace completion
    if [[ "$prev" == "--stop" || "$prev" == "--rm" || "$prev" == "--code" || "$prev" == "--status" || "$prev" == "--recreate" || "$prev" == "--reset" ]]; then
        if [[ -n "$workspaces" ]]; then
            COMPREPLY=( $(compgen -W "${workspaces}" -- ${cur}) )
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
            if [[ -n "$known_repos" ]]; then
                COMPREPLY=( $(compgen -W "${known_repos}" -- ${cur}) )
            fi
            return 0
        fi

        # Default: complete workspace names and offer owner/ completion
        local completions="$workspaces"

        # Add owners with trailing slash
        for owner in $owners; do
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
