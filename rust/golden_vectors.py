"""Emit the golden-vector tables for the Rust port as Rust source.

Run from the worktree root with `pixi run python`. Every expected value in the
Rust tests comes from here, so nothing is transcribed by hand.
"""

import sys
import tempfile

from devlaunch.workspace_id import (
    WorkspaceId,
    slug,
    source_workspace_id,
    validate_ref_name,
)
from devlaunch.dl import (
    expand_workspace_spec,
    spec_to_workspace_id,
    is_path_spec,
    is_git_spec,
    parse_owner_repo_branch,
    _resolve_devcontainer_ref,
)


def rs(s):
    out = '"'
    for ch in s:
        o = ord(ch)
        if ch == '"':
            out += '\\"'
        elif ch == "\\":
            out += "\\\\"
        elif ch == "\n":
            out += "\\n"
        elif ch == "\r":
            out += "\\r"
        elif ch == "\t":
            out += "\\t"
        elif o < 0x20 or o > 0x7E:
            out += "\\u{%x}" % o
        else:
            out += ch
    return out + '"'


TRIPLES = [
    ("blooop", "devlaunch", "main"),
    ("blooop", "wayfinder", "main"),
    ("blooop", "devlaunch", "feature/auth"),
    ("blooop", "devlaunch", "feature-auth"),
    ("blooop", "devlaunch", "feature.auth"),
    ("blooop", "devlaunch", "feature_auth"),
    ("blooop", "devlaunch", "featureauth"),
    ("blooop", "devlaunch", "Main"),
    ("NVIDIA", "cuda-samples", "main"),
    ("nvidia", "cuda-samples", "main"),
    ("BlOoOp", "dEvLaUnCh", "main"),
    ("owner", "My_Repo.git", "Feature/MyBranch"),
    ("blooop", "devlaunch", "dependabot/github_actions/codecov/codecov-action-6"),
    ("blooop", "devlaunch", "dependabot/github_actions/blooop/prek-action-2"),
    ("blooop", "python_template", "dependabot/pip/lib/dependencies-1"),
    ("blooop", "lifetime_foc_rig", "dependabot/github_actions/codecov/codecov-action-6"),
    ("kinisi-robotics", "kinisi_ros", "ags-devcontainer-tooling-support"),
    ("blooop", "devlaunch", "fix/gh-auth-in-devcontainer"),
    ("blooop", "test_renv", "nb4"),
    ("blooop", "dl", "a/bbbbbbbb/cccccccc/dddddddd/zzz"),
    ("blooop", "dl", "a/" + "b" * 12 + "/" + "c" * 12 + "/" + "d" * 12 + "/zzz"),
    ("blooop", "dl", "aa/" + "m" * 40 + "/" + "n" * 40 + "/zz"),
    ("blooop", "devlaunch", "a//b///c/d"),
    ("blooop", "devlaunch", "a" * 60),
    ("owner", "r" * 47, "main"),
    ("owner", "r" * 47, "a/very/long/branch/name/that/eats/the/budget"),
    ("owner", "r" * 20, "a/very/long/branch/name/that/eats/the/budget"),
    ("owner", "repo", "release/9999999999999999999999999176"),
    ("owner", "repo", "release/9999999999999999999999999234"),
    ("a", "bc", "main"),
    ("ab", "c", "main"),
    ("anyone", "github.com", "owner/repo"),
    ("blooop", "a-repo-name-that-is-long", "feature/shared-prefix-1"),
    ("blooop", "a-repo-name-that-is-long", "feature/shared-prefix-2"),
    ("owner", "repo", "v1.2.3"),
    ("owner", "repo", "release_1"),
    ("owner", "r", "b" * 80),
    ("owner", "r" * 80, "b" * 80),
    ("owner", "repo", "caf\u00e9"),
    ("owner", "repo", "\u5206\u652f"),
    ("owner", "repo", "\u0432\u0435\u0442\u043a\u0430"),
    ("owner", "repo", "\u0661\u0662\u0663"),
    ("owner", "KKK", "main"),
    ("owner", "repo", "x\n"),
    ("owner", "repo", "\u0130stanbul"),
    ("owner", "\u212a", "main"),
]

SOURCES = [
    "github.com/owner/repo",
    "github.com/loft-sh/devpod",
    "gitlab.com/group/my_repo",
    "gitlab.com/group/my-repo",
    "gitlab.com/group/my.repo",
    "github.com/Blooop/DevLaunch",
    "github.com/blooop/devlaunch",
    "github.com/" + "o" * 40 + "/" + "r" * 40,
    "github.com/" + "o" * 200 + "/" + "r" * 200,
    "github.com/o/r",
    "o",
    "github.com/owner/repo.git",
    "git@github.com:owner/repo.git",
    "GIT@GitHub.COM:Owner/Repo",
    "github.com/owner/repo-",
    "-----",
    "",
    "\u5206\u652f",
]

SLUGS = [
    "main",
    "Feature/MyBranch",
    "my_repo",
    "python_template",
    "v1.2.3",
    "--evil--",
    "a...b",
    "",
    "-",
    "---",
    "MAIN",
    "\u00dcn\u00efc\u00f4de",
    "\u5206\u652f",
    "\u212a",
    "\u0130stanbul",
    "\u03a3\u039f\u03a6\u039f\u03a3",
    "a b c",
    "\uff21\uff22\uff23",
    "\uff11\uff12\uff13",
    "\u00bd",
    "\u24b6",
    "caf\u00e9",
    "main\u0301",
    "x\n",
    "x\n\n",
    "a/b\n",
    "x ",
    "x\t",
]

NAMES = [
    "main",
    "feature/my-branch",
    "v1.2.3",
    "release_1",
    "a.b",
    "a-b",
    "a/b",
    "_x",
    ".x",
    "",
    "--evil",
    "-x",
    "branch name",
    "a;b",
    "..",
    "a%b",
    "a b",
    "a@b",
    "a:b",
    "a\\b",
    "caf\u00e9",
    "\u0432\u0435\u0442\u043a\u0430",
    "\u5206\u652f",
    "\u0661\u0662\u0663",
    "\u00bd",
    "\u00b2",
    "\uff21\uff22\uff23",
    "\u0130",
    "\u4e94",
    "x\n",
    "x\n\n",
    "\n",
    "x\ny",
    "x\r",
    "x\r\n",
    "main\n",
    "a/b\n",
    "x ",
    "x\t",
    "\u212a",
]

# Non-ASCII where Python's `\w` (str.isalnum() plus underscore) and Rust's
# char::is_alphanumeric() disagree. Measured, not guessed.
DIVERGENT = ["\u24b6", "x\u24b6", "\u093e", "x\u093e", "\u05bf", "x\u0903"]

SPECS = [
    "blooop/devlaunch",
    "blooop/devlaunch@main",
    "owner/repo@feature/my-branch",
    "Owner/Repo@Feature/MyBranch",
    "blooop/test_renv",
    "blooop/test_renv@nb12",
    "blooop/test_renv@nb14",
    "blooop/python_template",
    "blooop/python_template@nb4",
    "loft-sh/devpod",
    "owner/" + "r" * 60,
    "owner/" + "r" * 47 + "@main",
    "owner/repo@" + "a" * 60,
    "owner/myrepo@feature/some-very-long-branch-name-here",
    "user.name/repo.name",
    "my_user/my_repo",
    "owner/repo@main",
    "owner/repo@Main",
    "someone/devlaunch@main",
    "blooop/devlaunch@feature/auth",
    "blooop/devlaunch@feature-auth",
    "owner/repo@bad%branch",
    "owner/repo@br%20x",
    "owner/repo@",
    "owner/repo@br@x",
    "owner/repo@a b",
    "owner/repo@a:b",
    "owner/repo@..",
    "owner/repo@-x",
    "owner/repo@feature/../x",
    "owner/repo\n",
    "owner/repo@br\n",
    "owner/repo@br\nx",
    "github.com/owner/repo",
    "github.com/owner/repo.git",
    "github.com/owner/repo@branch",
    "gitlab.com/owner/repo",
    "gitlab.com/group/my_repo",
    "gitlab.com/group/my-repo",
    "gitlab.com/group/my.repo",
    "github.com/owner",
    "github.com/loft-sh/devpod",
    "github.com/Blooop/DevLaunch",
    "github.com/blooop/devlaunch",
    "github.com/" + "o" * 40 + "/" + "r" * 40,
    "https://github.com/owner/repo",
    "https://github.com/owner/repo@main",
    "https://github.com/owner/repo.git",
    "https://user@host/x",
    "ssh://git@github.com/o/r.git",
    "git@github.com:owner/repo.git",
    "git@github.com:owner/repo.git@feature/my-branch",
    "git@gitlab.com:owner/repo.git",
    "git@bitbucket.org:owner/repo.git",
    "git@enterprise.example.com:owner/repo.git",
    "myworkspace",
    "my-workspace",
    "myworkspace@foo",
    "workspace",
    "x@a/b",
    "./my-project",
    "/home/user/project",
    "~/projects/test",
    "./my-project@foo",
    "/home/user/project@branch",
    "~/projects/test@main",
    "./path/to/project",
    "a/b",
    "a//b",
    "a/b/c",
    "@",
    "@main",
    "/",
    "~",
    ".",
    "..",
    "-x",
    "x-",
    "x@y://z",
    "a/b@c:d",
]

DEVCONTAINERS = [
    "ubuntu",
    "alt",
    "x/y",
    "a.json",
    ".devcontainer/x/devcontainer.json",
    "foo/bar.json",
    "",
    " ",
    "\t",
    "\u00a0",
    "\u001c",
    "\u001f",
    "-x",
    "--help",
    "a-b",
    "a b",
    "x.json",
    "/abs/path.json",
    "a/b/c",
    "json",
]


def shape(spec):
    """Which arm `parse` must return for *spec*, as Rust source."""
    if is_path_spec(spec):
        return "Shape::Path"
    if "://" in spec:
        return "Shape::Url"
    if spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return "Shape::HostPath"
    import re

    if re.match(r"^[^@]+@[^:]+:", spec):
        return "Shape::SshUrl"
    parsed = parse_owner_repo_branch(spec)
    if parsed is not None:
        owner_repo, branch = parsed
        owner, repo = owner_repo.split("/", 1)
        b = "None" if branch is None else "Some(%s)" % rs(branch)
        return "Shape::OwnerRepo { owner: %s, repo: %s, branch: %s }" % (rs(owner), rs(repo), b)
    return "Shape::ExistingIdOrName"


def identity(spec):
    base = spec.split("@", 1)[0]
    try:
        got = spec_to_workspace_id(spec)
    except ValueError:
        return "Expect::Unsafe"
    if is_path_spec(base):
        return "Expect::PathLeaf(%s)" % rs(base)
    if is_git_spec(base):
        parsed = parse_owner_repo_branch(spec)
        if parsed is not None and parsed[1] is None:
            return "Expect::RepoLabel(%s)" % rs(got)
        return "Expect::Workspace(%s)" % rs(got)
    return "Expect::ExistingName(%s)" % rs(got)


def main(out_dir=None):
    """Write both tables into `out_dir`, defaulting to the system temp dir.

    A function rather than module-level script body, so the loop variables in
    here are locals: at module level they were globals that every helper above
    shadowed by taking a parameter of the same name, which reads as a mistake
    even where it is not one.

    A directory argument rather than a constant, because the output is copied
    into the Rust tests by hand and nothing reads it from a fixed place -- and
    because the alternative, a path baked in from whichever machine last ran
    this, is a script only that machine can run.
    """
    out = (out_dir or tempfile.gettempdir()).rstrip("/") + "/"
    with open(out + "wid_tables.rs.txt", "w", encoding="utf-8") as f:
        w = f.write
        w("    /// (owner, repo, ref) -> (suffix, value), straight out of the Python\n")
        w("    /// implementation. FROZEN: every workspace directory and devpod workspace\n")
        w("    /// on disk is named by this output.\n")
        w("    const GOLDEN_TRIPLES: &[(&str, &str, &str, &str, &str)] = &[\n")
        for owner, repo, ref in TRIPLES:
            ws = WorkspaceId(owner, repo, ref)
            w(
                "        (%s, %s, %s, %s, %s),\n"
                % (rs(owner), rs(repo), rs(ref), rs(ws.suffix), rs(ws.value))
            )
        w("    ];\n\n")
        w("    /// source -> id, for git sources that name no ref.\n")
        w("    const GOLDEN_SOURCES: &[(&str, &str)] = &[\n")
        for source in SOURCES:
            w("        (%s, %s),\n" % (rs(source), rs(source_workspace_id(source))))
        w("    ];\n\n")
        w("    /// text -> slug.\n")
        w("    const GOLDEN_SLUGS: &[(&str, &str)] = &[\n")
        for text in SLUGS:
            w("        (%s, %s),\n" % (rs(text), rs(slug(text))))
        w("    ];\n\n")
        w("    /// name -> whether Python's `^[\\w][\\w./-]*$` accepts it.\n")
        w("    const GOLDEN_SAFE_NAMES: &[(&str, bool)] = &[\n")
        for n in NAMES:
            try:
                validate_ref_name(n)
                ok = "true"
            except ValueError:
                ok = "false"
            w("        (%s, %s),\n" % (rs(n), ok))
        w("    ];\n\n")
        w("    /// Names Python refuses and this port accepts: see the module docs.\n")
        w("    const DIVERGENT_NAMES: &[&str] = &[\n")
        for n in DIVERGENT:
            try:
                validate_ref_name(n)
                raise SystemExit("expected Python to refuse %r" % n)
            except ValueError:
                pass
            w("        %s,\n" % rs(n))
        w("    ];\n")

    with open(out + "spec_tables.rs.txt", "w", encoding="utf-8") as f:
        w = f.write
        w("    /// spec -> (shape, expansion, identity), straight out of the Python\n")
        w("    /// implementation (`dl.py`: parse_owner_repo_branch, is_path_spec,\n")
        w("    /// is_git_spec, expand_workspace_spec, spec_to_workspace_id).\n")
        w("    #[rustfmt::skip]\n")
        w("    fn golden_specs() -> Vec<Case> {\n")
        w("        vec![\n")
        for spec in SPECS:
            w(
                "            Case { spec: %s, shape: %s, expanded: %s, identity: %s },\n"
                % (rs(spec), shape(spec), rs(expand_workspace_spec(spec)), identity(spec))
            )
        w("        ]\n")
        w("    }\n\n")
        w("    /// --devcontainer value -> resolved path, or the refusal.\n")
        w("    const GOLDEN_DEVCONTAINERS: &[(&str, Result<&str, DevcontainerRefError>)] = &[\n")
        for d in DEVCONTAINERS:
            try:
                rendered = "Ok(%s)" % rs(_resolve_devcontainer_ref(d))
            except ValueError as exc:
                rendered = (
                    "Err(DevcontainerRefError::Missing)"
                    if "requires a variant" in str(exc)
                    else "Err(DevcontainerRefError::FlagLike)"
                )
            w("        (%s, %s),\n" % (rs(d), rendered))
        w("    ];\n")

    print("wrote tables")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else None)
