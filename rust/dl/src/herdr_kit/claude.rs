//! The `~/.claude/settings.json` half of `dl --herdr-setup`.
//!
//! Claude Code rewrites this file itself, and other tools add hooks to it, so the
//! merge owns two things and nothing else:
//!
//! - **Its own Stop hook**, the one that runs `devlaunch-herdr-tab-title.sh`.
//!   Added when absent, corrected when it drifted, and kept to one copy.
//! - **Duplicates of herdr's SessionStart hook** (`herdr-agent-state.sh`), which
//!   older herdr releases appended once per `herdr integration install`. The
//!   last copy stays, because herdr appends its current form; the rest go.
//!
//! Every other key, event and hook passes through untouched, in its order. A
//! file the merge does not change is not rewritten, so its formatting is kept
//! exactly; a file it does change is written as Claude Code writes it, two-space
//! JSON with key order preserved.

use serde_json::{Map, Value, json};

/// The hook script's file name. Distinct from any name a user or herdr would
/// pick, because it is how the merge recognises its own entry.
pub(crate) const HOOK_NAME: &str = "devlaunch-herdr-tab-title.sh";
/// The script itself.
pub(crate) const HOOK_SCRIPT: &str = include_str!("assets/devlaunch-herdr-tab-title.sh");
/// herdr's own Claude hook, as `herdr integration install claude` writes it.
const HERDR_HOOK: &str = "herdr-agent-state.sh";
/// A tab-title hook installed some other way, such as from dotfiles.
const OTHER_TAB_TITLE_HOOK: &str = "herdr-tab-title.sh";
const TIMEOUT_SECONDS: u64 = 10;

/// The command the Stop hook runs: silent when the script is gone.
pub(crate) fn hook_command(script: &str) -> String {
    let quoted = super::shell_quote(script);
    format!("[ -x {quoted} ] && {quoted} || true")
}

/// What the merge found and would write.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Merged {
    /// The new file, or `None` when nothing changes.
    pub(crate) text: Option<String>,
    /// A tab-title hook from somewhere else is registered, so devlaunch's is not.
    pub(crate) other_tab_title: bool,
    /// Duplicate hook entries removed.
    pub(crate) removed: usize,
    /// devlaunch's Stop hook was added or corrected.
    pub(crate) registered: bool,
}

fn command_of(hook: &Value) -> &str {
    hook.get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn is_ours(hook: &Value) -> bool {
    command_of(hook).contains(HOOK_NAME)
}

fn is_other_tab_title(hook: &Value) -> bool {
    let command = command_of(hook);
    command.contains(OTHER_TAB_TITLE_HOOK) && !command.contains(HOOK_NAME)
}

/// Remove every hook in `groups` that `drop` selects, then any group it emptied.
fn remove_hooks(groups: &mut Vec<Value>, mut drop: impl FnMut(&Value) -> bool) -> usize {
    let mut removed = 0;
    groups.retain_mut(|group| {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|hook| !drop(hook));
        removed += before - hooks.len();
        !(before > 0 && hooks.is_empty())
    });
    removed
}

/// Merge devlaunch's Stop hook into `original` (`None`: no file yet).
pub(crate) fn merge(original: Option<&str>, command: &str) -> Result<Merged, String> {
    let before: Value = match original {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
            .map_err(|e| format!("settings.json is not valid JSON: {e}"))?,
        _ => Value::Object(Map::new()),
    };
    let mut after = before.clone();
    let root = after
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("settings.json's hooks is not an object")?;
    for (event, groups) in hooks.iter() {
        if !groups.is_array() {
            return Err(format!("settings.json's hooks.{event} is not an array"));
        }
    }
    let mut merged = Merged {
        other_tab_title: hooks.values().flat_map(hooks_in).any(is_other_tab_title),
        ..Merged::default()
    };

    // Our hook: at most one, in Stop, and none at all beside somebody else's.
    let keep_ours = !merged.other_tab_title;
    let mut kept = false;
    for (event, groups) in hooks.iter_mut() {
        let groups = groups.as_array_mut().expect("checked above");
        let in_stop = event == "Stop";
        merged.removed += remove_hooks(groups, |hook| {
            if !is_ours(hook) {
                return false;
            }
            if keep_ours && in_stop && !kept {
                kept = true;
                return false;
            }
            true
        });
    }
    if keep_ours {
        let desired = json!({"type": "command", "command": command, "timeout": TIMEOUT_SECONDS});
        let stop = hooks
            .entry("Stop")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("checked above");
        let existing = stop
            .iter_mut()
            .filter_map(|group| group.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
            .find(|hook| is_ours(hook));
        match existing {
            Some(hook) => {
                if *hook != desired {
                    *hook = desired;
                    merged.registered = true;
                }
            }
            None => {
                stop.push(json!({"matcher": "", "hooks": [desired]}));
                merged.registered = true;
            }
        }
    }

    // herdr's hook: the last copy in SessionStart, the one herdr wrote most recently, stays.
    if let Some(groups) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) {
        let is_herdrs = |hook: &Value| command_of(hook).contains(HERDR_HOOK);
        let mut stale = hooks_in(&Value::Array(groups.clone()))
            .filter(|hook| is_herdrs(hook))
            .count()
            .saturating_sub(1);
        merged.removed += remove_hooks(groups, |hook| {
            if !is_herdrs(hook) || stale == 0 {
                return false;
            }
            stale -= 1;
            true
        });
    }

    // A `hooks` object this merge created and left empty was never there.
    if before.get("hooks").is_none() && hooks.is_empty() {
        root.remove("hooks");
    }
    if after != before {
        let mut text = serde_json::to_string_pretty(&after).expect("a JSON value serialises");
        if original.is_none_or(|text| text.ends_with('\n')) {
            text.push('\n');
        }
        merged.text = Some(text);
    }
    Ok(merged)
}

fn hooks_in(groups: &Value) -> impl Iterator<Item = &Value> {
    groups
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str = "[ -x /h/.claude/hooks/devlaunch-herdr-tab-title.sh ] && /h/.claude/hooks/devlaunch-herdr-tab-title.sh || true";

    fn herdr_hook(matcher: &str) -> Value {
        json!({"matcher": matcher, "hooks": [{"type": "command", "command": "bash '/h/.claude/hooks/herdr-agent-state.sh' session", "timeout": 10}]})
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_missing_file_gets_only_the_stop_hook() {
        let merged = merge(None, COMMAND).unwrap();
        let settings = parse(merged.text.as_deref().unwrap());
        assert_eq!(
            settings,
            json!({"hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": COMMAND, "timeout": 10}]}]}})
        );
        assert!(merged.registered);
        assert!(merged.text.unwrap().ends_with('\n'));
    }

    #[test]
    fn foreign_keys_and_hooks_survive_in_their_order() {
        let original = json!({
            "model": "opus",
            "hooks": {
                "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "telemetry.sh"}]}],
                "Notification": [{"matcher": "", "hooks": [{"type": "command", "command": "notify"}]}]
            },
            "theme": "dark"
        });
        let text = serde_json::to_string_pretty(&original).unwrap();
        let merged = merge(Some(&text), COMMAND).unwrap();
        let settings = parse(merged.text.as_deref().unwrap());
        let keys: Vec<_> = settings.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "hooks", "theme"]);
        assert_eq!(
            settings["hooks"]["Notification"],
            original["hooks"]["Notification"]
        );
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop[0], original["hooks"]["Stop"][0]);
        assert_eq!(stop[1]["hooks"][0]["command"], COMMAND);
    }

    #[test]
    fn a_second_merge_changes_nothing() {
        let once = merge(Some("{\"model\": \"opus\"}"), COMMAND)
            .unwrap()
            .text
            .unwrap();
        assert_eq!(merge(Some(&once), COMMAND).unwrap(), Merged::default());
    }

    #[test]
    fn duplicates_of_our_hook_and_of_herdrs_are_removed() {
        let ours = json!({"matcher": "", "hooks": [{"type": "command", "command": COMMAND, "timeout": 10}]});
        let original = json!({"hooks": {
            "Stop": [ours.clone(), ours.clone()],
            "SessionStart": [herdr_hook("*"), herdr_hook("^(startup|resume)$"), {"matcher": "", "hooks": [{"type": "command", "command": "resource-check.sh"}]}],
            "Notification": [ours]
        }});
        let merged = merge(Some(&original.to_string()), COMMAND).unwrap();
        assert_eq!(merged.removed, 3);
        let settings = parse(merged.text.as_deref().unwrap());
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let start = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2);
        assert_eq!(start[0], herdr_hook("^(startup|resume)$"));
        assert_eq!(start[1]["hooks"][0]["command"], "resource-check.sh");
        assert_eq!(settings["hooks"]["Notification"], json!([]));
    }

    #[test]
    fn the_herdr_hook_written_last_survives_the_dedupe() {
        let stale = json!({"matcher": "*", "hooks": [{"type": "command", "command": "bash /h/.claude/hooks/herdr-agent-state.sh session", "timeout": 10}]});
        let current = herdr_hook("^(startup|resume|clear|compact|fork)$");
        let original = json!({"hooks": {"SessionStart": [stale, current.clone()]}});
        let merged = merge(Some(&original.to_string()), COMMAND).unwrap();
        assert_eq!(merged.removed, 1);
        let settings = parse(merged.text.as_deref().unwrap());
        assert_eq!(settings["hooks"]["SessionStart"], json!([current]));
    }

    #[test]
    fn a_tab_title_hook_from_dotfiles_is_left_to_run_alone() {
        let original = json!({"hooks": {"Stop": [{"matcher": "", "hooks": [
            {"type": "command", "command": "[ -x \"$HOME/.claude/hooks/herdr-tab-title.sh\" ] && \"$HOME/.claude/hooks/herdr-tab-title.sh\" || true"},
            {"type": "command", "command": COMMAND}
        ]}]}});
        let merged = merge(Some(&original.to_string()), COMMAND).unwrap();
        assert!(merged.other_tab_title);
        let settings = parse(merged.text.as_deref().unwrap());
        let hooks = settings["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(command_of(&hooks[0]).contains("/herdr-tab-title.sh"));
    }

    #[test]
    fn a_drifted_entry_is_corrected_in_place() {
        let original = json!({"hooks": {"Stop": [{"matcher": "", "hooks": [
            {"type": "command", "command": "/old/devlaunch-herdr-tab-title.sh"},
            {"type": "command", "command": "telemetry.sh"}
        ]}]}});
        let merged = merge(Some(&original.to_string()), COMMAND).unwrap();
        let settings = parse(merged.text.as_deref().unwrap());
        let hooks = settings["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(hooks[0]["command"], COMMAND);
        assert_eq!(hooks[1]["command"], "telemetry.sh");
    }

    #[test]
    fn unusable_settings_are_refused() {
        for original in [
            "[]",
            "{\"hooks\": []}",
            "{\"hooks\": {\"Stop\": {}}}",
            "{not json",
        ] {
            assert!(merge(Some(original), COMMAND).is_err(), "{original}");
        }
    }
}
