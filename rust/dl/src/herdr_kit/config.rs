//! The Herdr config half of `dl --herdr-setup`: a merge, never a replacement.
//!
//! Two kinds of key, and the difference is who a value belongs to.
//!
//! **Managed** keys point at files this setup installs, so a path that has moved
//! is a broken config rather than a preference: `terminal.default_shell`, the
//! `prefix+a` binding for the agent-queue plugin, the `status.sh` segment in
//! `ui.tab_bar_right`, and the ` · herdr` end of `ui.window_title`. They are set
//! on every run. Even so, each one stops short of a value that is plainly the
//! user's: a custom `default_shell` is refused rather than replaced, a
//! `status.sh` of the user's own keeps its segment, and a `prefix+a` bound to
//! something else keeps its binding.
//!
//! **Default** keys are everything else in `assets/config.toml`, the keymap,
//! theme, sidebar and toasts devlaunch ships. Each is added only where the key
//! is absent, so a value the user set, including one that matches herdr's own
//! default, is never overwritten.
//!
//! An empty or missing config is the one case that is not a merge: it gets the
//! packaged file verbatim, comments and all, with the managed paths filled in.

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// The config devlaunch ships, and the source of every default key.
pub(crate) const PACKAGED: &str = include_str!("assets/config.toml");
/// The plugin action `prefix+a` runs.
pub(crate) const AGENT_QUEUE_ACTION: &str = "local.agent-queue.toggle";
/// The end of `ui.window_title` a window manager rule can match on.
pub(crate) const TITLE_SUFFIX: &str = " · herdr";
/// The status command `assets/config.toml` spells, before the real path replaces it.
const STATUS_PLACEHOLDER: &str = "~/.local/share/devlaunch/herdr/status.sh";

/// The paths the managed keys point at, resolved for this machine.
pub(crate) struct Targets {
    /// The pane shell, for `terminal.default_shell`.
    pub(crate) shell: String,
    /// The older launcher a `default_shell` may still name, which is replaced.
    pub(crate) prototype_shell: String,
    /// The installed `status.sh`, for the tab-bar segment.
    pub(crate) status: String,
}

/// What a merge would change.
#[derive(Debug, Default)]
pub(crate) struct Merged {
    /// The whole new file.
    pub(crate) text: String,
    /// The config was empty, and is now the packaged file.
    pub(crate) fresh: bool,
    /// Default keys that were absent and are now set, as dotted paths.
    pub(crate) added: Vec<String>,
    /// Managed keys that were set or corrected.
    pub(crate) managed: Vec<String>,
    /// Managed keys left alone, and why.
    pub(crate) kept: Vec<String>,
}

/// Merge the packaged config into `original`.
///
/// Refuses, changing nothing, for a config that does not parse and for a custom
/// `default_shell`: both need a person, and a guess costs every pane.
pub(crate) fn merge(original: &str, targets: &Targets) -> Result<Merged, String> {
    let packaged = packaged();
    let mut merged = Merged::default();
    let mut document = if original.trim().is_empty() {
        merged.fresh = true;
        packaged.clone()
    } else {
        let mut document = original
            .parse::<DocumentMut>()
            .map_err(|e| format!("invalid Herdr config: {e}"))?;
        let mut next = last_position(document.as_table()) + 1;
        let bound = bound_keys(document.as_table());
        fill_defaults(
            document.as_table_mut(),
            packaged.as_table(),
            "",
            &bound,
            &mut next,
            &mut merged.added,
        );
        document
    };
    let mut next = last_position(document.as_table()) + 1;
    set_default_shell(&mut document, targets, &mut merged)?;
    set_status_segment(&mut document, targets, &mut merged);
    set_agent_queue_binding(&mut document, &packaged, &mut next, &mut merged);
    set_window_title(&mut document, &mut merged);
    merged.text = document.to_string();
    Ok(merged)
}

fn packaged() -> DocumentMut {
    PACKAGED
        .parse()
        .expect("assets/config.toml is valid TOML; a unit test parses it")
}

/// The largest position any table in `table` holds, so appended tables go last.
fn last_position(table: &Table) -> isize {
    let mut last = table.position().unwrap_or(0);
    for (_, item) in table.iter() {
        match item {
            Item::Table(child) => last = last.max(last_position(child)),
            Item::ArrayOfTables(array) => {
                for child in array.iter() {
                    last = last.max(last_position(child));
                }
            }
            _ => {}
        }
    }
    last
}

/// Give `item`'s tables positions from `next` onward, in document order.
///
/// A cloned table keeps the position it had in the packaged file, and toml_edit
/// sorts every table in a document by position when it writes one out. So an
/// inserted table left alone would land among the user's tables wherever its old
/// number happened to fall.
fn renumber(item: &mut Item, next: &mut isize) {
    match item {
        Item::Table(table) => renumber_table(table, next),
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                renumber_table(table, next);
            }
        }
        _ => {}
    }
}

fn renumber_table(table: &mut Table, next: &mut isize) {
    table.set_position(*next);
    *next += 1;
    for (_, child) in table.iter_mut() {
        renumber(child, next);
    }
}

/// Every key string the user's `[keys]` binds, to an action or a `[[keys.command]]`.
fn bound_keys(root: &Table) -> Vec<String> {
    let Some(keys) = root.get("keys").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut bound = Vec::new();
    for (_, item) in keys.iter() {
        match item {
            Item::Value(Value::String(key)) => bound.push(key.value().to_owned()),
            Item::Value(Value::Array(array)) => {
                bound.extend(array.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            Item::ArrayOfTables(commands) => bound.extend(
                commands
                    .iter()
                    .filter_map(|entry| entry.get("key").and_then(Item::as_str))
                    .map(str::to_owned),
            ),
            _ => {}
        }
    }
    bound
}

/// `default` without the keys in `bound`, or `None` when it keeps no key at all.
fn unclaimed(default: &Item, bound: &[String]) -> Option<Item> {
    let free = |key: &str| !bound.iter().any(|mine| mine == key);
    match default {
        Item::Value(Value::String(key)) => free(key.value()).then(|| default.clone()),
        Item::Value(Value::Array(array)) => {
            let mut array = array.clone();
            array.retain(|key| key.as_str().is_none_or(free));
            (!array.is_empty()).then(|| Item::Value(Value::Array(array)))
        }
        Item::ArrayOfTables(commands) => {
            let mut kept = ArrayOfTables::new();
            for entry in commands.iter() {
                if entry.get("key").and_then(Item::as_str).is_none_or(free) {
                    kept.push(entry.clone());
                }
            }
            (!kept.is_empty()).then_some(Item::ArrayOfTables(kept))
        }
        _ => Some(default.clone()),
    }
}

fn fill_defaults(
    user: &mut Table,
    defaults: &Table,
    prefix: &str,
    bound: &[String],
    next: &mut isize,
    added: &mut Vec<String>,
) {
    let keymap = prefix == "keys.";
    for (key, default) in defaults.iter() {
        let path = format!("{prefix}{key}");
        match (user.get_mut(key), default) {
            (None, _) => {
                let item = if keymap {
                    unclaimed(default, bound)
                } else {
                    Some(default.clone())
                };
                let Some(mut item) = item else {
                    continue;
                };
                renumber(&mut item, next);
                user.insert(key, item);
                added.push(path);
            }
            (Some(Item::Table(existing)), Item::Table(default)) => {
                fill_defaults(existing, default, &format!("{path}."), bound, next, added);
            }
            (Some(Item::ArrayOfTables(existing)), Item::ArrayOfTables(default)) => {
                for entry in default.iter() {
                    let claimed = keymap
                        && entry
                            .get("key")
                            .and_then(Item::as_str)
                            .is_some_and(|key| bound.iter().any(|mine| mine == key));
                    if !claimed && !existing.iter().any(|mine| same_binding(mine, entry)) {
                        push_entry(existing, entry.clone(), next);
                        added.push(format!("{path} {}", describe_binding(entry)));
                    }
                }
            }
            // Present in any other shape: the user's value, whatever it is.
            (Some(_), _) => {}
        }
    }
}

/// Two `[[keys.command]]` entries are one binding when they share a key or a command.
fn same_binding(one: &Table, other: &Table) -> bool {
    let field =
        |table: &Table, name: &str| table.get(name).and_then(Item::as_str).map(str::to_owned);
    let key = field(one, "key");
    let command = field(one, "command");
    (key.is_some() && key == field(other, "key"))
        || (command.is_some() && command == field(other, "command"))
}

fn describe_binding(entry: &Table) -> String {
    entry
        .get("key")
        .and_then(Item::as_str)
        .unwrap_or("?")
        .to_owned()
}

/// Append `entry` to `array` just after its current last entry.
fn push_entry(array: &mut ArrayOfTables, mut entry: Table, next: &mut isize) {
    match array.iter().last().and_then(Table::position) {
        // The same position as the last entry: the sort is stable, so the new one
        // is written straight after it rather than at the end of the file.
        Some(position) => entry.set_position(position),
        None => {
            entry.set_position(*next);
            *next += 1;
        }
    }
    array.push(entry);
}

/// Replace a string value, keeping the whitespace and comment around it.
fn set_string(value: &mut Value, text: &str) {
    let decor = value.decor().clone();
    *value = Value::from(text);
    *value.decor_mut() = decor;
}

fn table_like<'d>(document: &'d mut DocumentMut, key: &str) -> Option<&'d mut dyn TableLike> {
    document
        .entry(key)
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
}

fn set_default_shell(
    document: &mut DocumentMut,
    targets: &Targets,
    merged: &mut Merged,
) -> Result<(), String> {
    let terminal =
        table_like(document, "terminal").ok_or("Herdr terminal config is not a table")?;
    match terminal.get_mut("default_shell") {
        None => {
            terminal.insert("default_shell", toml_edit::value(targets.shell.as_str()));
        }
        Some(item) => {
            let value = item
                .as_value_mut()
                .filter(|value| value.is_str())
                .ok_or("Herdr default_shell is not a string")?;
            let current = value.as_str().unwrap_or_default();
            if current == targets.shell {
                return Ok(());
            }
            if !current.is_empty()
                && current != crate::pane_shell::NAME
                && current != targets.prototype_shell
            {
                return Err(format!(
                    "Herdr uses a custom default_shell ({current}); set terminal.default_shell to `{}` manually to replace it",
                    targets.shell
                ));
            }
            set_string(value, &targets.shell);
        }
    }
    merged.managed.push("terminal.default_shell".to_owned());
    Ok(())
}

fn status_segment(status: &str) -> Value {
    let mut segment = InlineTable::new();
    segment.insert("type", "command".into());
    segment.insert("command", status.into());
    segment.insert("interval_seconds", 5.into());
    segment.insert("timeout_seconds", 3.into());
    Value::InlineTable(segment)
}

fn set_status_segment(document: &mut DocumentMut, targets: &Targets, merged: &mut Merged) {
    let Some(ui) = table_like(document, "ui") else {
        merged
            .kept
            .push("ui is not a table, so the status segment was not added".to_owned());
        return;
    };
    let Some(item) = ui.get_mut("tab_bar_right") else {
        let mut bar = toml_edit::Array::new();
        bar.push(status_segment(&targets.status));
        ui.insert("tab_bar_right", toml_edit::value(bar));
        merged.managed.push("ui.tab_bar_right".to_owned());
        return;
    };
    let Some(bar) = item.as_array_mut() else {
        merged.kept.push(
            "ui.tab_bar_right is not an array, so the status segment was not added".to_owned(),
        );
        return;
    };
    let mut theirs = false;
    for segment in bar.iter_mut() {
        let Some(command) = segment
            .as_inline_table_mut()
            .and_then(|table| table.get_mut("command"))
        else {
            continue;
        };
        let Some(current) = command.as_str() else {
            continue;
        };
        if current == targets.status {
            return;
        }
        if current == STATUS_PLACEHOLDER {
            set_string(command, &targets.status);
            merged
                .managed
                .push("ui.tab_bar_right status segment".to_owned());
            return;
        }
        theirs |= current.ends_with("status.sh");
    }
    if theirs {
        merged.kept.push(
            "ui.tab_bar_right already runs a status.sh of your own, so devlaunch's was not added"
                .to_owned(),
        );
        return;
    }
    bar.push(status_segment(&targets.status));
    merged
        .managed
        .push("ui.tab_bar_right status segment".to_owned());
}

fn set_agent_queue_binding(
    document: &mut DocumentMut,
    packaged: &DocumentMut,
    next: &mut isize,
    merged: &mut Merged,
) {
    let ours = packaged["keys"]["command"]
        .as_array_of_tables()
        .and_then(|array| {
            array.iter().find(|entry| {
                entry.get("command").and_then(Item::as_str) == Some(AGENT_QUEUE_ACTION)
            })
        })
        .expect("assets/config.toml binds the agent-queue toggle")
        .clone();
    let bound = bound_keys(document.as_table());
    let Some(keys) = document
        .entry("keys")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    else {
        merged
            .kept
            .push("keys is not a table, so prefix+a was not bound".to_owned());
        return;
    };
    let commands = keys
        .entry("command")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    let Some(commands) = commands.as_array_of_tables_mut() else {
        merged.kept.push(
            "keys.command is not written as [[keys.command]] tables, so prefix+a was not bound"
                .to_owned(),
        );
        return;
    };
    let existing = commands
        .iter_mut()
        .find(|entry| entry.get("command").and_then(Item::as_str) == Some(AGENT_QUEUE_ACTION));
    if let Some(entry) = existing {
        if entry.get("type").and_then(Item::as_str) != Some("plugin_action") {
            entry.insert("type", toml_edit::value("plugin_action"));
            merged
                .managed
                .push("keys.command agent-queue toggle".to_owned());
        }
        return;
    }
    let key = ours
        .get("key")
        .and_then(Item::as_str)
        .unwrap_or_default()
        .to_owned();
    if bound.contains(&key) {
        merged.kept.push(format!(
            "{key} is bound to something else, so the agent-queue toggle ({AGENT_QUEUE_ACTION}) has no key"
        ));
        return;
    }
    push_entry(commands, ours, next);
    merged
        .managed
        .push(format!("keys.command {key} agent-queue toggle"));
}

fn set_window_title(document: &mut DocumentMut, merged: &mut Merged) {
    let Some(ui) = table_like(document, "ui") else {
        return;
    };
    match ui.get_mut("window_title") {
        None => {
            ui.insert(
                "window_title",
                toml_edit::value(format!("{{hostname}}: {{workspace}}{TITLE_SUFFIX}")),
            );
        }
        Some(item) => {
            let Some(value) = item.as_value_mut().filter(|value| value.is_str()) else {
                merged
                    .kept
                    .push("ui.window_title is not a string, so it was left alone".to_owned());
                return;
            };
            let current = value.as_str().unwrap_or_default().to_owned();
            if current.ends_with(TITLE_SUFFIX) {
                return;
            }
            set_string(value, &format!("{current}{TITLE_SUFFIX}"));
        }
    }
    merged.managed.push("ui.window_title".to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Targets {
        Targets {
            shell: "/home/u/.local/bin/dl-herdr-shell".to_owned(),
            prototype_shell: "/home/u/.local/bin/herdr-workspace-shell".to_owned(),
            status: "/home/u/.local/share/devlaunch/herdr/status.sh".to_owned(),
        }
    }

    fn parse(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    fn commands(document: &DocumentMut) -> Vec<(String, String)> {
        document["keys"]["command"]
            .as_array_of_tables()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["key"].as_str().unwrap().to_owned(),
                    entry["command"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_packaged_config_parses_and_binds_nothing_personal() {
        let document = packaged();
        let bound = commands(&document);
        assert!(
            bound
                .iter()
                .any(|(key, command)| key == "prefix+a" && command == AGENT_QUEUE_ACTION)
        );
        assert!(
            bound
                .iter()
                .all(|(_, command)| !command.contains("/.local/bin/")),
            "a binding points at a script devlaunch does not ship: {bound:?}"
        );
        assert!(!PACKAGED.contains("zja"));
        assert!(PACKAGED.contains(STATUS_PLACEHOLDER));
    }

    #[test]
    fn an_empty_config_gets_the_packaged_file_with_real_paths() {
        for original in ["", "\n  \n"] {
            let merged = merge(original, &targets()).unwrap();
            let document = parse(&merged.text);
            assert_eq!(
                document["terminal"]["default_shell"].as_str(),
                Some(targets().shell.as_str())
            );
            assert!(
                merged
                    .text
                    .contains("# Herdr config written by `dl --herdr-setup`")
            );
            assert!(merged.text.contains(&targets().status));
            assert!(!merged.text.contains(STATUS_PLACEHOLDER));
            assert_eq!(document["keys"]["prefix"].as_str(), Some("ctrl+space"));
            assert!(
                document["ui"]["window_title"]
                    .as_str()
                    .unwrap()
                    .ends_with(TITLE_SUFFIX)
            );
        }
    }

    #[test]
    fn default_keys_fill_gaps_and_never_replace_a_user_value() {
        let original = "# mine\n[keys]\nprefix = \"ctrl+b\" # muscle memory\nnew_tab = \"prefix+c\"\n\n[theme.custom]\ntext = \"#cccccc\"\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        // A root key has to precede every table, so `onboarding` lands above the
        // comment that belongs to `[keys]`, and the comment stays with its table.
        assert!(merged.text.contains("# mine\n[keys]\n"), "{}", merged.text);
        assert!(merged.text.contains("prefix = \"ctrl+b\" # muscle memory"));
        assert_eq!(document["keys"]["new_tab"].as_str(), Some("prefix+c"));
        assert_eq!(
            document["theme"]["custom"]["text"].as_str(),
            Some("#cccccc")
        );
        // Absent keys come from the package.
        assert_eq!(
            document["theme"]["custom"]["surface1"].as_str(),
            Some("#9aa0b6")
        );
        assert!(document["keys"]["next_agent"].is_array());
        assert_eq!(
            document["ui"]["toast"]["delay_seconds"].as_integer(),
            Some(2)
        );
        assert!(merged.added.contains(&"theme.custom.surface1".to_owned()));
        assert!(!merged.added.iter().any(|path| path == "keys.prefix"));
    }

    fn keys_of(document: &DocumentMut, action: &str) -> Vec<String> {
        match document["keys"].get(action) {
            None => Vec::new(),
            Some(item) => match item.as_str() {
                Some(key) => vec![key.to_owned()],
                None => item
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|key| key.as_str().unwrap().to_owned())
                    .collect(),
            },
        }
    }

    #[test]
    fn a_default_action_never_takes_a_key_the_user_already_bound() {
        let merged = merge("[keys]\nzoom = \"f6\"\n", &targets()).unwrap();
        let document = parse(&merged.text);
        assert_eq!(keys_of(&document, "zoom"), ["f6"]);
        assert_eq!(keys_of(&document, "rename_tab"), ["prefix+shift+t"]);

        let original =
            "[[keys.command]]\nkey = \"f7\"\ntype = \"shell\"\ncommand = \"echo mine\"\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        assert_eq!(
            keys_of(&document, "next_agent"),
            ["prefix+period", "ctrl+period"]
        );
        assert!(
            commands(&document)
                .iter()
                .all(|(key, command)| key != "f7" || command == "echo mine")
        );

        let merged = merge(
            "[keys]\nsplit_vertical = [\"prefix+g\", \"shift+f1\"]\nhelp = \"prefix+t\"\n",
            &targets(),
        )
        .unwrap();
        let document = parse(&merged.text);
        assert!(keys_of(&document, "goto").is_empty(), "{}", merged.text);
        assert!(!merged.added.iter().any(|path| path == "keys.goto"));
        assert!(!commands(&document).iter().any(|(key, _)| key == "prefix+t"));
    }

    #[test]
    fn inserted_tables_go_after_the_users_tables() {
        let original = "[keys]\nprefix = \"ctrl+b\"\n\n[terminal]\nfont_size = 14\n";
        let merged = merge(original, &targets()).unwrap();
        let terminal = merged.text.find("[terminal]").unwrap();
        let keys = merged.text.find("[keys]").unwrap();
        let theme = merged.text.find("[theme.custom]").unwrap();
        let ui = merged.text.find("[ui]").unwrap();
        assert!(
            keys < terminal && terminal < theme && theme < ui,
            "{}",
            merged.text
        );
    }

    #[test]
    fn managed_keys_are_set_even_when_the_user_had_a_value() {
        let original =
            "[ui]\nwindow_title = \"{workspace}\"\ntab_bar_right = [{ type = \"hostname\" }]\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        assert_eq!(
            document["ui"]["window_title"].as_str(),
            Some("{workspace} · herdr")
        );
        let bar = document["ui"]["tab_bar_right"].as_array().unwrap();
        assert_eq!(bar.len(), 2, "{bar:?}");
        assert_eq!(
            bar.get(1).unwrap().as_inline_table().unwrap()["command"].as_str(),
            Some(targets().status.as_str())
        );
        assert!(
            commands(&document)
                .iter()
                .any(|(_, command)| command == AGENT_QUEUE_ACTION)
        );
    }

    #[test]
    fn a_status_script_of_the_users_own_keeps_its_segment() {
        let original = "[ui]\ntab_bar_right = [{ type = \"command\", command = \"~/.config/herdr/status.sh\" }]\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        assert_eq!(document["ui"]["tab_bar_right"].as_array().unwrap().len(), 1);
        assert!(
            merged
                .kept
                .iter()
                .any(|note| note.contains("status.sh of your own"))
        );
    }

    #[test]
    fn prefix_a_bound_to_a_plain_action_gets_no_toggle() {
        let merged = merge("[keys]\nzoom = \"prefix+a\"\n", &targets()).unwrap();
        let document = parse(&merged.text);
        assert_eq!(keys_of(&document, "zoom"), ["prefix+a"]);
        assert!(
            !commands(&document).iter().any(|(key, _)| key == "prefix+a"),
            "{}",
            merged.text
        );
        assert!(merged.kept.iter().any(|note| note.contains("prefix+a")));
    }

    #[test]
    fn prefix_a_bound_elsewhere_is_left_bound_and_reported() {
        let original =
            "[[keys.command]]\nkey = \"prefix+a\"\ntype = \"shell\"\ncommand = \"echo mine\"\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        let bound = commands(&document);
        assert!(bound.contains(&("prefix+a".to_owned(), "echo mine".to_owned())));
        assert!(
            !bound
                .iter()
                .any(|(_, command)| command == AGENT_QUEUE_ACTION)
        );
        assert!(merged.kept.iter().any(|note| note.contains("prefix+a")));
        // The other packaged bindings still arrive, straight after the user's.
        assert!(bound.iter().any(|(key, _)| key == "prefix+t"));
    }

    #[test]
    fn the_toggle_on_another_key_is_kept_and_not_duplicated() {
        let original = "[[keys.command]]\nkey = \"f8\"\ntype = \"shell\"\ncommand = \"local.agent-queue.toggle\"\n";
        let merged = merge(original, &targets()).unwrap();
        let document = parse(&merged.text);
        let toggles: Vec<_> = document["keys"]["command"]
            .as_array_of_tables()
            .unwrap()
            .iter()
            .filter(|entry| entry["command"].as_str() == Some(AGENT_QUEUE_ACTION))
            .collect();
        assert_eq!(toggles.len(), 1);
        assert_eq!(toggles[0]["key"].as_str(), Some("f8"));
        assert_eq!(toggles[0]["type"].as_str(), Some("plugin_action"));
    }

    #[test]
    fn a_second_merge_changes_nothing() {
        for original in [
            "",
            "# mine\n[terminal]\nfont_size = 14 # keep\n",
            "[keys]\nprefix = \"ctrl+b\"\n[[keys.command]]\nkey = \"prefix+a\"\ntype = \"shell\"\ncommand = \"echo mine\"\n",
        ] {
            let once = merge(original, &targets()).unwrap();
            let twice = merge(&once.text, &targets()).unwrap();
            assert_eq!(twice.text, once.text, "{original:?}");
            assert!(
                !twice.fresh && twice.added.is_empty() && twice.managed.is_empty(),
                "{twice:?}"
            );
        }
    }

    #[test]
    fn a_custom_shell_and_invalid_toml_are_refused_and_the_prototype_is_replaced() {
        for original in [
            "[terminal]\ndefault_shell = '/custom/launcher'\n",
            "invalid toml = [",
        ] {
            assert!(merge(original, &targets()).is_err(), "{original:?}");
        }
        for replaceable in [
            "dl-herdr-shell",
            "/home/u/.local/bin/herdr-workspace-shell",
            "",
        ] {
            let original = format!("[terminal]\ndefault_shell = {replaceable:?} # launcher\n");
            let merged = merge(&original, &targets()).unwrap();
            assert!(
                merged
                    .text
                    .contains(&format!("default_shell = {:?} # launcher", targets().shell)),
                "{}",
                merged.text
            );
        }
    }
}
