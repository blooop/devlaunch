//! A pull request the user named instead of a branch.
//!
//! `dl owner/repo@some-branch` says which checkout to open by naming the branch.
//! A pull request names the same thing indirectly, and it is what a person
//! actually has in their hand: a review request arrives as
//! `https://github.com/blooop/devlaunch/pull/579`, and turning that into a branch
//! name means opening the page and copying a string out of it. So the link is
//! accepted where the branch goes, and the branch is looked up.
//!
//! This module is the *pure* half — the string in the user's hand, classified.
//! The lookup that turns a [`PullRequestRef`] into a branch is
//! [`crate::clients::gh::pull_request_head`], because only GitHub knows the
//! answer and only a runner can ask it.
//!
//! # The three spellings, and why the set is closed
//!
//! - `owner/repo@<url>` — the shape asked for. The prefix is redundant with the
//!   URL, which is the point: it is what `dl`'s other specs look like, so it is
//!   what a person types without thinking. A prefix that *disagrees* with the URL
//!   is refused rather than resolved either way ([`Malformed::RepoMismatch`]):
//!   both readings are defensible, and a wrong guess opens a repository the user
//!   did not ask for.
//! - `<url>` alone — the URL is already fully qualified, so a prefix carries
//!   nothing. Nothing is given up by accepting it, either: a `/pull/` URL is not
//!   a clonable git source, so today it reaches devpod and fails there.
//! - `owner/repo@#579` — the short one. `#` is outside `dl`'s branch pattern, so
//!   this steals no spelling, where a bare `owner/repo@579` would: `579` is a
//!   legal branch name, and it has to keep meaning one.
//!
//! # What is deliberately not accepted
//!
//! **Any host but `github.com`.** A request number is resolved by asking GitHub,
//! and `gh` asks the host it was told about; a GitHub Enterprise URL would need
//! its host carried through the lookup and onto the call as `--hostname`. Nothing
//! here forecloses that — [`PullRequestRef`] would grow a host — but guessing
//! that an unrecognised host speaks GitHub's API is worse than not matching it.
//!
//! **A bare `owner/repo@579`.** See above.

use super::spec;

/// The pull request a spec names, as the *base* repository sees it.
///
/// This is the repository the request was opened **against**, which is what a URL
/// spells and what a lookup is addressed to. It is not necessarily where the
/// branch lives — a request from a fork has its head in another repository
/// entirely — so nothing here names a workspace. The lookup's answer does.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub struct PullRequestRef {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

/// What a raw spec turns out to say about a pull request.
///
/// Three arms and not `Option<Result<_, _>>`, because [`Classified::NotOne`] and
/// [`Classified::Malformed`] are different answers and a caller must not be able
/// to spend one for the other: `NotOne` means "classify this the ordinary way",
/// and `Malformed` means "stop, and say why". Collapsing them is the bug this
/// shape exists to prevent — a spec plainly meant as a link falling through to
/// [`spec::WorkspaceSpec::Url`] and reaching devpod, which reports that it cannot
/// clone `.../pull/579`: a true sentence about the wrong problem.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Classified {
    /// No pull request here. The spec means whatever [`spec::parse`] says.
    NotOne,
    /// The request this spec names.
    Names(PullRequestRef),
    /// Meant as a pull request reference, and not usable as one.
    Malformed(Malformed),
}

/// Why something meant as a pull request reference is not one.
///
/// Data rather than sentences (#251 §5): the words are the `dl` binary's.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum Malformed {
    /// `a/b@https://github.com/c/d/pull/5`. Both halves name a repository and
    /// they disagree, so neither is acted on.
    RepoMismatch {
        prefix_owner: String,
        prefix_repo: String,
        url_owner: String,
        url_repo: String,
    },
    /// A `/pull/` URL or a `#` suffix whose number is missing, not a number, or
    /// zero. Carries what stood there instead, because the reader's next move is
    /// to look at what they pasted.
    NotANumber { found: String },
}

/// The pull request `spec` names, if it names one.
///
/// Total, and it cannot fail: every string is one of the three arms.
// binary surface — not part of the frozen wf API (#251 §7)
pub fn classify(spec: &str) -> Classified {
    let spec = spec.trim();
    // A path is a path. `~/pull/579` is a directory on this machine, and the
    // patterns below have no business reading it.
    if spec.starts_with("./") || spec.starts_with('/') || spec.starts_with('~') {
        return Classified::NotOne;
    }
    match spec
        .split_once('@')
        .and_then(|(prefix, suffix)| owner_repo(prefix).map(|(o, r)| (o, r, suffix)))
    {
        Some((owner, repo, suffix)) => prefixed(owner, repo, suffix),
        // No usable `owner/repo@` prefix, so the whole spec has to be the URL.
        None => classify_url(spec),
    }
}

/// `owner/repo@<suffix>`, the prefix having already parsed as an owner and a repo.
///
/// The suffix is either `#<number>`, which says nothing about *which* repository
/// and so leaves the prefix as the only answer there is, or a URL, which says
/// everything and therefore has to agree with the prefix.
fn prefixed(prefix_owner: &str, prefix_repo: &str, suffix: &str) -> Classified {
    if let Some(digits) = suffix.strip_prefix('#') {
        return match pr_number(digits) {
            Some(number) => Classified::Names(PullRequestRef {
                owner: prefix_owner.to_owned(),
                repo: prefix_repo.to_owned(),
                number,
            }),
            None => Classified::Malformed(Malformed::NotANumber {
                found: digits.to_owned(),
            }),
        };
    }
    match classify_url(suffix) {
        Classified::Names(named)
            if !named.owner.eq_ignore_ascii_case(prefix_owner)
                || !named.repo.eq_ignore_ascii_case(prefix_repo) =>
        {
            Classified::Malformed(Malformed::RepoMismatch {
                prefix_owner: prefix_owner.to_owned(),
                prefix_repo: prefix_repo.to_owned(),
                url_owner: named.owner,
                url_repo: named.repo,
            })
        }
        settled => settled,
    }
}

/// A GitHub pull request URL, in the spellings a browser hands over.
///
/// Accepted with or without a scheme, with or without `www.`, and with anything
/// at all after the number: GitHub's own tabs are `/files`, `/commits` and
/// `/checks`, and a review link carries a `#discussion_r…` fragment. Everything
/// past the number is dropped, because it names a *view* of the request rather
/// than a different request.
fn classify_url(text: &str) -> Classified {
    let without_scheme = match text.split_once("://") {
        Some((scheme, rest)) => {
            if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
                return Classified::NotOne;
            }
            rest
        }
        None => text,
    };
    let host_path =
        strip_prefix_ignore_ascii_case(without_scheme, "www.").unwrap_or(without_scheme);
    let Some(path) = strip_prefix_ignore_ascii_case(host_path, "github.com/") else {
        return Classified::NotOne;
    };
    // A query or a fragment describes a view, not a request.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments = path.split('/');
    let (Some(owner), Some(repo), Some(kind)) = (segments.next(), segments.next(), segments.next())
    else {
        return Classified::NotOne;
    };
    if !spec::is_name_part(owner) || !spec::is_name_part(repo) {
        return Classified::NotOne;
    }
    // `pulls` is the list page rather than one request, but it is what a person
    // types from memory and it can mean nothing else in this position.
    if !kind.eq_ignore_ascii_case("pull") && !kind.eq_ignore_ascii_case("pulls") {
        return Classified::NotOne;
    }
    let found = segments.next().unwrap_or_default();
    match pr_number(found) {
        Some(number) => Classified::Names(PullRequestRef {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        }),
        // `github.com/o/r/pull` with nothing after it is still a pull request
        // reference, an incomplete one. Answering `NotOne` here would hand devpod
        // a URL it cannot clone and report the wrong problem.
        None => Classified::Malformed(Malformed::NotANumber {
            found: found.to_owned(),
        }),
    }
}

/// `owner/repo`, against the same character class [`spec`] reads.
///
/// One reading of what an owner and a repo may be, shared with that module, so a
/// name accepted here cannot be one it then refuses.
fn owner_repo(text: &str) -> Option<(&str, &str)> {
    let (owner, repo) = text.split_once('/')?;
    (spec::is_name_part(owner) && spec::is_name_part(repo)).then_some((owner, repo))
}

/// A pull request number: decimal digits, and at least one.
///
/// Zero is refused because GitHub numbers from one, so `pull/0` is a typo rather
/// than a request, and asking for it spends a round trip to be told so.
fn pr_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match text.parse::<u32>() {
        Ok(0) | Err(_) => None,
        Ok(number) => Some(number),
    }
}

fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(owner: &str, repo: &str, number: u32) -> Classified {
        Classified::Names(PullRequestRef {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        })
    }

    #[test]
    fn the_shape_asked_for_is_the_prefix_and_the_link() {
        assert_eq!(
            classify("blooop/devlaunch@https://github.com/blooop/devlaunch/pull/579"),
            names("blooop", "devlaunch", 579)
        );
    }

    #[test]
    fn a_link_on_its_own_says_everything_the_prefix_would_have() {
        for spec in [
            "https://github.com/blooop/devlaunch/pull/579",
            "http://github.com/blooop/devlaunch/pull/579",
            "https://www.github.com/blooop/devlaunch/pull/579",
            "github.com/blooop/devlaunch/pull/579",
            "www.github.com/blooop/devlaunch/pull/579",
            "HTTPS://GitHub.com/blooop/devlaunch/pull/579",
        ] {
            assert_eq!(classify(spec), names("blooop", "devlaunch", 579), "{spec}");
        }
    }

    #[test]
    fn everything_past_the_number_names_a_view_and_is_dropped() {
        for spec in [
            "https://github.com/blooop/devlaunch/pull/579/files",
            "https://github.com/blooop/devlaunch/pull/579/commits/deadbeef",
            "https://github.com/blooop/devlaunch/pull/579/",
            "https://github.com/blooop/devlaunch/pull/579#discussion_r12345",
            "https://github.com/blooop/devlaunch/pull/579?w=1",
            "https://github.com/blooop/devlaunch/pulls/579",
        ] {
            assert_eq!(classify(spec), names("blooop", "devlaunch", 579), "{spec}");
        }
    }

    #[test]
    fn the_hash_shorthand_takes_its_repository_from_the_prefix() {
        assert_eq!(
            classify("blooop/devlaunch@#579"),
            names("blooop", "devlaunch", 579)
        );
    }

    #[test]
    fn a_bare_number_stays_a_branch_name() {
        // `579` matches `spec`'s branch pattern, so somebody's branch is called
        // that and this spelling cannot be taken for a pull request.
        assert_eq!(classify("blooop/devlaunch@579"), Classified::NotOne);
    }

    #[test]
    fn an_ordinary_spec_is_not_a_pull_request() {
        for spec in [
            "blooop/devlaunch",
            "blooop/devlaunch@main",
            "blooop/devlaunch@feature/pull/579",
            "devlaunch-main-3j1t",
            "./devlaunch",
            "/home/me/devlaunch",
            "~/devlaunch",
            "~/github.com/o/r/pull/579",
            "git@github.com:blooop/devlaunch.git",
            "https://github.com/blooop/devlaunch",
            "https://github.com/blooop/devlaunch.git",
            "https://gitlab.com/blooop/devlaunch/pull/579",
            "https://example.com/blooop/devlaunch/pull/579",
            "ssh://github.com/blooop/devlaunch/pull/579",
            "github.com/blooop/devlaunch/issues/579",
            "github.com/blooop/devlaunch",
        ] {
            assert_eq!(classify(spec), Classified::NotOne, "{spec}");
        }
    }

    #[test]
    fn a_prefix_that_disagrees_with_the_link_is_refused_rather_than_guessed_at() {
        assert_eq!(
            classify("a/b@https://github.com/blooop/devlaunch/pull/579"),
            Classified::Malformed(Malformed::RepoMismatch {
                prefix_owner: "a".to_owned(),
                prefix_repo: "b".to_owned(),
                url_owner: "blooop".to_owned(),
                url_repo: "devlaunch".to_owned(),
            })
        );
    }

    #[test]
    fn a_prefix_agrees_with_the_link_whatever_case_it_was_typed_in() {
        assert_eq!(
            classify("BlooOP/DevLaunch@https://github.com/blooop/devlaunch/pull/579"),
            names("blooop", "devlaunch", 579)
        );
    }

    #[test]
    fn a_reference_with_no_usable_number_is_malformed_and_not_a_url_to_clone() {
        for (spec, found) in [
            ("https://github.com/o/r/pull/abc", "abc"),
            ("https://github.com/o/r/pull/", ""),
            ("https://github.com/o/r/pull", ""),
            ("https://github.com/o/r/pull/0", "0"),
            ("https://github.com/o/r/pull/-1", "-1"),
            ("o/r@#", ""),
            ("o/r@#zero", "zero"),
            ("o/r@#0", "0"),
        ] {
            assert_eq!(
                classify(spec),
                Classified::Malformed(Malformed::NotANumber {
                    found: found.to_owned()
                }),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_number_too_large_for_the_type_is_malformed_rather_than_wrapped() {
        assert_eq!(
            classify("https://github.com/o/r/pull/99999999999999999999"),
            Classified::Malformed(Malformed::NotANumber {
                found: "99999999999999999999".to_owned()
            })
        );
    }

    #[test]
    fn surrounding_whitespace_from_a_paste_is_dropped() {
        assert_eq!(
            classify("  https://github.com/blooop/devlaunch/pull/579\n"),
            names("blooop", "devlaunch", 579)
        );
    }
}
