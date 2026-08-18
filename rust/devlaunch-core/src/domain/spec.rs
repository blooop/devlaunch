//! What the user typed as a workspace spec, parsed once.
//!
//! `dl <spec>` accepts six unrelated things behind one argument: a directory on
//! this machine, `owner/repo[@branch]`, a bare `github.com/...` host path, a full
//! URL, an `ssh://`-less `user@host:path`, and — as the fallback — the name of a
//! workspace that already exists. Python asked that question again at every use
//! site (`is_path_spec`, `is_git_spec`, `parse_owner_repo_branch`,
//! `expand_workspace_spec`, `spec_to_workspace_id` each re-tested the string),
//! which is how the shapes drifted apart. Here the string is classified once,
//! into [`WorkspaceSpec`], and every later question is answered by matching on
//! the arm.
//!
//! Two derivations hang off the parse:
//!
//! - [`WorkspaceSpec::expanded`] — the source devpod is given
//!   (`owner/repo@branch` becomes `git@github.com:owner/repo.git@branch`;
//!   everything else is handed over as typed).
//! - [`identity`] — what names the workspace. Python returned a bare string for
//!   all five cases; [`SpecIdentity`] separates them, because they are not
//!   interchangeable: a branchless `owner/repo` yields only a repo *label* (there
//!   is no ref to hash, so there is no identity to derive), and a path spec's
//!   name is the resolved directory leaf, which only the filesystem can answer.
//!
//! # Ported from `devlaunch/dl.py`
//!
//! Semantics come from `dl.py` lines 1113-1310 and are pinned by golden vectors
//! taken from the Python. Three of its behaviours are easy to mistake for bugs
//! in this port and are ported on purpose:
//!
//! - **The identity gate reads the spec twice.** Python splits at the first `@`
//!   and asks `is_path_spec`/`is_git_spec` about the *base*, then asks
//!   `parse_owner_repo_branch` about the *whole* spec. So `git@github.com:o/r.git`
//!   has base `git`, which is no git source at all, and falls through to the
//!   existing-name arm; and `https://user@host/x` has base `https://user`, whose
//!   identity is derived from `user` alone.
//! - **An `owner/repo` spec whose `@`-suffix is not a usable branch** (`owner/repo@`,
//!   `owner/repo@br@x`, `owner/repo@a b`) still has base `owner/repo`, so it
//!   reaches the URL rule and derives the id of `github.com/owner/repo`, branch
//!   discarded.
//! - **`$` matches before one trailing newline**, in `OWNER_REPO_PATTERN` as in
//!   the ref rule, so `owner/repo@br\n` parses with the newline inside the
//!   branch — and therefore inside the suffix hash.
//!
//! `extract_devcontainer_flag` is deliberately absent: it is argv handling, which
//! belongs to the binary (M5). [`resolve_devcontainer_ref`] is the part that
//! decides what a `--devcontainer` value means.

// Callers land in M5 (the binary) through M7 (launch); until then the port's own
// tests are the only consumers of this module.
#![allow(dead_code)]

use std::borrow::Cow;

use super::workspace_id::{TARGET_LENGTH, UnsafeName, WorkspaceId, slug, source_workspace_id};

/// A workspace spec, classified.
///
/// Every arm carries what that arm makes meaningful and nothing else, so there is
/// no "which fields apply this time" question to get wrong. The classification is
/// total — the last arm is the fallback Python spelled "otherwise return as-is" —
/// so parsing cannot fail and there is no error type here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceSpec<'a> {
    /// A directory on this machine: `./x`, `/x`, `~/x`.
    Path(&'a str),
    /// `owner/repo[@branch]`, the shape `dl` exists to make short.
    OwnerRepo {
        owner: &'a str,
        repo: &'a str,
        branch: Option<&'a str>,
    },
    /// A bare host path: `github.com/...`, `gitlab.com/...`.
    HostPath(&'a str),
    /// Anything with a protocol in it.
    Url(&'a str),
    /// `user@host:path`, an scp-style git remote.
    SshUrl(&'a str),
    /// The name or id of a workspace that already exists.
    ExistingIdOrName(&'a str),
}

/// Classify a raw spec. Total: every string is one of the six.
pub(crate) fn parse(spec: &str) -> WorkspaceSpec<'_> {
    if is_path(spec) {
        return WorkspaceSpec::Path(spec);
    }
    if spec.contains("://") {
        return WorkspaceSpec::Url(spec);
    }
    if spec.starts_with("github.com/") || spec.starts_with("gitlab.com/") {
        return WorkspaceSpec::HostPath(spec);
    }
    if is_ssh_url(spec) {
        return WorkspaceSpec::SshUrl(spec);
    }
    match match_owner_repo(spec) {
        Some((owner, repo, branch)) => WorkspaceSpec::OwnerRepo {
            owner,
            repo,
            branch,
        },
        None => WorkspaceSpec::ExistingIdOrName(spec),
    }
}

impl<'a> WorkspaceSpec<'a> {
    /// The source devpod is given for this spec.
    ///
    /// Only `owner/repo` grows: GitHub is the host `dl` assumes, and the SSH form
    /// is what devpod clones with the user's own key.
    pub(crate) fn expanded(&self) -> Cow<'a, str> {
        match *self {
            Self::OwnerRepo {
                owner,
                repo,
                branch: Some(branch),
            } => Cow::Owned(format!("git@github.com:{owner}/{repo}.git@{branch}")),
            Self::OwnerRepo {
                owner,
                repo,
                branch: None,
            } => Cow::Owned(format!("git@github.com:{owner}/{repo}.git")),
            Self::Path(spec)
            | Self::HostPath(spec)
            | Self::Url(spec)
            | Self::SshUrl(spec)
            | Self::ExistingIdOrName(spec) => Cow::Borrowed(spec),
        }
    }

    /// Whether this names a git repository devpod would clone.
    ///
    /// Python's `is_git_spec`: an ssh URL is *not* one of these, which is not an
    /// oversight to fix here — the identity rule below depends on it.
    pub(crate) fn is_git_source(&self) -> bool {
        matches!(
            self,
            Self::Url(_) | Self::HostPath(_) | Self::OwnerRepo { .. }
        )
    }

    pub(crate) fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }
}

/// What names the workspace a spec asks for.
///
/// Python answered with a bare string, so a repo label and a workspace id were
/// the same type and a caller could spend one for the other. These four arms are
/// the four different things that string could be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpecIdentity<'a> {
    /// A workspace id: names the devpod workspace and the clone-directory leaf.
    Workspace(String),
    /// `owner/repo` with no ref. A workspace is a branch checkout, so there is
    /// nothing here to identify — this is legible context only. A caller that
    /// wants an id resolves the default branch first and derives from the triple.
    /// (Capped anyway, so no caller can get a string from here that overflows
    /// devpod's limit.)
    RepoLabel(String),
    /// A path spec. devpod names it after the resolved directory, so the leaf is
    /// the filesystem's answer, not this module's: expand `~`, resolve, take the
    /// final component. The path carried here is the spec text before any `@`.
    PathLeaf(&'a str),
    /// The spec already named a workspace; it is its own identity.
    ExistingName(&'a str),
}

/// Derive what *spec* names, or refuse an owner, repo or ref that is not a safe
/// git name.
pub(crate) fn identity(spec: &str) -> Result<SpecIdentity<'_>, UnsafeName> {
    // Python splits at the first `@` and gates on the *base*; see the module docs.
    let base = match spec.split_once('@') {
        Some((base, _)) => base,
        None => spec,
    };
    let base_spec = parse(base);
    if let WorkspaceSpec::Path(path) = base_spec {
        return Ok(SpecIdentity::PathLeaf(path));
    }
    if !base_spec.is_git_source() {
        return Ok(SpecIdentity::ExistingName(spec));
    }
    match parse(spec) {
        WorkspaceSpec::OwnerRepo {
            owner,
            repo,
            branch: Some(branch),
        } => Ok(SpecIdentity::Workspace(
            WorkspaceId::new(owner, repo, branch)?.value(),
        )),
        WorkspaceSpec::OwnerRepo {
            repo, branch: None, ..
        } => Ok(SpecIdentity::RepoLabel(repo_label(repo))),
        // Not an (owner, repo, ref) triple: a URL, a host path, or an owner/repo
        // whose `@`-suffix is no branch. Same scheme as a triple -- slug for
        // legibility, suffix for identity, capped. The old rule here deleted `_`
        // while the owner/repo path turned it into `-`, so one repo derived two
        // ids; applying only the slug rule instead swapped that for a collision,
        // since `my_repo`, `my-repo` and `my.repo` slug alike.
        _ => Ok(SpecIdentity::Workspace(source_workspace_id(
            &normalise_source(&base_spec.expanded()),
        ))),
    }
}

fn repo_label(repo: &str) -> String {
    slug(repo)
        .chars()
        .take(TARGET_LENGTH)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Reduce an expanded git source to the `host/path` a source id is hashed over.
fn normalise_source(expanded: &str) -> String {
    let without_protocol = match expanded.split_once("://") {
        Some((_, rest)) => rest,
        None => expanded,
    };
    let rewritten = match rewrite_scp_form(without_protocol) {
        Some(rewritten) => rewritten,
        None => without_protocol.to_string(),
    };
    match rewritten.strip_suffix(".git") {
        Some(trimmed) => trimmed.to_string(),
        None => rewritten,
    }
}

/// `user@host:path` -> `host/path`, or `None` when the source is not scp-shaped.
///
/// Python's `re.match(r"^[^@]+@([^:]+):(.*)")`: the path group is `.*`, which
/// stops at a newline, so anything after one is dropped. Kept, because the output
/// feeds a hash whose values are already on disk.
fn rewrite_scp_form(source: &str) -> Option<String> {
    let (user, rest) = source.split_once('@')?;
    if user.is_empty() {
        return None;
    }
    let (host, path) = rest.split_once(':')?;
    if host.is_empty() {
        return None;
    }
    let path = match path.find('\n') {
        Some(end) => &path[..end],
        None => path,
    };
    Some(format!("{host}/{path}"))
}

fn is_path(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with('/') || spec.starts_with('~')
}

/// Python's `^[^@]+@[^:]+:` — an scp-style remote, told apart from
/// `owner/repo@branch` by the colon, which no `owner/repo` character class allows.
fn is_ssh_url(spec: &str) -> bool {
    let Some((user, rest)) = spec.split_once('@') else {
        return false;
    };
    if user.is_empty() {
        return false;
    }
    match rest.split_once(':') {
        Some((host, _)) => !host.is_empty(),
        None => false,
    }
}

/// Python's `OWNER_REPO_PATTERN`:
/// `^[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+(@[a-zA-Z0-9_./%-]+)?$`.
///
/// The captures come from the raw spec, not from the newline-stripped body the
/// `$` quirk matches against, so a trailing newline stays inside the repo or the
/// branch exactly as Python left it.
fn match_owner_repo(spec: &str) -> Option<(&str, &str, Option<&str>)> {
    let body = spec.strip_suffix('\n').unwrap_or(spec);
    let (owner_repo, branch) = match body.split_once('@') {
        Some((owner_repo, branch)) => (owner_repo, Some(branch)),
        None => (body, None),
    };
    let (owner, repo) = owner_repo.split_once('/')?;
    if !is_name_part(owner) || !is_name_part(repo) {
        return None;
    }
    if let Some(branch) = branch
        && !is_branch_part(branch)
    {
        return None;
    }
    // Re-split the raw spec so the captures keep any trailing newline.
    let (owner_repo, branch) = match spec.split_once('@') {
        Some((owner_repo, branch)) => (owner_repo, Some(branch)),
        None => (spec, None),
    };
    let (owner, repo) = owner_repo.split_once('/')?;
    Some((owner, repo, branch))
}

fn is_name_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn is_branch_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'%' | b'-'))
}

/// A `--devcontainer` value that has been through the parse boundary: a path
/// devpod can be handed as `--devcontainer-path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevcontainerPath(String);

impl DevcontainerPath {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a `--devcontainer` value cannot be a path. The offending value is the
/// caller's own argument, so the refusal does not repeat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DevcontainerRefError {
    /// Empty or blank: `dl --devcontainer ''`.
    Missing,
    /// A flag turned up where the value should be: `dl --devcontainer --help`.
    FlagLike,
}

/// Turn a `--devcontainer` value into a devcontainer.json path for devpod.
///
/// A bare name expands to the spec's one-level-deep variant location,
/// `.devcontainer/<name>/devcontainer.json`. Anything containing a separator or
/// ending in `.json` is used as given.
///
/// devpod's own `--devcontainer-id` takes a bare variant name and looks like the
/// same job, but it is silently ignored in devpod 0.26.1: a fresh
/// `devpod up --id x --devcontainer-id alt` parses
/// `.devcontainer/devcontainer.json` and stores no devContainerID, while
/// `--devcontainer-path` selects the variant correctly. Build the path here until
/// that is fixed upstream.
pub(crate) fn resolve_devcontainer_ref(
    raw: &str,
) -> Result<DevcontainerPath, DevcontainerRefError> {
    if raw.chars().all(is_python_space) {
        return Err(DevcontainerRefError::Missing);
    }
    if raw.starts_with('-') {
        return Err(DevcontainerRefError::FlagLike);
    }
    if raw.contains('/') || raw.ends_with(".json") {
        return Ok(DevcontainerPath(raw.to_string()));
    }
    Ok(DevcontainerPath(format!(
        ".devcontainer/{raw}/devcontainer.json"
    )))
}

/// `str.isspace()`, which counts the four information separators that Unicode's
/// `White_Space` property leaves out.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::workspace_id::NamePart;

    /// The arm [`parse`] must return, with the payloads the arm makes meaningful
    /// and none of the raw text the golden table would only repeat.
    #[derive(Debug, PartialEq, Eq)]
    enum Shape<'a> {
        Path,
        OwnerRepo {
            owner: &'a str,
            repo: &'a str,
            branch: Option<&'a str>,
        },
        HostPath,
        Url,
        SshUrl,
        ExistingIdOrName,
    }

    fn shape_of<'a>(parsed: &WorkspaceSpec<'a>) -> Shape<'a> {
        match *parsed {
            WorkspaceSpec::Path(_) => Shape::Path,
            WorkspaceSpec::OwnerRepo {
                owner,
                repo,
                branch,
            } => Shape::OwnerRepo {
                owner,
                repo,
                branch,
            },
            WorkspaceSpec::HostPath(_) => Shape::HostPath,
            WorkspaceSpec::Url(_) => Shape::Url,
            WorkspaceSpec::SshUrl(_) => Shape::SshUrl,
            WorkspaceSpec::ExistingIdOrName(_) => Shape::ExistingIdOrName,
        }
    }

    /// What Python's `spec_to_workspace_id` answers, in the arms this port draws.
    #[derive(Debug, PartialEq, Eq, Hash)]
    enum Expect {
        Workspace(&'static str),
        RepoLabel(&'static str),
        PathLeaf(&'static str),
        ExistingName(&'static str),
        Unsafe,
    }

    struct Case {
        spec: &'static str,
        shape: Shape<'static>,
        expanded: &'static str,
        identity: Expect,
    }

    fn observed(spec: &str) -> Expect {
        match identity(spec) {
            Err(_) => Expect::Unsafe,
            Ok(SpecIdentity::Workspace(value)) => Expect::Workspace(leak(value)),
            Ok(SpecIdentity::RepoLabel(label)) => Expect::RepoLabel(leak(label)),
            Ok(SpecIdentity::PathLeaf(path)) => Expect::PathLeaf(leak(path.to_string())),
            Ok(SpecIdentity::ExistingName(name)) => Expect::ExistingName(leak(name.to_string())),
        }
    }

    /// Only so the two sides of an `assert_eq!` share one type; the strings live
    /// as long as the test process either way.
    fn leak(owned: String) -> &'static str {
        Box::leak(owned.into_boxed_str())
    }

    /// spec -> (shape, expansion, identity), straight out of the Python
    /// implementation (`dl.py`: parse_owner_repo_branch, is_path_spec,
    /// is_git_spec, expand_workspace_spec, spec_to_workspace_id).
    #[rustfmt::skip]
    fn golden_specs() -> Vec<Case> {
        vec![
            Case { spec: "blooop/devlaunch", shape: Shape::OwnerRepo { owner: "blooop", repo: "devlaunch", branch: None }, expanded: "git@github.com:blooop/devlaunch.git", identity: Expect::RepoLabel("devlaunch") },
            Case { spec: "blooop/devlaunch@main", shape: Shape::OwnerRepo { owner: "blooop", repo: "devlaunch", branch: Some("main") }, expanded: "git@github.com:blooop/devlaunch.git@main", identity: Expect::Workspace("devlaunch-main-zovomobo") },
            Case { spec: "owner/repo@feature/my-branch", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("feature/my-branch") }, expanded: "git@github.com:owner/repo.git@feature/my-branch", identity: Expect::Workspace("repo-feature-my-branch-kefelaga") },
            Case { spec: "Owner/Repo@Feature/MyBranch", shape: Shape::OwnerRepo { owner: "Owner", repo: "Repo", branch: Some("Feature/MyBranch") }, expanded: "git@github.com:Owner/Repo.git@Feature/MyBranch", identity: Expect::Workspace("repo-feature-mybranch-metanola") },
            Case { spec: "blooop/test_renv", shape: Shape::OwnerRepo { owner: "blooop", repo: "test_renv", branch: None }, expanded: "git@github.com:blooop/test_renv.git", identity: Expect::RepoLabel("test-renv") },
            Case { spec: "blooop/test_renv@nb12", shape: Shape::OwnerRepo { owner: "blooop", repo: "test_renv", branch: Some("nb12") }, expanded: "git@github.com:blooop/test_renv.git@nb12", identity: Expect::Workspace("test-renv-nb12-renovolo") },
            Case { spec: "blooop/test_renv@nb14", shape: Shape::OwnerRepo { owner: "blooop", repo: "test_renv", branch: Some("nb14") }, expanded: "git@github.com:blooop/test_renv.git@nb14", identity: Expect::Workspace("test-renv-nb14-jokovafe") },
            Case { spec: "blooop/python_template", shape: Shape::OwnerRepo { owner: "blooop", repo: "python_template", branch: None }, expanded: "git@github.com:blooop/python_template.git", identity: Expect::RepoLabel("python-template") },
            Case { spec: "blooop/python_template@nb4", shape: Shape::OwnerRepo { owner: "blooop", repo: "python_template", branch: Some("nb4") }, expanded: "git@github.com:blooop/python_template.git@nb4", identity: Expect::Workspace("python-template-nb4-foganeje") },
            Case { spec: "loft-sh/devpod", shape: Shape::OwnerRepo { owner: "loft-sh", repo: "devpod", branch: None }, expanded: "git@github.com:loft-sh/devpod.git", identity: Expect::RepoLabel("devpod") },
            Case { spec: "owner/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr", shape: Shape::OwnerRepo { owner: "owner", repo: "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr", branch: None }, expanded: "git@github.com:owner/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr.git", identity: Expect::RepoLabel("rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr") },
            Case { spec: "owner/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr@main", shape: Shape::OwnerRepo { owner: "owner", repo: "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr", branch: Some("main") }, expanded: "git@github.com:owner/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr.git@main", identity: Expect::Workspace("rrrrrrrrrrrrrrrrrrrr-main-rerajezi") },
            Case { spec: "owner/repo@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa") }, expanded: "git@github.com:owner/repo.git@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", identity: Expect::Workspace("repo-aaaaaaaaaaaaaaaaaaaaaaaa-pokipete") },
            Case { spec: "owner/myrepo@feature/some-very-long-branch-name-here", shape: Shape::OwnerRepo { owner: "owner", repo: "myrepo", branch: Some("feature/some-very-long-branch-name-here") }, expanded: "git@github.com:owner/myrepo.git@feature/some-very-long-branch-name-here", identity: Expect::Workspace("myrepo-feature-some-very-long-lafazota") },
            Case { spec: "user.name/repo.name", shape: Shape::OwnerRepo { owner: "user.name", repo: "repo.name", branch: None }, expanded: "git@github.com:user.name/repo.name.git", identity: Expect::RepoLabel("repo-name") },
            Case { spec: "my_user/my_repo", shape: Shape::OwnerRepo { owner: "my_user", repo: "my_repo", branch: None }, expanded: "git@github.com:my_user/my_repo.git", identity: Expect::RepoLabel("my-repo") },
            Case { spec: "owner/repo@main", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("main") }, expanded: "git@github.com:owner/repo.git@main", identity: Expect::Workspace("repo-main-gemetato") },
            Case { spec: "owner/repo@Main", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("Main") }, expanded: "git@github.com:owner/repo.git@Main", identity: Expect::Workspace("repo-main-rafigemo") },
            Case { spec: "someone/devlaunch@main", shape: Shape::OwnerRepo { owner: "someone", repo: "devlaunch", branch: Some("main") }, expanded: "git@github.com:someone/devlaunch.git@main", identity: Expect::Workspace("devlaunch-main-dedavevi") },
            Case { spec: "blooop/devlaunch@feature/auth", shape: Shape::OwnerRepo { owner: "blooop", repo: "devlaunch", branch: Some("feature/auth") }, expanded: "git@github.com:blooop/devlaunch.git@feature/auth", identity: Expect::Workspace("devlaunch-feature-auth-poliseno") },
            Case { spec: "blooop/devlaunch@feature-auth", shape: Shape::OwnerRepo { owner: "blooop", repo: "devlaunch", branch: Some("feature-auth") }, expanded: "git@github.com:blooop/devlaunch.git@feature-auth", identity: Expect::Workspace("devlaunch-feature-auth-nesatabe") },
            Case { spec: "owner/repo@bad%branch", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("bad%branch") }, expanded: "git@github.com:owner/repo.git@bad%branch", identity: Expect::Unsafe },
            Case { spec: "owner/repo@br%20x", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("br%20x") }, expanded: "git@github.com:owner/repo.git@br%20x", identity: Expect::Unsafe },
            Case { spec: "owner/repo@", shape: Shape::ExistingIdOrName, expanded: "owner/repo@", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "owner/repo@br@x", shape: Shape::ExistingIdOrName, expanded: "owner/repo@br@x", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "owner/repo@a b", shape: Shape::ExistingIdOrName, expanded: "owner/repo@a b", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "owner/repo@a:b", shape: Shape::SshUrl, expanded: "owner/repo@a:b", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "owner/repo@..", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("..") }, expanded: "git@github.com:owner/repo.git@..", identity: Expect::Unsafe },
            Case { spec: "owner/repo@-x", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("-x") }, expanded: "git@github.com:owner/repo.git@-x", identity: Expect::Unsafe },
            Case { spec: "owner/repo@feature/../x", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("feature/../x") }, expanded: "git@github.com:owner/repo.git@feature/../x", identity: Expect::Workspace("repo-feature-x-hipohogi") },
            Case { spec: "owner/repo\n", shape: Shape::OwnerRepo { owner: "owner", repo: "repo\n", branch: None }, expanded: "git@github.com:owner/repo\n.git", identity: Expect::RepoLabel("repo") },
            Case { spec: "owner/repo@br\n", shape: Shape::OwnerRepo { owner: "owner", repo: "repo", branch: Some("br\n") }, expanded: "git@github.com:owner/repo.git@br\n", identity: Expect::Workspace("repo-br-gokimiva") },
            Case { spec: "owner/repo@br\nx", shape: Shape::ExistingIdOrName, expanded: "owner/repo@br\nx", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "github.com/owner/repo", shape: Shape::HostPath, expanded: "github.com/owner/repo", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "github.com/owner/repo.git", shape: Shape::HostPath, expanded: "github.com/owner/repo.git", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "github.com/owner/repo@branch", shape: Shape::HostPath, expanded: "github.com/owner/repo@branch", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "gitlab.com/owner/repo", shape: Shape::HostPath, expanded: "gitlab.com/owner/repo", identity: Expect::Workspace("gitlab-com-owner-repo-ninajera") },
            Case { spec: "gitlab.com/group/my_repo", shape: Shape::HostPath, expanded: "gitlab.com/group/my_repo", identity: Expect::Workspace("gitlab-com-group-my-repo-gaditizi") },
            Case { spec: "gitlab.com/group/my-repo", shape: Shape::HostPath, expanded: "gitlab.com/group/my-repo", identity: Expect::Workspace("gitlab-com-group-my-repo-ledorapa") },
            Case { spec: "gitlab.com/group/my.repo", shape: Shape::HostPath, expanded: "gitlab.com/group/my.repo", identity: Expect::Workspace("gitlab-com-group-my-repo-napasava") },
            Case { spec: "github.com/owner", shape: Shape::HostPath, expanded: "github.com/owner", identity: Expect::Workspace("github-com-owner-ranejiza") },
            Case { spec: "github.com/loft-sh/devpod", shape: Shape::HostPath, expanded: "github.com/loft-sh/devpod", identity: Expect::Workspace("github-com-loft-sh-devpod-vatomiha") },
            Case { spec: "github.com/Blooop/DevLaunch", shape: Shape::HostPath, expanded: "github.com/Blooop/DevLaunch", identity: Expect::Workspace("github-com-blooop-devlaunch-lakatoje") },
            Case { spec: "github.com/blooop/devlaunch", shape: Shape::HostPath, expanded: "github.com/blooop/devlaunch", identity: Expect::Workspace("github-com-blooop-devlaunch-lakatoje") },
            Case { spec: "github.com/oooooooooooooooooooooooooooooooooooooooo/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr", shape: Shape::HostPath, expanded: "github.com/oooooooooooooooooooooooooooooooooooooooo/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr", identity: Expect::Workspace("github-com-oooooooooooooooooo-rasijome") },
            Case { spec: "https://github.com/owner/repo", shape: Shape::Url, expanded: "https://github.com/owner/repo", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "https://github.com/owner/repo@main", shape: Shape::Url, expanded: "https://github.com/owner/repo@main", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "https://github.com/owner/repo.git", shape: Shape::Url, expanded: "https://github.com/owner/repo.git", identity: Expect::Workspace("github-com-owner-repo-lokolede") },
            Case { spec: "https://user@host/x", shape: Shape::Url, expanded: "https://user@host/x", identity: Expect::Workspace("user-hadefeji") },
            Case { spec: "ssh://git@github.com/o/r.git", shape: Shape::Url, expanded: "ssh://git@github.com/o/r.git", identity: Expect::Workspace("git-femapohe") },
            Case { spec: "git@github.com:owner/repo.git", shape: Shape::SshUrl, expanded: "git@github.com:owner/repo.git", identity: Expect::ExistingName("git@github.com:owner/repo.git") },
            Case { spec: "git@github.com:owner/repo.git@feature/my-branch", shape: Shape::SshUrl, expanded: "git@github.com:owner/repo.git@feature/my-branch", identity: Expect::ExistingName("git@github.com:owner/repo.git@feature/my-branch") },
            Case { spec: "git@gitlab.com:owner/repo.git", shape: Shape::SshUrl, expanded: "git@gitlab.com:owner/repo.git", identity: Expect::ExistingName("git@gitlab.com:owner/repo.git") },
            Case { spec: "git@bitbucket.org:owner/repo.git", shape: Shape::SshUrl, expanded: "git@bitbucket.org:owner/repo.git", identity: Expect::ExistingName("git@bitbucket.org:owner/repo.git") },
            Case { spec: "git@enterprise.example.com:owner/repo.git", shape: Shape::SshUrl, expanded: "git@enterprise.example.com:owner/repo.git", identity: Expect::ExistingName("git@enterprise.example.com:owner/repo.git") },
            Case { spec: "myworkspace", shape: Shape::ExistingIdOrName, expanded: "myworkspace", identity: Expect::ExistingName("myworkspace") },
            Case { spec: "my-workspace", shape: Shape::ExistingIdOrName, expanded: "my-workspace", identity: Expect::ExistingName("my-workspace") },
            Case { spec: "myworkspace@foo", shape: Shape::ExistingIdOrName, expanded: "myworkspace@foo", identity: Expect::ExistingName("myworkspace@foo") },
            Case { spec: "workspace", shape: Shape::ExistingIdOrName, expanded: "workspace", identity: Expect::ExistingName("workspace") },
            Case { spec: "x@a/b", shape: Shape::ExistingIdOrName, expanded: "x@a/b", identity: Expect::ExistingName("x@a/b") },
            Case { spec: "./my-project", shape: Shape::Path, expanded: "./my-project", identity: Expect::PathLeaf("./my-project") },
            Case { spec: "/home/user/project", shape: Shape::Path, expanded: "/home/user/project", identity: Expect::PathLeaf("/home/user/project") },
            Case { spec: "~/projects/test", shape: Shape::Path, expanded: "~/projects/test", identity: Expect::PathLeaf("~/projects/test") },
            Case { spec: "./my-project@foo", shape: Shape::Path, expanded: "./my-project@foo", identity: Expect::PathLeaf("./my-project") },
            Case { spec: "/home/user/project@branch", shape: Shape::Path, expanded: "/home/user/project@branch", identity: Expect::PathLeaf("/home/user/project") },
            Case { spec: "~/projects/test@main", shape: Shape::Path, expanded: "~/projects/test@main", identity: Expect::PathLeaf("~/projects/test") },
            Case { spec: "./path/to/project", shape: Shape::Path, expanded: "./path/to/project", identity: Expect::PathLeaf("./path/to/project") },
            Case { spec: "a/b", shape: Shape::OwnerRepo { owner: "a", repo: "b", branch: None }, expanded: "git@github.com:a/b.git", identity: Expect::RepoLabel("b") },
            Case { spec: "a//b", shape: Shape::ExistingIdOrName, expanded: "a//b", identity: Expect::ExistingName("a//b") },
            Case { spec: "a/b/c", shape: Shape::ExistingIdOrName, expanded: "a/b/c", identity: Expect::ExistingName("a/b/c") },
            Case { spec: "@", shape: Shape::ExistingIdOrName, expanded: "@", identity: Expect::ExistingName("@") },
            Case { spec: "@main", shape: Shape::ExistingIdOrName, expanded: "@main", identity: Expect::ExistingName("@main") },
            Case { spec: "/", shape: Shape::Path, expanded: "/", identity: Expect::PathLeaf("/") },
            Case { spec: "~", shape: Shape::Path, expanded: "~", identity: Expect::PathLeaf("~") },
            Case { spec: ".", shape: Shape::ExistingIdOrName, expanded: ".", identity: Expect::ExistingName(".") },
            Case { spec: "..", shape: Shape::ExistingIdOrName, expanded: "..", identity: Expect::ExistingName("..") },
            Case { spec: "-x", shape: Shape::ExistingIdOrName, expanded: "-x", identity: Expect::ExistingName("-x") },
            Case { spec: "x-", shape: Shape::ExistingIdOrName, expanded: "x-", identity: Expect::ExistingName("x-") },
            Case { spec: "x@y://z", shape: Shape::Url, expanded: "x@y://z", identity: Expect::ExistingName("x@y://z") },
            Case { spec: "a/b@c:d", shape: Shape::SshUrl, expanded: "a/b@c:d", identity: Expect::Workspace("github-com-a-b-bapafaho") },
        ]
    }

    /// --devcontainer value -> resolved path, or the refusal.
    const GOLDEN_DEVCONTAINERS: &[(&str, Result<&str, DevcontainerRefError>)] = &[
        ("ubuntu", Ok(".devcontainer/ubuntu/devcontainer.json")),
        ("alt", Ok(".devcontainer/alt/devcontainer.json")),
        ("x/y", Ok("x/y")),
        ("a.json", Ok("a.json")),
        (
            ".devcontainer/x/devcontainer.json",
            Ok(".devcontainer/x/devcontainer.json"),
        ),
        ("foo/bar.json", Ok("foo/bar.json")),
        ("", Err(DevcontainerRefError::Missing)),
        (" ", Err(DevcontainerRefError::Missing)),
        ("\t", Err(DevcontainerRefError::Missing)),
        ("\u{a0}", Err(DevcontainerRefError::Missing)),
        ("\u{1c}", Err(DevcontainerRefError::Missing)),
        ("\u{1f}", Err(DevcontainerRefError::Missing)),
        ("-x", Err(DevcontainerRefError::FlagLike)),
        ("--help", Err(DevcontainerRefError::FlagLike)),
        ("a-b", Ok(".devcontainer/a-b/devcontainer.json")),
        ("a b", Ok(".devcontainer/a b/devcontainer.json")),
        ("x.json", Ok("x.json")),
        ("/abs/path.json", Ok("/abs/path.json")),
        ("a/b/c", Ok("a/b/c")),
        ("json", Ok(".devcontainer/json/devcontainer.json")),
    ];

    #[test]
    fn golden_specs_classify_as_python_does() {
        for case in golden_specs() {
            assert_eq!(
                shape_of(&parse(case.spec)),
                case.shape,
                "classification of {:?}",
                case.spec
            );
        }
    }

    #[test]
    fn golden_specs_expand_as_python_does() {
        for case in golden_specs() {
            assert_eq!(
                parse(case.spec).expanded(),
                case.expanded,
                "expansion of {:?}",
                case.spec
            );
        }
    }

    #[test]
    fn golden_specs_derive_pythons_identity() {
        for case in golden_specs() {
            assert_eq!(
                observed(case.spec),
                case.identity,
                "identity of {:?}",
                case.spec
            );
        }
    }

    #[test]
    fn golden_devcontainer_refs_resolve_as_python_does() {
        for &(raw, expected) in GOLDEN_DEVCONTAINERS {
            let got = resolve_devcontainer_ref(raw).map(|path| path.as_str().to_string());
            assert_eq!(
                got.as_deref().map_err(|refusal| *refusal),
                expected,
                "{raw:?}"
            );
        }
    }

    // ---- the classification, one behaviour per test -----------------------

    #[test]
    fn a_path_spec_is_a_dot_slash_a_slash_or_a_tilde() {
        for spec in ["./my-project", "/home/user/project", "~/projects/test"] {
            assert!(matches!(parse(spec), WorkspaceSpec::Path(_)), "{spec}");
        }
        for spec in ["myworkspace", "owner/repo"] {
            assert!(!matches!(parse(spec), WorkspaceSpec::Path(_)), "{spec}");
        }
    }

    #[test]
    fn a_git_source_is_owner_repo_a_host_path_or_a_url() {
        for spec in [
            "owner/repo",
            "blooop/devlaunch@main",
            "github.com/owner/repo",
            "gitlab.com/owner/repo",
            "https://github.com/owner/repo",
        ] {
            assert!(parse(spec).is_git_source(), "{spec}");
        }
        for spec in [
            "myworkspace",
            "./my-project",
            "git@github.com:owner/repo.git",
        ] {
            assert!(!parse(spec).is_git_source(), "{spec}");
        }
    }

    #[test]
    fn the_owner_repo_rule_accepts_what_pythons_pattern_accepted() {
        for spec in [
            "owner/repo",
            "loft-sh/devpod",
            "user.name/repo.name",
            "my_user/my_repo",
            "owner/repo@main",
            "owner/repo@feature/my-feature",
            "owner/repo@br%20x",
        ] {
            assert!(
                matches!(parse(spec), WorkspaceSpec::OwnerRepo { .. }),
                "{spec}"
            );
        }
        for spec in [
            "workspace",
            "./path/to/project",
            "/home/user/project",
            "a/b/c",
            "owner/repo@",
            "owner/repo@a b",
        ] {
            assert!(
                !matches!(parse(spec), WorkspaceSpec::OwnerRepo { .. }),
                "{spec}"
            );
        }
    }

    #[test]
    fn owner_repo_splits_the_branch_at_the_first_at_sign() {
        assert_eq!(
            parse("blooop/devlaunch"),
            WorkspaceSpec::OwnerRepo {
                owner: "blooop",
                repo: "devlaunch",
                branch: None,
            }
        );
        assert_eq!(
            parse("owner/repo@feature/my-branch"),
            WorkspaceSpec::OwnerRepo {
                owner: "owner",
                repo: "repo",
                branch: Some("feature/my-branch"),
            }
        );
    }

    #[test]
    fn a_path_with_an_at_sign_is_still_a_path() {
        for spec in [
            "./my-project@foo",
            "/home/user/project@branch",
            "~/projects/test@main",
        ] {
            assert!(matches!(parse(spec), WorkspaceSpec::Path(_)), "{spec}");
        }
    }

    #[test]
    fn a_url_with_an_at_sign_is_still_a_url() {
        for spec in [
            "https://github.com/owner/repo@main",
            "github.com/owner/repo@branch",
        ] {
            assert!(
                !matches!(parse(spec), WorkspaceSpec::OwnerRepo { .. }),
                "{spec}"
            );
        }
    }

    #[test]
    fn an_ssh_url_is_told_from_owner_repo_by_its_colon() {
        for spec in [
            "git@github.com:owner/repo.git",
            "git@github.com:owner/repo.git@feature/my-branch",
            "git@gitlab.com:owner/repo.git",
            "git@bitbucket.org:owner/repo.git",
            "git@enterprise.example.com:owner/repo.git",
        ] {
            assert!(matches!(parse(spec), WorkspaceSpec::SshUrl(_)), "{spec}");
        }
        assert!(!matches!(parse("x@a/b"), WorkspaceSpec::SshUrl(_)));
    }

    #[test]
    fn anything_else_is_taken_for_an_existing_workspace() {
        for spec in ["myworkspace", "my-workspace", "workspace", "-x", "a/b/c"] {
            assert!(
                matches!(parse(spec), WorkspaceSpec::ExistingIdOrName(_)),
                "{spec}"
            );
        }
    }

    // ---- expansion --------------------------------------------------------

    #[test]
    fn owner_repo_expands_to_an_ssh_url() {
        assert_eq!(
            parse("loft-sh/devpod").expanded(),
            "git@github.com:loft-sh/devpod.git"
        );
        assert_eq!(
            parse("blooop/devlaunch@main").expanded(),
            "git@github.com:blooop/devlaunch.git@main"
        );
        assert_eq!(
            parse("owner/repo@feature/my-branch").expanded(),
            "git@github.com:owner/repo.git@feature/my-branch"
        );
    }

    #[test]
    fn everything_else_expands_to_itself() {
        for spec in [
            "git@github.com:owner/repo.git",
            "git@github.com:owner/repo.git@feature/my-branch",
            "git@gitlab.com:owner/repo.git",
            "git@bitbucket.org:owner/repo.git",
            "git@enterprise.example.com:owner/repo.git",
            "./my-project",
            "/home/user/project",
            "~/projects/test",
            "github.com/owner/repo",
            "gitlab.com/owner/repo",
            "https://github.com/owner/repo",
            "myworkspace",
            "my-workspace",
        ] {
            assert_eq!(parse(spec).expanded(), spec);
        }
    }

    // ---- identity ---------------------------------------------------------

    #[test]
    fn owner_repo_with_a_branch_derives_the_triples_id() {
        assert_eq!(
            observed("blooop/devlaunch@main"),
            Expect::Workspace("devlaunch-main-zovomobo")
        );
    }

    #[test]
    fn owner_repo_without_a_branch_is_a_repo_label_not_an_identity() {
        // A workspace is a branch checkout, so there is no ref to hash and nothing
        // to identify. The arm exists so a caller cannot spend a label as an id by
        // accident: it resolves the default branch first and derives from the triple.
        assert_eq!(observed("blooop/devlaunch"), Expect::RepoLabel("devlaunch"));
        assert_eq!(observed("blooop/test_renv"), Expect::RepoLabel("test-renv"));
        assert_eq!(
            observed("blooop/python_template"),
            Expect::RepoLabel("python-template")
        );
    }

    #[test]
    fn the_repo_label_is_capped_like_every_other_id() {
        let label = format!("owner/{}", "r".repeat(60));
        match identity(&label) {
            Ok(SpecIdentity::RepoLabel(value)) => {
                assert!(value.len() <= crate::domain::workspace_id::TARGET_LENGTH)
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn repo_case_does_not_fork_the_workspace() {
        let ids: std::collections::HashSet<Expect> = [
            "blooop/devlaunch@main",
            "Blooop/devlaunch@main",
            "blooop/DevLaunch@main",
            "BLOOOP/DEVLAUNCH@main",
        ]
        .into_iter()
        .map(observed)
        .collect();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn branch_case_does_fork_the_workspace() {
        assert_ne!(
            observed("blooop/devlaunch@main"),
            observed("blooop/devlaunch@Main")
        );
    }

    #[test]
    fn the_owner_is_part_of_the_identity() {
        assert_ne!(
            observed("blooop/devlaunch@main"),
            observed("someone/devlaunch@main")
        );
    }

    #[test]
    fn slash_and_dash_branches_are_distinct() {
        assert_ne!(
            observed("blooop/devlaunch@feature/auth"),
            observed("blooop/devlaunch@feature-auth")
        );
    }

    #[test]
    fn a_url_spec_is_slugged_suffixed_and_capped() {
        for spec in [
            "github.com/loft-sh/devpod",
            "https://github.com/owner/repo",
            &format!("github.com/{}/{}", "o".repeat(40), "r".repeat(40)),
        ] {
            match identity(spec) {
                Ok(SpecIdentity::Workspace(value)) => {
                    assert!(
                        value.len() <= crate::domain::workspace_id::TARGET_LENGTH,
                        "{value}"
                    );
                }
                other => panic!("{spec}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_urls_git_suffix_and_protocol_do_not_change_its_identity() {
        assert_eq!(
            observed("github.com/owner/repo.git"),
            observed("github.com/owner/repo")
        );
        assert_eq!(
            observed("https://github.com/owner/repo"),
            observed("github.com/owner/repo")
        );
        assert_eq!(
            observed("github.com/Blooop/DevLaunch"),
            observed("github.com/blooop/devlaunch")
        );
    }

    #[test]
    fn url_specs_are_injective_where_their_slugs_collide() {
        let ids: std::collections::HashSet<Expect> = [
            "gitlab.com/group/my_repo",
            "gitlab.com/group/my-repo",
            "gitlab.com/group/my.repo",
        ]
        .into_iter()
        .map(observed)
        .collect();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn an_owner_repo_spec_whose_at_suffix_is_no_branch_falls_back_to_the_url_rule() {
        // Python's OWNER_REPO_PATTERN refuses these, but `base_spec` is still
        // `owner/repo`, so `is_git_spec` sends them down the URL path: the id
        // becomes the one for `github.com/owner/repo`, branch and all discarded.
        let host_path = observed("github.com/owner/repo");
        for spec in [
            "owner/repo@",
            "owner/repo@br@x",
            "owner/repo@a b",
            "owner/repo@a:b",
        ] {
            assert_eq!(observed(spec), host_path, "{spec}");
        }
    }

    #[test]
    fn a_path_spec_leaves_the_leaf_to_the_filesystem() {
        assert_eq!(
            identity("./my-project@foo"),
            Ok(SpecIdentity::PathLeaf("./my-project"))
        );
        assert_eq!(
            identity("/home/user/project"),
            Ok(SpecIdentity::PathLeaf("/home/user/project"))
        );
    }

    #[test]
    fn an_existing_name_is_returned_whole_at_sign_and_all() {
        assert_eq!(
            identity("myworkspace"),
            Ok(SpecIdentity::ExistingName("myworkspace"))
        );
        assert_eq!(
            identity("myworkspace@foo"),
            Ok(SpecIdentity::ExistingName("myworkspace@foo"))
        );
        // An ssh URL lands here too: `base_spec` is the part before the first `@`,
        // which is a bare `git`, so nothing recognises it as a git source.
        assert_eq!(
            identity("git@github.com:owner/repo.git"),
            Ok(SpecIdentity::ExistingName("git@github.com:owner/repo.git"))
        );
    }

    #[test]
    fn different_branches_are_different_workspaces() {
        let nb12 = identity("blooop/test_renv@nb12");
        let nb14 = identity("blooop/test_renv@nb14");
        assert_ne!(nb12, nb14);
        for got in [&nb12, &nb14] {
            match got {
                Ok(SpecIdentity::Workspace(value)) => assert!(value.starts_with("test-renv-nb1")),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_long_branch_is_truncated_without_a_trailing_dash() {
        for spec in [
            &format!("owner/repo@{}", "a".repeat(60)),
            "owner/myrepo@feature/some-very-long-branch-name-here",
        ] {
            match identity(spec) {
                Ok(SpecIdentity::Workspace(value)) => {
                    assert!(value.len() <= crate::domain::workspace_id::TARGET_LENGTH);
                    assert!(!value.ends_with('-'), "{value}");
                }
                other => panic!("{spec}: {other:?}"),
            }
        }
    }

    #[test]
    fn an_unsafe_branch_is_refused_before_it_can_name_anything() {
        assert_eq!(
            identity("owner/repo@bad%branch"),
            Err(UnsafeName {
                part: NamePart::Ref,
                name: "bad%branch".to_string(),
            })
        );
    }

    #[test]
    fn a_traversal_in_the_owner_or_repo_is_refused_by_the_same_boundary() {
        // `%` and `..` both survive OWNER_REPO_PATTERN, so the constructor is what
        // stops them -- before anything joins repos_dir/<owner>/<repo>.
        assert_eq!(
            identity("x/..@main"),
            Err(UnsafeName {
                part: NamePart::Repo,
                name: "..".to_string(),
            })
        );
        assert_eq!(
            identity("../x@main"),
            Err(UnsafeName {
                part: NamePart::Owner,
                name: "..".to_string(),
            })
        );
    }

    // ---- the --devcontainer value ----------------------------------------

    #[test]
    fn a_bare_devcontainer_name_becomes_a_variant_path() {
        assert_eq!(
            resolve_devcontainer_ref("ubuntu").map(|p| p.as_str().to_string()),
            Ok(".devcontainer/ubuntu/devcontainer.json".to_string())
        );
    }

    #[test]
    fn a_devcontainer_value_with_a_separator_or_json_suffix_is_used_as_given() {
        for raw in [
            "x/y",
            "a.json",
            ".devcontainer/x/devcontainer.json",
            "/abs/path.json",
        ] {
            assert_eq!(
                resolve_devcontainer_ref(raw).map(|p| p.as_str().to_string()),
                Ok(raw.to_string())
            );
        }
    }

    #[test]
    fn a_missing_devcontainer_value_is_refused() {
        for raw in ["", " ", "\t", "\u{a0}", "\u{1c}", "\u{1f}"] {
            assert_eq!(
                resolve_devcontainer_ref(raw),
                Err(DevcontainerRefError::Missing),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_flag_where_the_devcontainer_value_should_be_is_refused() {
        for raw in ["-x", "--help"] {
            assert_eq!(
                resolve_devcontainer_ref(raw),
                Err(DevcontainerRefError::FlagLike),
                "{raw:?}"
            );
        }
    }
}
