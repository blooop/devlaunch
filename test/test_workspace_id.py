"""Tests for the workspace-id parse boundary.

These pin the defects verified in blooop/devlaunch#55 and the scheme decided
there. The derivation is the single source of both the devpod workspace id and
the clone-directory leaf, so a change here renames real directories.
"""

import pytest

from devlaunch.workspace_id import (
    REPO_SLUG_LENGTH,
    SUFFIX_LENGTH,
    TARGET_LENGTH,
    WorkspaceId,
    slug,
    source_workspace_id,
    validate_ref_name,
)


class TestSyllableSuffix:
    """The 8-char syllable suffix is the identity-bearing part of every id.

    DO NOT CHANGE THESE EXPECTED VALUES. They are not arbitrary fixtures: the
    planned Rust port has to reproduce the same ids byte for byte, and every
    workspace directory and devpod workspace already on disk is named by this
    output. Changing the consonant table, the vowel table, the syllable count,
    the digest, the byte slice, or the NUL delimiter renames every workspace in
    existence.

    These are 4 syllables / 24 bits. The 3-syllable / 18-bit values published in
    #55's resolution are superseded: at 18 bits a corpus of 500 branches in one
    repo already hit a birthday collision, so the width was widened by one
    syllable. Do not try to reproduce the old `zovomo`-era ids.
    """

    @pytest.mark.parametrize(
        "owner,repo,ref,expected",
        [
            ("blooop", "devlaunch", "main", "zovomobo"),
            ("blooop", "wayfinder", "main", "hesirora"),
            ("blooop", "devlaunch", "feature/auth", "poliseno"),
            ("blooop", "devlaunch", "feature-auth", "nesatabe"),
        ],
    )
    def test_suffix_is_frozen(self, owner, repo, ref, expected):
        assert WorkspaceId(owner, repo, ref).suffix == expected

    def test_suffix_is_four_syllables(self):
        """24 bits, not 18: widened after a real birthday collision at 3 syllables."""
        assert SUFFIX_LENGTH == 8
        assert len(WorkspaceId("blooop", "devlaunch", "main").suffix) == 8

    def test_suffix_is_alternating_consonant_vowel(self):
        suffix = WorkspaceId("blooop", "devlaunch", "main").suffix
        assert len(suffix) == SUFFIX_LENGTH
        for i, char in enumerate(suffix):
            table = "bdfghjklmnprstvz" if i % 2 == 0 else "aeio"
            assert char in table

    def test_suffix_hashes_the_unsanitized_ref(self):
        """Slug-equal refs must hash differently, or the derivation is not injective."""
        assert (
            WorkspaceId("blooop", "devlaunch", "feature/auth").suffix
            != WorkspaceId("blooop", "devlaunch", "feature-auth").suffix
        )

    def test_suffix_separates_the_triple_fields(self):
        """The NUL delimiter keeps field boundaries; without it these would collide."""
        assert WorkspaceId("a", "bc", "main").suffix != WorkspaceId("ab", "c", "main").suffix


class TestOwnerAndRepoAreCaseInsensitive:
    """One GitHub repo is one workspace, however the user spelled it.

    GitHub owner and repo names are case-insensitive, so `NVIDIA/cuda-samples` and
    `nvidia/cuda-samples` are the same repository. Hashing them raw made each
    spelling its own id, so the same repo cloned twice into two containers — a
    regression against the old derivation, which lowercased before deriving.

    Refs stay case-sensitive: git refs genuinely are, and `Main` and `main` can
    both exist in one repository.
    """

    CASE_VARIANTS = [
        ("blooop", "devlaunch"),
        ("Blooop", "devlaunch"),
        ("blooop", "DevLaunch"),
        ("BLOOOP", "DEVLAUNCH"),
        ("BlOoOp", "dEvLaUnCh"),
    ]

    def test_case_variants_of_one_repo_derive_one_id(self):
        ids = {WorkspaceId(owner, repo, "main").value for owner, repo in self.CASE_VARIANTS}
        assert len(ids) == 1

    def test_case_variants_share_the_suffix(self):
        suffixes = {WorkspaceId(owner, repo, "main").suffix for owner, repo in self.CASE_VARIANTS}
        assert len(suffixes) == 1

    def test_lowercase_input_is_unaffected(self):
        """Case folding must not move ids for input that was already lowercase."""
        assert WorkspaceId("blooop", "devlaunch", "main").suffix == "zovomobo"

    def test_refs_stay_case_sensitive(self):
        """Two real refs differing only in case are two workspaces."""
        assert (
            WorkspaceId("blooop", "devlaunch", "main").value
            != WorkspaceId("blooop", "devlaunch", "Main").value
        )

    def test_nvidia_case_regression(self):
        """The concrete case from review: two spellings, one container."""
        assert (
            WorkspaceId("NVIDIA", "cuda-samples", "main").value
            == WorkspaceId("nvidia", "cuda-samples", "main").value
        )


class TestSlug:
    """One slug rule, used for both the repo part and the ref part."""

    @pytest.mark.parametrize(
        "text,expected",
        [
            ("main", "main"),
            ("Feature/MyBranch", "feature-mybranch"),
            ("my_repo", "my-repo"),
            ("python_template", "python-template"),
            ("v1.2.3", "v1-2-3"),
            ("--evil--", "evil"),
            ("a...b", "a-b"),
            ("", ""),
        ],
    )
    def test_slug(self, text, expected):
        assert slug(text) == expected

    def test_underscore_has_exactly_one_meaning(self):
        """Defect 4: `_` became `-` in one derivation and was deleted in the other."""
        assert slug("my_repo") == "my-repo"
        assert WorkspaceId("owner", "my_repo", "main").value.startswith("my-repo-")


class TestValidation:
    """Bad input gets exactly one response: ValueError, at the constructor."""

    @pytest.mark.parametrize("ref", ["main", "feature/my-branch", "v1.2.3", "release_1"])
    def test_accepts_ordinary_refs(self, ref):
        assert WorkspaceId("owner", "repo", ref).ref == ref

    @pytest.mark.parametrize("ref", ["", "--evil", "-x", "branch name", "a;b", "..", "a%b"])
    def test_rejects_unsafe_refs(self, ref):
        with pytest.raises(ValueError, match="Invalid git ref"):
            WorkspaceId("owner", "repo", ref)

    @pytest.mark.parametrize("owner", ["", "--evil", "own er"])
    def test_rejects_unsafe_owners(self, owner):
        with pytest.raises(ValueError, match="Invalid git owner"):
            WorkspaceId(owner, "repo", "main")

    @pytest.mark.parametrize("repo", ["", "--evil", "re po"])
    def test_rejects_unsafe_repos(self, repo):
        with pytest.raises(ValueError, match="Invalid git repo"):
            WorkspaceId("owner", repo, "main")

    def test_validate_ref_name_is_the_same_predicate(self):
        validate_ref_name("main")
        with pytest.raises(ValueError, match="Invalid git ref"):
            validate_ref_name("--evil")


class TestInjectivity:
    """Defects 1 and 3: distinct inputs must give distinct ids, always."""

    def test_slash_and_dash_refs_differ(self):
        a = WorkspaceId("blooop", "devlaunch", "feature/auth").value
        b = WorkspaceId("blooop", "devlaunch", "feature-auth").value
        assert a != b

    def test_five_preimages_of_feature_auth_now_differ(self):
        """#55 verified five refs collapsing onto `feature-auth`."""
        refs = ["feature/auth", "feature-auth", "feature.auth", "feature_auth", "featureauth"]
        ids = {WorkspaceId("blooop", "devlaunch", ref).value for ref in refs}
        assert len(ids) == 5

    def test_thirty_char_shared_prefix_refs_differ(self):
        """Truncation used to make these byte-identical."""
        prefix = "feature/a-very-long-branch-nam"
        assert len(prefix) == 30
        a = WorkspaceId("blooop", "devlaunch", prefix + "e-one").value
        b = WorkspaceId("blooop", "devlaunch", prefix + "e-two").value
        assert a != b

    def test_same_ref_different_repo_differs(self):
        a = WorkspaceId("blooop", "devlaunch", "main").value
        b = WorkspaceId("blooop", "wayfinder", "main").value
        assert a != b

    def test_same_ref_different_owner_differs(self):
        """The old derivation dropped the owner entirely."""
        a = WorkspaceId("blooop", "devlaunch", "main").value
        b = WorkspaceId("someone-else", "devlaunch", "main").value
        assert a != b

    def test_truncation_contributes_no_collisions(self):
        """The property that makes the scheme correct: only the suffix can collide.

        500 refs that all slug down to the same truncated prefix, so the readable
        part of every id is byte-identical. The number of distinct ids must equal
        the number of distinct suffixes — truncation adds nothing on top of the
        suffix's own birthday rate. (18 bits is a decided budget, so this asserts
        equality rather than 500: it is the *independence* that matters here.)
        """
        parsed = [
            WorkspaceId("blooop", "a-repo-name-that-is-long", f"feature/shared-prefix-{i}")
            for i in range(500)
        ]
        readable = {ws.value.rsplit("-", 1)[0] for ws in parsed}
        assert len(readable) == 1, "the corpus must actually collide before truncation"
        assert len({ws.value for ws in parsed}) == len({ws.suffix for ws in parsed})


class TestLength:
    """Defect 2: the 48-char guard had a hole that skipped truncation entirely."""

    def test_forty_seven_char_repo_fits(self):
        repo = "r" * 47
        assert len(repo) == 47
        value = WorkspaceId("owner", repo, "main").value
        assert len(value) <= TARGET_LENGTH

    @pytest.mark.parametrize("repo_len", range(1, 60))
    def test_every_repo_length_fits(self, repo_len):
        value = WorkspaceId("owner", "r" * repo_len, "some/long/branch/name-here").value
        assert len(value) <= TARGET_LENGTH

    @pytest.mark.parametrize("ref_len", range(1, 80))
    def test_every_ref_length_fits(self, ref_len):
        value = WorkspaceId("owner", "devlaunch", "b" * ref_len).value
        assert len(value) <= TARGET_LENGTH

    def test_repo_slug_intact_up_to_twenty_chars(self):
        repo = "r" * REPO_SLUG_LENGTH
        value = WorkspaceId("owner", repo, "a/very/long/branch/name/that/eats/the/budget").value
        assert value.startswith(repo + "-")

    def test_repo_slug_never_cut_below_twenty(self):
        value = WorkspaceId("owner", "r" * 47, "a/very/long/branch/name/that/eats/the/budget").value
        assert value.startswith("r" * REPO_SLUG_LENGTH + "-")

    def test_suffix_is_never_truncated(self):
        for repo_len in (1, 20, 47, 80):
            value = WorkspaceId("owner", "r" * repo_len, "b" * 80).value
            assert value.endswith("-" + WorkspaceId("owner", "r" * repo_len, "b" * 80).suffix)

    def test_constructed_id_shape_is_lowercase_alnum_and_dashes(self):
        """Holds for ids this constructor produces.

        It is not an invariant of every string devlaunch hands devpod as `--id`:
        a path spec (`dl ./My_Project`) still passes a resolved directory name
        straight through, which this boundary never sees. That gap is tracked
        separately; the claim here is scoped to constructed ids on purpose.
        """
        value = WorkspaceId("Owner", "My_Repo.git", "Feature/MyBranch").value
        assert all(c.isalnum() and c.islower() or c.isdigit() or c == "-" for c in value)
        assert not value.startswith("-")
        assert not value.endswith("-")


class TestSegmentAwareTruncation:
    """Path-shaped refs drop middle segments before losing characters."""

    def test_dependabot_ref_keeps_the_action_name(self):
        value = WorkspaceId(
            "blooop", "devlaunch", "dependabot/github_actions/codecov/codecov-action-6"
        ).value
        assert "codecov" in value
        assert value.startswith("devlaunch-dependabot-codecov")
        assert "github-actions" not in value

    def test_only_as_many_middle_segments_drop_as_needed(self):
        """Segments go leftmost-middle first, and only until the ref fits."""
        value = WorkspaceId("blooop", "dl", "a/bbbbbbbb/cccccccc/dddddddd/zzz").value
        assert value.startswith("dl-a-cccccccc-dddddddd-zzz-")
        assert "bbbbbbbb" not in value

    def test_first_and_last_segments_survive_heavy_truncation(self):
        """However many middles have to go, the ends stay."""
        ref = "aa/" + "m" * 30 + "/" + "n" * 30 + "/zz"
        value = WorkspaceId("blooop", "dl", ref).value
        assert value.startswith("dl-aa-zz-")
        assert "mmmm" not in value and "nnnn" not in value

    def test_single_segment_ref_is_truncated_by_characters(self):
        value = WorkspaceId("blooop", "devlaunch", "a" * 60).value
        assert value.startswith("devlaunch-aaa")

    def test_no_double_dashes_after_dropping_segments(self):
        value = WorkspaceId("blooop", "devlaunch", "a//b///c/d").value
        assert "--" not in value


class TestSourceWorkspaceId:
    """Ids for git sources that name no ref — plain URL specs.

    `dl github.com/owner/repo` cannot become a WorkspaceId: there is no ref to
    hash. It still reaches devpod as `--id`, so it gets the same treatment —
    slugged, suffixed and capped — rather than a bare slug. A bare slug was both
    uncapped (a long URL gave 92 characters against devpod's 48-char ceiling) and
    non-injective, collapsing `my_repo`, `my-repo` and `my.repo` onto one id.
    """

    def test_underscore_dash_and_dot_sources_stay_distinct(self):
        """The collision the first cut of this branch introduced."""
        sources = [
            "gitlab.com/group/my_repo",
            "gitlab.com/group/my-repo",
            "gitlab.com/group/my.repo",
        ]
        assert len({source_workspace_id(s) for s in sources}) == 3

    @pytest.mark.parametrize("length", [1, 10, 40, 80, 200])
    def test_every_source_length_is_capped(self, length):
        source = f"github.com/{'o' * length}/{'r' * length}"
        assert len(source_workspace_id(source)) <= TARGET_LENGTH

    def test_long_source_used_to_overflow_devpod(self):
        """92 characters before; devpod's own ceiling is 48."""
        source = f"github.com/{'o' * 40}/{'r' * 40}"
        assert len(source_workspace_id(source)) <= TARGET_LENGTH

    def test_readable_prefix_survives(self):
        assert source_workspace_id("github.com/loft-sh/devpod").startswith(
            "github-com-loft-sh-devpod-"
        )

    def test_suffix_is_present_and_full_width(self):
        value = source_workspace_id("github.com/owner/repo")
        assert len(value.rsplit("-", 1)[1]) == SUFFIX_LENGTH

    def test_source_case_is_folded(self):
        """Same repo, different spelling of the URL: one workspace."""
        assert source_workspace_id("github.com/Blooop/DevLaunch") == source_workspace_id(
            "github.com/blooop/devlaunch"
        )

    def test_source_ids_cannot_collide_with_triple_ids(self):
        """A ref-less source and a real (owner, repo, ref) workspace are different things.

        Both are tagged before hashing, so even a source whose slug matches a
        triple's slug gets a different suffix.
        """
        # A triple contrived so its slug matches the URL's exactly.
        triple = WorkspaceId("anyone", "github.com", "owner/repo")
        source = source_workspace_id("github.com/owner/repo")
        # Same readable slug ...
        assert source.rsplit("-", 1)[0] == triple.value.rsplit("-", 1)[0] == "github-com-owner-repo"
        # ... different identity, because the source's fields are tagged.
        assert source != triple.value

    def test_is_deterministic(self):
        assert source_workspace_id("github.com/owner/repo") == source_workspace_id(
            "github.com/owner/repo"
        )


class TestDeterminism:
    """The derivation is pure: same triple in, same id out, always."""

    def test_stable_across_calls(self):
        first = WorkspaceId("blooop", "devlaunch", "main").value
        second = WorkspaceId("blooop", "devlaunch", "main").value
        assert first == second

    def test_independent_of_cwd(self, tmp_path, monkeypatch):
        before = WorkspaceId("blooop", "devlaunch", "main").value
        monkeypatch.chdir(tmp_path)
        assert WorkspaceId("blooop", "devlaunch", "main").value == before

    def test_independent_of_environment(self, monkeypatch):
        before = WorkspaceId("blooop", "devlaunch", "main").value
        monkeypatch.setenv("PYTHONHASHSEED", "12345")
        monkeypatch.setenv("HOME", "/nowhere")
        monkeypatch.setenv("DEVLAUNCH_CACHE_DIR", "/nowhere")
        assert WorkspaceId("blooop", "devlaunch", "main").value == before

    def test_stable_in_a_fresh_interpreter(self):
        """Guards against PYTHONHASHSEED-style per-process randomisation."""
        import subprocess
        import sys

        code = (
            "from devlaunch.workspace_id import WorkspaceId;"
            "print(WorkspaceId('blooop', 'devlaunch', 'main').value)"
        )
        out = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, check=True
        )
        assert out.stdout.strip() == WorkspaceId("blooop", "devlaunch", "main").value


class TestRealWorldCorpus:
    """The names this repo actually sees, so the format stays readable."""

    CORPUS = [
        ("blooop", "devlaunch", "main"),
        ("blooop", "wayfinder", "main"),
        ("blooop", "devlaunch", "feature/auth"),
        ("blooop", "devlaunch", "feature-auth"),
        ("blooop", "devlaunch", "dependabot/github_actions/codecov/codecov-action-6"),
        ("blooop", "devlaunch", "dependabot/github_actions/blooop/prek-action-2"),
        ("blooop", "python_template", "dependabot/pip/lib/dependencies-1"),
        ("blooop", "lifetime_foc_rig", "dependabot/github_actions/codecov/codecov-action-6"),
        ("kinisi-robotics", "kinisi_ros", "ags-devcontainer-tooling-support"),
        ("blooop", "devlaunch", "fix/gh-auth-in-devcontainer"),
        ("blooop", "test_renv", "nb4"),
        ("owner", "r" * 47, "main"),
    ]

    def test_corpus_is_unique_and_within_budget(self):
        ids = [WorkspaceId(*triple).value for triple in self.CORPUS]
        assert len(set(ids)) == len(ids)
        assert max(len(i) for i in ids) <= TARGET_LENGTH

    def test_repo_name_is_intact_for_short_repos(self):
        for owner, repo, ref in self.CORPUS:
            if len(slug(repo)) <= REPO_SLUG_LENGTH:
                assert WorkspaceId(owner, repo, ref).value.startswith(slug(repo) + "-")


class TestWorkspaceIdType:
    """The parsed value itself: immutable, self-describing, hashable."""

    def test_str_is_the_id(self):
        ws = WorkspaceId("blooop", "devlaunch", "main")
        assert str(ws) == ws.value

    def test_is_frozen(self):
        ws = WorkspaceId("blooop", "devlaunch", "main")
        with pytest.raises(Exception):
            ws.ref = "other"  # type: ignore[misc]

    def test_equality_is_by_triple(self):
        assert WorkspaceId("blooop", "devlaunch", "main") == WorkspaceId(
            "blooop", "devlaunch", "main"
        )
        assert WorkspaceId("blooop", "devlaunch", "main") != WorkspaceId(
            "blooop", "devlaunch", "other"
        )

    def test_is_hashable(self):
        assert len({WorkspaceId("blooop", "devlaunch", "main")}) == 1
