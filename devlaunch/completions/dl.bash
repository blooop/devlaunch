# dl completion
_dl_completion() {
    local cur prev opts
    COMPREPLY=()

    # Extract current word from COMP_LINE instead of COMP_WORDS
    # This avoids issues with COMP_WORDBREAKS treating dashes as word boundaries
    local line="${COMP_LINE:0:COMP_POINT}"
    # Get current word: everything after the last space (or the whole line if no space)
    if [[ "$line" =~ [[:space:]]([^[:space:]]*)$ ]]; then
        cur="${BASH_REMATCH[1]}"
    elif [[ "$line" =~ ^([^[:space:]]*)$ ]]; then
        cur="${BASH_REMATCH[1]}"
    else
        cur=""
    fi

    # Get previous word similarly
    local before_cur="${line% *}"
    if [[ "$before_cur" == "$line" ]]; then
        prev=""
    elif [[ "$before_cur" =~ [[:space:]]([^[:space:]]*)$ ]]; then
        prev="${BASH_REMATCH[1]}"
    else
        prev="${before_cur##* }"
    fi

    # Count actual words (space-separated) for position detection
    local word_count
    word_count=$(echo "$line" | awk '{print NF}')
    # If line ends with space, we're starting a new word
    if [[ "$line" =~ [[:space:]]$ ]]; then
        ((word_count++))
        cur=""
    fi

    # Global command options (only valid as first arg)
    local global_opts="--ls --install --help -h --version"

    # Workspace subcommands
    local ws_cmds="stop rm code restart recreate reset --"

    # Cache file location (honors XDG_CACHE_HOME)
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/dl"
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

        # Default: complete workspace names and offer owner/ completion
        compopt -o nospace  # For owner/ completions
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

    # Second argument (after workspace): subcommands
    if [[ ${word_count} -eq 3 ]]; then
        # Don't complete after global flags
        # Extract the first argument (word after "dl")
        local first
        first=$(echo "$line" | awk '{print $2}')
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
# end dl completion
