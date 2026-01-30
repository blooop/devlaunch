# Plan: Fix List Argument Handling in rockerc.yaml

## Issue Reference

- **Repository**: blooop/rockerc
- **Issue**: [#120](https://github.com/blooop/rockerc/issues/120)
- **Specific Comment**: [issuecomment-3824380588](https://github.com/blooop/rockerc/issues/120#issuecomment-3824380588)

## Problem Statement

When using list values in `rockerc.yaml` configuration files, the arguments are incorrectly formatted. For example:

```yaml
devices:
  - /dev/dri
  - /dev/ttyACM0
```

**Current (incorrect) output:**
```
rocker --x11 --user --home --devices [/dev/dri, /dev/ttyACM0]
```

**Expected output:**
```
rocker --x11 --user --home --devices /dev/dri --devices /dev/ttyACM0
```

## Root Cause Analysis

The bug is located in the `yaml_dict_to_args()` function in `rockerc/rockerc.py` (around line 355).

The current implementation uses `str(v)` to convert values to strings:

```python
for k, v in d.items():
    segments.extend([f"--{k}", str(v)])
```

When `v` is a Python list, `str(v)` produces the literal string representation `[/dev/dri, /dev/ttyACM0]` instead of properly expanding the list into multiple arguments.

## Proposed Solution

Modify the `yaml_dict_to_args()` function to detect list values and handle them by repeating the flag for each item in the list.

### Implementation

```python
def yaml_dict_to_args(d: dict, extra_args: str = "") -> str:
    """Given a dictionary of arguments turn it into an argument string to pass to rocker."""
    image = d.pop("image", None)
    segments = []

    # explicit flags
    for a in d.pop("args", []):
        segments.append(f"--{a}")

    # key/value pairs
    for k, v in d.items():
        if isinstance(v, list):
            # Handle list values by repeating the flag for each item
            for item in v:
                segments.extend([f"--{k}", str(item)])
        else:
            segments.extend([f"--{k}", str(v)])

    # ... rest of function
```

## Tasks

### 1. Code Changes
- [ ] Fork/clone the rockerc repository
- [ ] Locate `yaml_dict_to_args()` function in `rockerc/rockerc.py`
- [ ] Add list type checking with `isinstance(v, list)`
- [ ] Iterate over list items and append each as a separate `--key value` pair

### 2. Testing
- [ ] Add unit tests for list argument handling
- [ ] Test with various YAML configurations:
  - Single device: `devices: /dev/dri`
  - Multiple devices as list:
    ```yaml
    devices:
      - /dev/dri
      - /dev/ttyACM0
    ```
  - Other list-capable arguments (volumes, env vars, etc.)
- [ ] Verify backward compatibility with existing non-list configurations

### 3. Documentation
- [ ] Update examples to show proper list syntax
- [ ] Add a note in documentation about list argument support
- [ ] Consider adding an example file specifically for multi-device setups

### 4. PR Submission
- [ ] Create a new branch for the fix
- [ ] Commit changes with clear commit message
- [ ] Open PR referencing issue #120 and the specific comment
- [ ] Include before/after examples in PR description

## Edge Cases to Consider

1. **Empty lists**: Should produce no arguments for that key
2. **Single-item lists**: Should work identically to non-list value
3. **Nested lists**: Should either flatten or raise a clear error
4. **Mixed types in list**: Each item should be converted via `str()`
5. **Boolean values in lists**: May need special handling (flags vs key-value)

## Acceptance Criteria

1. YAML list values are correctly expanded into repeated flags
2. Existing non-list configurations continue to work unchanged
3. Unit tests cover list argument handling
4. Documentation is updated with examples

## Additional Notes

- PR #136 closed the original issue but only addressed documentation, not this bug
- The fix follows Docker's convention of repeating flags for multiple values
- This pattern is common across Docker-related tools (docker-compose, etc.)
