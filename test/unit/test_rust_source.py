"""The reader that takes constants out of the Rust source (#267).

`test/fixtures/rust_source.py` is how the Python-side guards get at values the
binary owns -- the timing vocabulary, the switch names. Every doc guard that
crosses that boundary rests on it, and it works by *parsing Rust*, which is
exactly the kind of thing that fails quietly: a regex that stops matching returns
nothing, and a caller comparing against nothing passes.

So this file pins two properties. That each reader still finds something (a
rename in `timing.rs` fails here, naming the reader, rather than in whichever doc
test happened to call it). And that the values are the ones the Rust source
actually declares -- asserted against literals, because a reader checked only
against itself is not checked at all.

The literals below are therefore deliberate duplication, and the only place in the
tree allowed any: they are what makes this a test rather than a tautology. When
`timing.rs` renames a stage, this file is edited in the same change, by somebody
who has read what the rename means for the bench scripts and the trend.
"""

import pytest

from fixtures import rust_source

# What `rust/devlaunch-core/src/timing.rs` declares today.
STAGES = ("handoff", "host-prep", "devpod-up", "tools", "attach")
HANDOFF = "handoff"
TIMING_ENV_VAR = "DEVLAUNCH_TIMING"
TIMING_JSON_VALUE = "json"
TIMING_JSON_PREFIX = "dl-timing-json:"
TOTAL_EPOCH = "in-process, excluding interpreter startup"


class TestTheTimingVocabulary:
    def test_the_stages_are_the_ones_timing_rs_names(self):
        assert rust_source.timing_stages() == STAGES

    def test_the_handoff_is_the_stage_no_launch_runs(self):
        assert rust_source.handoff_stage() == HANDOFF
        assert HANDOFF in STAGES, "the excluded stage has to be one of the stages"

    def test_stage_all_declares_the_same_order_as_the_names(self):
        """`Stage::ALL` and `Stage::name()` are two lists in one file.

        The readers take the vocabulary from `name()`, because those are the
        strings that reach the timing document. `ALL` is what a Rust test iterates.
        Nothing in Rust makes them agree, so a variant added to one and not the
        other is a real possibility -- and it would mean the document says something
        the enum's own exhaustive walk never covers.
        """
        variants = rust_source.timing_stage_variants()
        stages = rust_source.timing_stages()
        assert len(variants) == len(stages), f"{variants} vs {stages}"
        # `HostPrep` -> `host-prep`: the same word, in the two casings the two
        # lists are written in.
        assert [_dashed(variant) for variant in variants] == list(stages)


def _dashed(variant: str) -> str:
    """`HostPrep` -> `host-prep`, the spelling `name()` gives the same variant."""
    out = []
    for index, char in enumerate(variant):
        if char.isupper() and index:
            out.append("-")
        out.append(char.lower())
    return "".join(out)


class TestTheSwitchNames:
    @pytest.mark.parametrize(
        "reader,expected",
        [
            (rust_source.timing_env_var, TIMING_ENV_VAR),
            (rust_source.timing_json_value, TIMING_JSON_VALUE),
            (rust_source.timing_json_prefix, TIMING_JSON_PREFIX),
            (rust_source.timing_total_epoch, TOTAL_EPOCH),
        ],
        ids=lambda value: getattr(value, "__name__", value),
    )
    def test_each_reader_returns_what_timing_rs_declares(self, reader, expected):
        assert reader() == expected


class TestItFailsLoudlyRatherThanEmptily:
    """A reader that stopped matching must say so, not return nothing.

    This is the whole risk of parsing source: `re.search` returning None is not an
    error, and a guard comparing against an empty tuple is green forever. Each
    reader asserts, so this points them at a file that has none of what they want
    and expects the assertion.
    """

    def test_a_source_without_the_declarations_is_an_assertion(self, tmp_path, monkeypatch):
        empty = tmp_path / "timing.rs"
        empty.write_text("// nothing this reader is looking for\n", encoding="utf-8")
        monkeypatch.setattr(rust_source, "TIMING_RS", empty)

        for reader in (
            rust_source.timing_stages,
            rust_source.timing_stage_variants,
            rust_source.handoff_stage,
            rust_source.timing_env_var,
            rust_source.timing_json_value,
            rust_source.timing_json_prefix,
            rust_source.timing_total_epoch,
        ):
            with pytest.raises(AssertionError):
                reader()

    def test_a_missing_source_names_the_file(self, tmp_path, monkeypatch):
        monkeypatch.setattr(rust_source, "TIMING_RS", tmp_path / "gone.rs")
        with pytest.raises(AssertionError, match="gone.rs"):
            rust_source.timing_stages()
