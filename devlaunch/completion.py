"""Utilities for installing shell autocompletion scripts."""

from __future__ import annotations

import logging
import os
import pathlib
from typing import Optional

from .completion_loader import load_completion_script

_RC_BLOCK_START = "# >>> devlaunch completions >>>"
_RC_BLOCK_END = "# <<< devlaunch completions <<<"

# Legacy blocks to clean up from previous installations
_LEGACY_BLOCKS = {
    "# dl completion": "# end dl completion",
    "# dp completion": "# end dp completion",
    _RC_BLOCK_START: _RC_BLOCK_END,
}

_LEGACY_SINGLE_LINES = {
    "complete -F _dl_completion dl",
    "complete -F _dp_completion dp",
}


def _completion_file_path() -> pathlib.Path:
    """Return the path where the completion script should be stored."""
    override = os.environ.get("DEVLAUNCH_COMPLETION_FILE")
    if override:
        return pathlib.Path(override).expanduser()
    return pathlib.Path.home() / ".config" / "devlaunch" / "completions.sh"


def install_completions(rc_path: Optional[pathlib.Path] = None) -> int:
    """Install or refresh completion scripts for dl."""
    completion_path = _completion_file_path().expanduser()
    rc_target = (rc_path if rc_path is not None else pathlib.Path.home() / ".bashrc").expanduser()

    try:
        completion_path.parent.mkdir(parents=True, exist_ok=True)

        # Write the dl completion script
        completion_script = load_completion_script("dl").rstrip() + "\n"
        completion_path.write_text(completion_script, encoding="utf-8")
        logging.info("Wrote completion script to %s", completion_path)

        rc_target.parent.mkdir(parents=True, exist_ok=True)
        existing_content = ""
        if rc_target.exists():
            existing_content = rc_target.read_text(encoding="utf-8")

        # Remove any existing completion block and legacy blocks before appending
        def remove_block(text: str, start: str, end: str) -> str:
            lines = text.splitlines()
            out = []
            skip = False
            for line in lines:
                if line.strip() == start:
                    skip = True
                    continue
                if skip and line.strip() == end:
                    skip = False
                    continue
                if not skip:
                    out.append(line)
            return "\n".join(out)

        # Remove main block
        cleaned_content = remove_block(existing_content, _RC_BLOCK_START, _RC_BLOCK_END)
        # Remove legacy blocks
        for block_start, block_end in _LEGACY_BLOCKS.items():
            cleaned_content = remove_block(cleaned_content, block_start, block_end)
        # Remove legacy single lines
        cleaned_lines = []
        for line in cleaned_content.splitlines():
            if not any(line.strip().startswith(single) for single in _LEGACY_SINGLE_LINES):
                cleaned_lines.append(line)
        cleaned_content = "\n".join(cleaned_lines).strip()
        escaped_path = str(completion_path).replace('"', r"\"")
        source_line = f'source "{escaped_path}"'
        block = "\n".join([_RC_BLOCK_START, source_line, _RC_BLOCK_END])

        if cleaned_content:
            new_content = f"{cleaned_content}\n\n{block}\n"
        else:
            new_content = f"{block}\n"

        rc_target.write_text(new_content, encoding="utf-8")
        logging.info("Added completion source block to %s", rc_target)
        logging.info("Run 'source %s' or restart your terminal to enable completion", rc_target)
        print(
            "[devlaunch] Autocomplete has been updated. Run 'source %s' or restart your terminal to enable completion."
            % rc_target
        )
        return 0
    except OSError as error:
        logging.error("Failed to install completions: %s", error)
        return 1
