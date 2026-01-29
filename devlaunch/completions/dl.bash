# dl completion
# Note: This completion function does not support quoted arguments or escaped spaces.
# All arguments are treated as literal strings separated by whitespace.
# This is acceptable because GitHub usernames, repo names, and workspace names
# do not contain spaces or special characters that would require quoting.
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
# end dl completion
