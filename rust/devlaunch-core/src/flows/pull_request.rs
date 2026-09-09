//! Turn a pull request reference into the spec it stands for.
//!
//! [`domain::pull_request`](crate::domain::pull_request) says whether the string
//! in the user's hand names a pull request; [`clients::gh`](crate::clients::gh)
//! says which branch that request's head is. This is the two of them joined, and
//! its whole output is **another spec** — `owner/repo@branch`, in the head
//! repository — which is why it sits here and not in either.
//!
//! # A rewrite, and deliberately nothing more
//!
//! The resolution ends by handing back a string that goes on to be classified
//! exactly as though it had been typed. That is the cheap part of this feature and
//! the reason it is small: a pull request is a branch in a repository, `dl`
//! already opens branches in repositories, and everything a workspace is — the
//! derived id, the clone directory, the metadata record, the terminal title, what
//! `dl --ls` shows, what `dl <ws> rm` removes — follows from the triple with
//! nothing here to teach it. Two workspaces opened from the same branch by its
//! link and by its name are the *same workspace*, and they are the same workspace
//! because there is one spec by the time anything downstream looks.
//!
//! It also means the rewrite has to happen **once**, before any use of the target
//! word. `dl <spec> --rm` resolves its target twice, on the way in and again on
//! the way out, and the two must not disagree about what they are removing; a
//! second lookup could answer differently, since a request's head branch can be
//! force-pushed or the request retargeted between them. The `dl` binary does it
//! in `dispatch`, which is the one place a typed target exists before anything
//! reads it.
//!
//! # The head repository, not the base one
//!
//! The rewritten spec names the repository the **branch** is in. For a request
//! from a fork that is not the repository the link spelled, and it is not a
//! subtlety that can be skipped: `owner/repo@their-branch` against the base
//! repository either finds nothing or finds an unrelated branch that happens to
//! share a name. `dl` clones the fork, which is what checking out that branch
//! means, and the workspace is named after the fork accordingly.

use crate::clients::gh::{self, PullRequestHead, PullRequestUnavailable};
use crate::domain::pull_request::{self, Classified, Malformed, PullRequestRef};
use crate::domain::spec::{self, WorkspaceSpec};
use crate::runner::Runner;

/// What a raw spec turned out to mean.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Resolved {
    /// No pull request in it. The spec means what it says, and **nothing was
    /// asked of the network** — this is the arm every ordinary `dl` invocation
    /// takes, so it must cost nothing.
    AsTyped,
    /// The spec named a pull request, and this is the spec it stands for.
    Rewritten {
        /// `owner/repo@branch`, in the head repository. Ready to be classified.
        spec: String,
        /// What the user's string named, for a report that repeats it back.
        named: PullRequestRef,
        /// What GitHub said, including the state — a merged request usually has
        /// no head branch left, and that is worth saying before the checkout
        /// fails.
        head: PullRequestHead,
    },
    /// The spec named a pull request and no spec could be got out of it.
    Refused(Refusal),
}

/// Why a pull request reference produced no spec.
#[derive(Clone, Debug, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Refusal {
    /// The reference itself does not parse. Nothing was asked of GitHub.
    Malformed(Malformed),
    /// GitHub was asked and could not answer. The reference is carried alongside
    /// so a diagnostic can name which request it was about.
    NotLookedUp {
        named: PullRequestRef,
        why: PullRequestUnavailable,
    },
    /// GitHub answered, and the spec that answer spells is not one dl reads back
    /// as the triple it was built from.
    ///
    /// git allows names dl's own `owner/repo@branch` grammar cannot carry —
    /// anything with an `@` or a `:` in it, most obviously — and the offender is
    /// usually the branch but need not be. A rewrite that produced one would be
    /// re-read as some *other* spec, which is a wrong workspace rather than an
    /// error, so it is refused here instead. Carries the whole spec rather than
    /// the branch alone, so a diagnostic never has to guess which part of it was
    /// unspellable. Rare enough that no known request has one, and cheap enough
    /// to rule out.
    Unspellable {
        named: PullRequestRef,
        spelled: String,
    },
}

/// The spec `raw` stands for, asking GitHub only if `raw` names a pull request.
// binary surface — not part of the frozen wf API (#251 §7)
pub fn resolve(runner: &dyn Runner, raw: &str) -> Resolved {
    let named = match pull_request::classify(raw) {
        Classified::NotOne => return Resolved::AsTyped,
        Classified::Malformed(malformed) => {
            return Resolved::Refused(Refusal::Malformed(malformed));
        }
        Classified::Names(named) => named,
    };
    let head = match gh::pull_request_head(runner, &named.owner, &named.repo, named.number) {
        Ok(head) => head,
        Err(why) => return Resolved::Refused(Refusal::NotLookedUp { named, why }),
    };
    // The invariant the whole rewrite rests on, checked directly rather than
    // approximated by character classes: the string handed on must read back as
    // the very triple it was built from. git allows names dl's own
    // `owner/repo@branch` grammar cannot carry -- a branch with an `@` or a `:`
    // in it, most obviously -- and a spec that re-reads as some *other* spec is a
    // wrong workspace rather than an error. Asking `spec::parse` is what makes
    // this a guard over the grammar rather than a second copy of it.
    let spelled = format!("{}/{}@{}", head.owner, head.repo, head.branch);
    let intended = WorkspaceSpec::OwnerRepo {
        owner: &head.owner,
        repo: &head.repo,
        branch: Some(&head.branch),
    };
    if spec::parse(&spelled) != intended {
        return Resolved::Refused(Refusal::Unspellable { named, spelled });
    }
    Resolved::Rewritten {
        spec: spelled,
        named,
        head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::gh::PullRequestState;
    use crate::testing::ScriptedRunner;
    use devlaunch_test_support::Response;

    /// The argv the lookup must issue, spelled out rather than built from
    /// `gh::PR_FIELDS`. It is a second copy of that list on purpose: the head
    /// repository is the whole reason this call exists, so dropping
    /// `headRepository` or `headRepositoryOwner` from it has to break a test
    /// rather than quietly resolve every fork request to the base repository.
    const VIEW: &[&str] = &[
        "gh",
        "pr",
        "view",
        "579",
        "--repo",
        "blooop/devlaunch",
        "--json",
        "headRefName,headRepository,headRepositoryOwner,state",
    ];

    fn answering(json: &str) -> ScriptedRunner {
        ScriptedRunner::new().with_script(VIEW.iter().copied(), Response::stdout(json))
    }

    #[test]
    fn an_ordinary_spec_asks_nothing_of_github() {
        let fake = ScriptedRunner::new();
        assert_eq!(resolve(&fake, "blooop/devlaunch@main"), Resolved::AsTyped);
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_link_becomes_the_head_branchs_own_spec() {
        let fake = answering(
            r#"{"headRefName":"pr_link","headRepository":{"name":"devlaunch"},
                "headRepositoryOwner":{"login":"blooop"},"state":"OPEN"}"#,
        );
        let resolved = resolve(
            &fake,
            "blooop/devlaunch@https://github.com/blooop/devlaunch/pull/579",
        );
        let Resolved::Rewritten { spec, head, named } = resolved else {
            panic!("{resolved:?}");
        };
        assert_eq!(spec, "blooop/devlaunch@pr_link");
        assert_eq!(head.state, PullRequestState::Open);
        assert_eq!(named.number, 579);
    }

    #[test]
    fn a_request_from_a_fork_rewrites_to_the_fork() {
        // The point of the lookup: the base repository has no `their-branch`, and
        // if it had one it would be somebody else's.
        let fake = answering(
            r#"{"headRefName":"their-branch","headRepository":{"name":"devlaunch"},
                "headRepositoryOwner":{"login":"contributor"},"state":"OPEN"}"#,
        );
        let resolved = resolve(&fake, "https://github.com/blooop/devlaunch/pull/579");
        let Resolved::Rewritten { spec, .. } = resolved else {
            panic!("{resolved:?}");
        };
        assert_eq!(spec, "contributor/devlaunch@their-branch");
    }

    #[test]
    fn a_merged_request_still_resolves_and_says_that_it_merged() {
        let fake = answering(
            r#"{"headRefName":"landed","headRepository":{"name":"devlaunch"},
                "headRepositoryOwner":{"login":"blooop"},"state":"MERGED"}"#,
        );
        let resolved = resolve(&fake, "blooop/devlaunch@#579");
        let Resolved::Rewritten { spec, head, .. } = resolved else {
            panic!("{resolved:?}");
        };
        assert_eq!(spec, "blooop/devlaunch@landed");
        assert_eq!(head.state, PullRequestState::Merged);
    }

    #[test]
    fn a_malformed_reference_costs_no_round_trip() {
        let fake = ScriptedRunner::new();
        let resolved = resolve(&fake, "https://github.com/blooop/devlaunch/pull/abc");
        assert!(matches!(
            resolved,
            Resolved::Refused(Refusal::Malformed(Malformed::NotANumber { .. }))
        ));
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_gh_that_refuses_names_the_request_it_refused_about() {
        let fake = ScriptedRunner::new().with_script(
            VIEW.iter().copied(),
            Response::failed(1, "no pull requests found for branch\n"),
        );
        let resolved = resolve(&fake, "blooop/devlaunch@#579");
        let Resolved::Refused(Refusal::NotLookedUp { named, why }) = resolved else {
            panic!("{resolved:?}");
        };
        assert_eq!(named.number, 579);
        assert!(matches!(
            why,
            PullRequestUnavailable::Refused { ref reason, .. }
                if reason == "no pull requests found for branch"
        ));
    }

    #[test]
    fn no_gh_on_path_is_its_own_refusal() {
        let fake = ScriptedRunner::new().with_missing("gh");
        let resolved = resolve(&fake, "blooop/devlaunch@#579");
        assert!(matches!(
            resolved,
            Resolved::Refused(Refusal::NotLookedUp {
                why: PullRequestUnavailable::GhMissing,
                ..
            })
        ));
    }

    #[test]
    fn a_spec_dl_would_read_back_as_something_else_is_refused_rather_than_rewritten() {
        // Two branch names, one for each way the round trip can fail. `we@ird`
        // re-reads as an existing workspace name, and `wei:rd` as an scp-style git
        // remote. Either way the rewrite would have opened something other than
        // the branch the request is on.
        for branch in ["we@ird", "wei:rd"] {
            let fake = answering(&format!(
                r#"{{"headRefName":"{branch}","headRepository":{{"name":"devlaunch"}},
                    "headRepositoryOwner":{{"login":"blooop"}},"state":"OPEN"}}"#
            ));
            let resolved = resolve(&fake, "blooop/devlaunch@#579");
            let Resolved::Refused(Refusal::Unspellable { spelled, .. }) = resolved else {
                panic!("{branch}: {resolved:?}");
            };
            assert_eq!(spelled, format!("blooop/devlaunch@{branch}"));
        }
    }

    #[test]
    fn a_deleted_fork_has_no_branch_to_open() {
        let fake = answering(
            r#"{"headRefName":"gone","headRepository":null,
                "headRepositoryOwner":null,"state":"CLOSED"}"#,
        );
        let resolved = resolve(&fake, "blooop/devlaunch@#579");
        assert!(matches!(
            resolved,
            Resolved::Refused(Refusal::NotLookedUp {
                why: PullRequestUnavailable::HeadRepositoryGone,
                ..
            })
        ));
    }

    #[test]
    fn junk_where_the_document_should_be_is_not_read_as_a_branch() {
        let fake = answering("<html>rate limited</html>");
        let resolved = resolve(&fake, "blooop/devlaunch@#579");
        assert!(matches!(
            resolved,
            Resolved::Refused(Refusal::NotLookedUp {
                why: PullRequestUnavailable::Unreadable,
                ..
            })
        ));
    }
}
