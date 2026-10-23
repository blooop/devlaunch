//! The compose files a compose project came from, read for the volumes they
//! mark `external`.
//!
//! compose does not label a volume it did not make, so the files are the only
//! place that says a volume is external. [`crate::flows::workspace_volumes`]
//! reads them to keep such volumes when it deletes a workspace.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use yaml_rust2::{Yaml, YamlLoader};

/// The compose files one compose project came from, as a container's
/// `com.docker.compose.project.config_files` label names them, and the project
/// directory compose resolved their relative paths against
/// (`com.docker.compose.project.working_dir`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComposeFiles {
    /// The project directory, when the container carries the label.
    pub(crate) working_dir: Option<PathBuf>,
    /// The files, in the order compose read them. Empty for a container compose
    /// did not start.
    pub(crate) files: Vec<PathBuf>,
}

impl ComposeFiles {
    /// Add another container's files of the same project: the first project
    /// directory named stays, and each file is added once, in the order first
    /// seen.
    pub(crate) fn merge(&mut self, other: ComposeFiles) {
        self.working_dir = self.working_dir.take().or(other.working_dir);
        for file in other.files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
    }

    /// The names of the volumes these compose files mark `external`, read the way
    /// compose merges them: file by file in order, a later file's fields over an
    /// earlier one's, per key under the top-level `volumes:`.
    ///
    /// An external volume's name is its `name:`, or the legacy `external: { name: }`,
    /// or else its key: compose does not prefix an external name with the project.
    /// A file that cannot be read or parsed, an `external:` that is not a bool or a
    /// map (compose reads `"true"` or `yes` as true), or a name with a `$` variable in
    /// it that only compose's environment could expand, is an error, because a volume the
    /// read cannot name is one the sweep cannot be sure to spare.
    ///
    /// A file's `include:` is followed, because an included file's top-level
    /// `volumes:` join the project: [`declare_file`] says how. A service's `extends:`
    /// is not, because it brings in one service and none of the base file's
    /// top-level volumes: compose refuses a service whose volume only the base file
    /// declares.
    pub(crate) fn external_volumes(&self) -> Result<BTreeSet<String>, String> {
        let project_dir = self
            .working_dir
            .as_deref()
            .or_else(|| self.files.first().and_then(|file| file.parent()))
            .unwrap_or(Path::new(""));
        let mut declared: IndexMap<String, Declared> = IndexMap::new();
        for file in &self.files {
            declare_file(file, project_dir, &mut declared, &mut Vec::new())?;
        }
        let mut names = BTreeSet::new();
        for (key, entry) in declared {
            let Some(External::Yes { legacy_name }) = entry.external else {
                continue;
            };
            let name = entry.name.or(legacy_name).unwrap_or(key);
            let Some(literal) = unescape_dollars(&name) else {
                return Err(format!(
                    "the compose files name external volume {name} with a variable"
                ));
            };
            names.insert(literal);
        }
        Ok(names)
    }
}

/// What the compose files so far say of one top-level volume.
#[derive(Default)]
struct Declared {
    /// `None` until a file says `external:`.
    external: Option<External>,
    name: Option<String>,
}

/// A volume's `external:`, as the last file that set it said.
enum External {
    /// `external: false`.
    No,
    /// `external: true`, or the legacy `external: { name: }` form with the name
    /// it gives.
    Yes { legacy_name: Option<String> },
}

/// Add what `file` declares under its top-level `volumes:` to `declared`, after
/// what the files it includes declare. compose merges every YAML document of a
/// file in order, as it merges files, so each document is read in turn.
///
/// compose resolves an `include:` path against the project directory of the file
/// that names it: `project_dir`, for a file compose was given, or for an
/// included file the directory of the first of its paths, or the
/// `project_directory:` its entry names. The included files come first, so a
/// later file still overrides what an earlier one included. `including` is the
/// chain of files being read, to refuse a file that includes itself.
fn declare_file(
    file: &Path,
    project_dir: &Path,
    declared: &mut IndexMap<String, Declared>,
    including: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let identity = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    if including.contains(&identity) {
        return Err(format!("compose file {} includes itself", file.display()));
    }
    let failed = |what: &str, error: &dyn std::fmt::Display| {
        format!("could not {what} compose file {}: {error}", file.display())
    };
    let text = std::fs::read_to_string(file).map_err(|error| failed("read", &error))?;
    let documents = YamlLoader::load_from_str(&text).map_err(|error| failed("parse", &error))?;
    including.push(identity);
    for document in &documents {
        let document = apply_merge_keys(document).map_err(|error| failed("read", &error))?;
        let included = includes(&document, project_dir).map_err(|error| failed("read", &error))?;
        for include in included {
            for path in &include.paths {
                declare_file(path, &include.project_dir, declared, including)?;
            }
        }
        declare_volumes(&document, file, declared)?;
    }
    including.pop();
    Ok(())
}

/// Add what one YAML document of `file` declares under its top-level `volumes:`
/// to `declared`.
fn declare_volumes(
    document: &Yaml,
    file: &Path,
    declared: &mut IndexMap<String, Declared>,
) -> Result<(), String> {
    let Some(volumes) = document["volumes"].as_hash() else {
        return Ok(());
    };
    for (key, spec) in volumes {
        let Some(key) = yaml_text(key) else {
            continue;
        };
        let entry = declared.entry(key.clone()).or_default();
        if let Some(name) = spec["name"].as_str() {
            entry.name = Some(name.to_owned());
        }
        match &spec["external"] {
            Yaml::Boolean(false) => entry.external = Some(External::No),
            Yaml::Boolean(true) => entry.external = Some(External::Yes { legacy_name: None }),
            external @ Yaml::Hash(_) => {
                entry.external = Some(External::Yes {
                    legacy_name: external["name"].as_str().map(str::to_owned),
                });
            }
            Yaml::BadValue | Yaml::Null => {}
            _ => {
                return Err(format!(
                    "compose file {} marks volume {key} external with a value that is not a bool",
                    file.display()
                ));
            }
        }
    }
    Ok(())
}

/// One entry of a compose file's `include:`: the files it loads, merged in
/// order, and the project directory their own relative paths resolve against.
struct Include {
    paths: Vec<PathBuf>,
    project_dir: PathBuf,
}

/// The entries of `document`'s `include:`, its relative paths resolved against
/// `project_dir`. An entry is a path, or a map whose `path:` is a path or a list
/// of them, with an optional `project_directory:`. Anything else, or a path with
/// a `$` variable in it, is an error.
fn includes(document: &Yaml, project_dir: &Path) -> Result<Vec<Include>, String> {
    let entries = match &document["include"] {
        Yaml::BadValue | Yaml::Null => return Ok(Vec::new()),
        Yaml::Array(entries) => entries,
        _ => return Err("an `include:` that is not a list".to_owned()),
    };
    let resolve = |path: &str| {
        unescape_dollars(path)
            .map(|path| project_dir.join(path))
            .ok_or_else(|| format!("an `include:` path {path} with a variable"))
    };
    entries
        .iter()
        .map(|entry| {
            let (paths, directory) = match entry {
                Yaml::String(path) => (vec![resolve(path)?], None),
                Yaml::Hash(_) => {
                    let paths = match &entry["path"] {
                        Yaml::String(path) => vec![resolve(path)?],
                        Yaml::Array(paths) => paths
                            .iter()
                            .map(|path| {
                                path.as_str()
                                    .ok_or_else(|| {
                                        "an `include:` path that is not a string".to_owned()
                                    })
                                    .and_then(&resolve)
                            })
                            .collect::<Result<_, _>>()?,
                        _ => return Err("an `include:` entry with no path".to_owned()),
                    };
                    let directory = match &entry["project_directory"] {
                        Yaml::BadValue | Yaml::Null => None,
                        Yaml::String(directory) => Some(resolve(directory)?),
                        _ => {
                            return Err(
                                "an `include:` project_directory that is not a string".to_owned()
                            );
                        }
                    };
                    (paths, directory)
                }
                _ => return Err("an `include:` entry that is not a path or a map".to_owned()),
            };
            let project_dir = match directory {
                Some(directory) => directory,
                None => paths
                    .first()
                    .ok_or_else(|| "an `include:` entry with no path".to_owned())?
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_owned(),
            };
            Ok(Include { paths, project_dir })
        })
        .collect()
}

/// `value` with every YAML merge key (`<<: *anchor`, or `<<: [*a, *b]`) applied the
/// way compose applies it: a map's own keys win, then the merged maps in order.
fn apply_merge_keys(value: &Yaml) -> Result<Yaml, String> {
    match value {
        Yaml::Hash(hash) => {
            let merge_key = Yaml::String("<<".to_owned());
            let mut applied = yaml_rust2::yaml::Hash::new();
            for (key, item) in hash {
                if *key != merge_key {
                    applied.insert(key.clone(), apply_merge_keys(item)?);
                }
            }
            let sources = match hash.get(&merge_key) {
                None => &[][..],
                Some(source @ Yaml::Hash(_)) => std::slice::from_ref(source),
                Some(Yaml::Array(sources)) => sources.as_slice(),
                Some(_) => return Err("a merge key `<<` that is not a map".to_owned()),
            };
            for source in sources {
                let Yaml::Hash(source) = apply_merge_keys(source)? else {
                    return Err("a merge key `<<` that is not a map".to_owned());
                };
                for (key, item) in source {
                    if !applied.contains_key(&key) {
                        applied.insert(key, item);
                    }
                }
            }
            Ok(Yaml::Hash(applied))
        }
        Yaml::Array(items) => items
            .iter()
            .map(apply_merge_keys)
            .collect::<Result<_, _>>()
            .map(Yaml::Array),
        other => Ok(other.clone()),
    }
}

/// `text` with compose's `$$` escape read as a literal `$`, or `None` when any
/// other `$` starts a variable only compose's environment could expand.
fn unescape_dollars(text: &str) -> Option<String> {
    let mut literal = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '$' && chars.next() != Some('$') {
            return None;
        }
        literal.push(c);
    }
    Some(literal)
}

/// A YAML key or scalar as compose reads it: a string, or a number or bool it
/// spells the same way.
fn yaml_text(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(n) => Some(n.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These files, with no project directory label.
    fn in_order(files: Vec<PathBuf>) -> ComposeFiles {
        ComposeFiles {
            working_dir: None,
            files,
        }
    }

    #[test]
    fn a_merge_keeps_the_first_project_directory_and_each_file_once() {
        let mut group = in_order(vec!["/p/a.yml".into()]);
        group.merge(ComposeFiles {
            working_dir: Some("/p".into()),
            files: vec!["/p/b.yml".into(), "/p/a.yml".into()],
        });
        group.merge(ComposeFiles {
            working_dir: Some("/q".into()),
            files: vec!["/p/c.yml".into()],
        });
        assert_eq!(
            group,
            ComposeFiles {
                working_dir: Some("/p".into()),
                files: vec!["/p/a.yml".into(), "/p/b.yml".into(), "/p/c.yml".into()],
            }
        );
    }

    #[test]
    fn a_merge_key_is_applied_before_external_is_read() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("compose.yml");
        std::fs::write(
            &file,
            "x-ext: &ext\n  external: true\nx-named: &named\n  name: app-named\n\
             volumes:\n  app-cache:\n    <<: *ext\n  \
             listed:\n    <<: [*named, *ext]\n  \
             overridden:\n    <<: *ext\n    external: false\n",
        )
        .expect("compose");
        assert_eq!(
            in_order(vec![file]).external_volumes(),
            Ok(["app-cache", "app-named"]
                .map(str::to_owned)
                .into_iter()
                .collect())
        );
    }

    #[test]
    fn a_doubled_dollar_in_an_external_name_is_a_literal_dollar() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("compose.yml");
        std::fs::write(
            &file,
            "volumes:\n  cache:\n    external: true\n    name: a$$b\n",
        )
        .expect("compose");
        assert_eq!(
            in_order(vec![file]).external_volumes(),
            Ok(["a$b".to_owned()].into_iter().collect())
        );
    }

    fn externals_of(base: &str, over: &str) -> Result<BTreeSet<String>, String> {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let base_path = dir.path().join("base.yml");
        let over_path = dir.path().join("over.yml");
        std::fs::write(&base_path, base).expect("base");
        std::fs::write(&over_path, over).expect("override");
        in_order(vec![base_path, over_path]).external_volumes()
    }

    #[test]
    fn a_later_external_false_drops_an_earlier_external_true() {
        assert_eq!(
            externals_of(
                "volumes:\n  cache:\n    external: true\n    name: app-cache\n",
                "volumes:\n  cache:\n    external: false\n",
            ),
            Ok(BTreeSet::new())
        );
    }

    #[test]
    fn a_later_external_true_drops_an_earlier_legacy_name() {
        assert_eq!(
            externals_of(
                "volumes:\n  cache:\n    external:\n      name: app-cache\n",
                "volumes:\n  cache:\n    external: true\n",
            ),
            Ok(["cache".to_owned()].into_iter().collect())
        );
    }

    #[test]
    fn a_later_compose_file_overrides_an_earlier_one_key_by_key() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let base = dir.path().join("base.yml");
        let over = dir.path().join("over.yml");
        std::fs::write(&base, "volumes:\n  cache:\n    name: app-cache\n  b: {}\n").expect("base");
        std::fs::write(
            &over,
            "volumes:\n  cache:\n    external: true\n  b:\n    external: true\n    name: app-b\n",
        )
        .expect("override");
        assert_eq!(
            in_order(vec![base.clone(), over.clone()]).external_volumes(),
            Ok(["app-b", "app-cache"]
                .map(str::to_owned)
                .into_iter()
                .collect())
        );
        // And the other way round, `external` from the first file stays unless
        // the second says otherwise.
        assert_eq!(
            in_order(vec![over, base]).external_volumes(),
            Ok(["app-b", "app-cache"]
                .map(str::to_owned)
                .into_iter()
                .collect())
        );
    }

    #[test]
    fn a_later_yaml_document_overrides_an_earlier_one_and_may_include() {
        assert_eq!(
            externals_in(
                &[
                    (
                        "compose.yml",
                        "---\n---\nvolumes:\n  v:\n    external: true\n\
                         ---\nvolumes:\n  v:\n    external: false\n\
                         ---\ninclude: [inc/x.yml]\n",
                    ),
                    (
                        "inc/x.yml",
                        "volumes:\n  w:\n    external: true\n    name: x-cache\n"
                    ),
                ],
                &["compose.yml"],
                None,
            ),
            names(&["x-cache"])
        );
    }

    /// Write `files` under a scratch dir and read the external volumes of the
    /// ones `labelled` names, in that order, with the scratch dir's `working_dir`
    /// subdirectory as compose's project directory when one is given.
    fn externals_in(
        files: &[(&str, &str)],
        labelled: &[&str],
        working_dir: Option<&str>,
    ) -> Result<BTreeSet<String>, String> {
        let dir = tempfile::tempdir().expect("a scratch dir");
        for (relative, text) in files {
            let file = dir.path().join(relative);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("a dir");
            std::fs::write(&file, text).expect("a compose file");
        }
        let working_dir = working_dir.map(|sub| dir.path().join(sub));
        let labelled: Vec<PathBuf> = labelled.iter().map(|file| dir.path().join(file)).collect();
        ComposeFiles {
            working_dir,
            files: labelled,
        }
        .external_volumes()
    }

    fn names(names: &[&str]) -> Result<BTreeSet<String>, String> {
        Ok(names.iter().map(|name| (*name).to_owned()).collect())
    }

    #[test]
    fn a_nested_include_is_relative_to_the_file_that_includes_it() {
        // compose loads an included file with its own directory as its project
        // directory, so `deep/inner.yml` named in `inc/a.yml` is `inc/deep/inner.yml`.
        assert_eq!(
            externals_in(
                &[
                    ("compose.yml", "include:\n  - path: inc/a.yml\n"),
                    ("inc/a.yml", "include: [deep/inner.yml]\n"),
                    (
                        "inc/deep/inner.yml",
                        "volumes:\n  v:\n    external: true\n    name: deep-cache\n"
                    ),
                ],
                &["compose.yml"],
                None,
            ),
            names(&["deep-cache"])
        );
    }

    #[test]
    fn an_include_in_a_later_file_is_relative_to_the_project_directory() {
        // compose resolves a top-level `include:` against the project directory,
        // the first file's directory, even when a later `-f` file in another
        // directory names it.
        let files = [
            ("a/compose.yml", "services:\n  x:\n    image: x\n"),
            ("b/over.yml", "include: [inc.yml]\n"),
            (
                "a/inc.yml",
                "volumes:\n  v:\n    external: true\n    name: in-a\n",
            ),
            (
                "b/inc.yml",
                "volumes:\n  v:\n    external: true\n    name: in-b\n",
            ),
        ];
        let labelled = ["a/compose.yml", "b/over.yml"];
        assert_eq!(externals_in(&files, &labelled, None), names(&["in-a"]));
        // The label names the project directory when compose was given one.
        assert_eq!(externals_in(&files, &labelled, Some("b")), names(&["in-b"]));
    }

    #[test]
    fn an_include_with_a_list_of_paths_merges_them_in_order() {
        assert_eq!(
            externals_in(
                &[
                    (
                        "compose.yml",
                        "include:\n  - path: [inc/x.yml, inc/y.yml]\n"
                    ),
                    (
                        "inc/x.yml",
                        "volumes:\n  v:\n    external: true\n    name: x-cache\n  w:\n    external: true\n"
                    ),
                    ("inc/y.yml", "volumes:\n  v:\n    external: false\n"),
                ],
                &["compose.yml"],
                None,
            ),
            names(&["w"])
        );
    }

    #[test]
    fn an_include_with_a_project_directory_resolves_its_own_includes_there() {
        assert_eq!(
            externals_in(
                &[
                    (
                        "compose.yml",
                        "include:\n  - path: inc/x.yml\n    project_directory: inc/other\n",
                    ),
                    ("inc/x.yml", "include: [y.yml]\n"),
                    (
                        "inc/other/y.yml",
                        "volumes:\n  v:\n    external: true\n    name: other-cache\n"
                    ),
                ],
                &["compose.yml"],
                None,
            ),
            names(&["other-cache"])
        );
    }

    #[test]
    fn a_later_file_overrides_a_volume_an_earlier_file_included() {
        assert_eq!(
            externals_in(
                &[
                    ("compose.yml", "include: [inc/x.yml]\n"),
                    ("over.yml", "volumes:\n  v:\n    external: false\n"),
                    (
                        "inc/x.yml",
                        "volumes:\n  v:\n    external: true\n    name: x-cache\n"
                    ),
                ],
                &["compose.yml", "over.yml"],
                None,
            ),
            Ok(BTreeSet::new())
        );
    }

    #[test]
    fn an_include_path_with_a_variable_removes_nothing() {
        let read = externals_in(
            &[("compose.yml", "include:\n  - ${DIR}/x.yml\n")],
            &["compose.yml"],
            None,
        );
        assert!(
            matches!(&read, Err(reason) if reason.contains("${DIR}/x.yml")),
            "{read:?}"
        );
    }

    #[test]
    fn an_include_dl_cannot_read_as_paths_removes_nothing() {
        for include in [
            "include: inc/x.yml\n",
            "include: [3]\n",
            "include:\n  - project_directory: inc\n",
            "include:\n  - path: []\n",
            "include:\n  - path: [inc/x.yml, 4]\n",
            "include:\n  - path: inc/x.yml\n    project_directory: [inc]\n",
        ] {
            let read = externals_in(
                &[
                    ("compose.yml", include),
                    ("inc/x.yml", "volumes:\n  v:\n    external: true\n"),
                ],
                &["compose.yml"],
                None,
            );
            assert!(
                matches!(&read, Err(reason) if reason.contains("compose.yml")),
                "{include}: {read:?}"
            );
        }
    }

    #[test]
    fn an_include_cycle_removes_nothing() {
        let read = externals_in(
            &[
                ("compose.yml", "include: [inc/a.yml]\n"),
                ("inc/a.yml", "include: [b.yml]\n"),
                ("inc/b.yml", "include: [a.yml]\n"),
            ],
            &["compose.yml"],
            None,
        );
        assert!(
            matches!(&read, Err(reason) if reason.contains("a.yml")),
            "{read:?}"
        );
    }
}
