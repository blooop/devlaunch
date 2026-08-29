//! The gate, the walk and the reader, at the seam each one is claimed at.
//!
//! **Every row here is a real directory tree.** The whole decision rests on
//! reading files that programs wrote, so a fixture that stubbed the read would
//! be testing the stub — and the one claim most worth breaking, that the
//! predicate never reads a directory's name, can only be shown by building
//! directories whose names would fool a name-matcher and watching them land on
//! the side their *contents* put them on.

use super::*;

/// The signature as a real writer emits it: 43 bytes, then the specification's
/// own explanatory comment. Nothing here trims or reflows it — the point is
/// that the first 43 bytes are compared as bytes.
const REAL_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
                        # This file is a cache directory tag created by a build tool.\n\
                        # For information about cache directory tags see https://bford.info/cachedir/\n";

fn dir(at: &Path) -> PathBuf {
    std::fs::create_dir_all(at).expect("a directory");
    at.to_path_buf()
}

/// A directory carrying the published tag, whatever it is called.
fn tagged(at: &Path) -> PathBuf {
    dir(at);
    std::fs::write(at.join("CACHEDIR.TAG"), REAL_TAG).expect("a cache tag");
    at.to_path_buf()
}

/// What pixi writes into an installed environment. `manifest_path` is the
/// container path a real one carries on every host, present here precisely
/// because nothing may read it.
fn pixi_record(env: &Path, environment: &str) {
    let meta = dir(&env.join("conda-meta"));
    std::fs::write(
        meta.join("pixi"),
        serde_json::json!({
            "manifest_path": "/workspaces/devlaunch-container/pyproject.toml",
            "environment_name": environment,
            "pixi_version": "0.77.0",
            "environment_lock_file_hash": "cb70a71a2c1df89c",
        })
        .to_string(),
    )
    .expect("pixi's own record");
}

/// A lockfile naming `environments`, in the shape pixi writes.
fn lock(at: &Path, environments: &[&str]) {
    let mut text = String::from("version: 7\nplatforms:\n- name: linux-64\nenvironments:\n");
    for environment in environments {
        text.push_str(&format!(
            "  {environment}:\n    channels:\n    - url: https://conda.anaconda.org/conda-forge/\n    packages:\n      linux-64:\n      - conda: https://example.invalid/a.conda\n"
        ));
    }
    text.push_str("packages:\n- conda: https://example.invalid/a.conda\n");
    std::fs::write(at.join("pixi.lock"), text).expect("a lockfile");
}

/// A clone with one site in it, which is the only shape `Inside` can spell.
struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a scratch directory"),
        }
    }

    fn clone_root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn site(&self, leaf: &str) -> PathBuf {
        dir(&self
            .clone_root()
            .join(".claude")
            .join("worktrees")
            .join(leaf))
    }

    /// The walk, with nothing claiming anything and no other site in the forest.
    fn walk(&self, site: &Path) -> Vec<Tagged> {
        tagged_in(&self.clone_root(), site, &[], &[site.to_path_buf()])
    }
}

fn places(found: &[Tagged]) -> Vec<String> {
    found.iter().map(|it| it.at().as_str().to_owned()).collect()
}

// ---------------------------------------------------------------------------
// the gate
// ---------------------------------------------------------------------------

#[test]
fn the_gate_is_the_published_signature_and_nothing_looser() {
    let world = World::new();
    let root = world.clone_root();

    assert!(
        declared_regenerable(&tagged(&root.join("real"))),
        "the published 43 bytes at offset 0 are the whole gate"
    );
    assert!(
        !declared_regenerable(&dir(&root.join("untagged"))),
        "no CACHEDIR.TAG at all is not a declaration"
    );

    // Every one of these is a file that mentions the signature and is not one.
    for (name, content) in [
        ("wrong-hex", "Signature: 8a477f597d28d172789f06886806bc54\n"),
        (
            "leading-space",
            " Signature: 8a477f597d28d172789f06886806bc55\n",
        ),
        (
            "not-at-offset-zero",
            "# a comment first\nSignature: 8a477f597d28d172789f06886806bc55\n",
        ),
        ("truncated", "Signature: 8a477f597d28d172789f0688680"),
        ("empty", ""),
    ] {
        let at = dir(&root.join(name));
        std::fs::write(at.join("CACHEDIR.TAG"), content).expect("a near miss");
        assert!(
            !declared_regenerable(&at),
            "{name} is not the specification's signature and must not read as one"
        );
    }
}

#[test]
fn the_gate_never_reads_a_directorys_name() {
    // The measured population this row stands for (devlaunch#468 §2): rattler,
    // cargo, uv and pytest write a tag; `python -m venv` and npm write none,
    // and npm writes none anywhere beneath `node_modules` either. **The same
    // name lands on both sides** — a `.venv` is admitted or refused depending
    // on which program made it — so a predicate keyed on the name cannot
    // express the rule at all.
    //
    // The claim under test is about the predicate, so the test ranges over the
    // predicate: every one of these names is put to it, and the answer tracks
    // the file and never the name.
    let world = World::new();
    let root = world.clone_root();
    for name in [
        ".venv",
        "node_modules",
        ".pixi",
        "envs",
        "default",
        "target",
        "src",
        "a directory nobody would ever call a cache",
    ] {
        let untagged = dir(&root.join("plain").join(name));
        let with_tag = tagged(&root.join("declared").join(name));
        assert!(
            !declared_regenerable(&untagged),
            "{name} carries no tag and must be refused whatever it is called"
        );
        assert!(
            declared_regenerable(&with_tag),
            "{name} carries the tag and must be admitted whatever it is called"
        );
    }
}

#[test]
fn a_stdlib_venv_and_a_node_modules_are_not_found_by_the_walk() {
    // The fixture row devlaunch#472 asks for, at the level the walk decides:
    // both are shaped exactly like the population a name-matcher would take,
    // and neither is a candidate, because neither program ever made the claim.
    let world = World::new();
    let site = world.site("agent-one");
    let venv = dir(&site.join(".venv").join("lib").join("python3.12"));
    std::fs::write(venv.join("os.py"), "stdlib\n").expect("a stdlib file");
    std::fs::write(
        site.join(".venv").join("pyvenv.cfg"),
        "home = /usr/bin\ninclude-system-site-packages = false\n",
    )
    .expect("a pyvenv.cfg");
    let package = dir(&site.join("node_modules").join("lodash"));
    std::fs::write(package.join("index.js"), "module.exports = {}\n").expect("a package");

    assert!(
        world.walk(&site).is_empty(),
        "nothing in a stdlib venv or a node_modules declares itself regenerable"
    );
}

// ---------------------------------------------------------------------------
// the walk
// ---------------------------------------------------------------------------

#[test]
fn the_walk_does_not_descend_past_a_tag() {
    let world = World::new();
    let site = world.site("agent-one");
    let outer = tagged(&site.join(".pixi").join("envs").join("default"));
    // A second tag inside the first: cargo's own `target` is exactly this shape
    // when somebody builds inside an environment. The outer declaration covers
    // it, and reporting both would bill the same bytes twice under R3.
    tagged(&outer.join("share").join("build"));

    assert_eq!(
        places(&world.walk(&site)),
        vec![".claude/worktrees/agent-one/.pixi/envs/default"],
        "the outermost tag is the unit"
    );
}

#[test]
fn the_walk_never_puts_the_question_to_the_site_itself() {
    // A site's own verdict is the sweep's answer about that directory. A tag on
    // it would be a second answer to a question already asked, and the second
    // answer would be the one that deletes a registered worktree.
    let world = World::new();
    let site = world.site("agent-one");
    tagged(&site);

    assert!(
        world.walk(&site).is_empty(),
        "the walk starts below the site and never tests the site"
    );
}

#[test]
fn the_walk_stops_at_a_site_the_forest_already_holds() {
    let world = World::new();
    let site = world.site("agent-one");
    let nested = dir(&site.join(".claude").join("worktrees").join("agent-two"));
    tagged(&nested.join(".pixi").join("envs").join("default"));

    let found = tagged_in(
        &world.clone_root(),
        &site,
        &[],
        &[site.clone(), nested.clone()],
    );

    assert!(
        found.is_empty(),
        "a nested site's derivatives are that site's own pass to find, so they are \
         attributed to its path and counted once: {found:?}"
    );
}

#[test]
fn a_symlink_is_never_followed_out_of_the_tree() {
    let world = World::new();
    let site = world.site("agent-one");
    let outside = tagged(&world.clone_root().join("elsewhere"));
    std::os::unix::fs::symlink(&outside, site.join("link")).expect("a symlink");

    assert!(
        world.walk(&site).is_empty(),
        "following a link walks a removal out of the tree --prune is scoped to"
    );
}

// ---------------------------------------------------------------------------
// the reader
// ---------------------------------------------------------------------------

#[test]
fn a_lock_that_names_the_environment_re_derives_it() {
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    lock(&site, &["default"]);

    let found = world.walk(&site);

    let [Tagged::Derivable(derivative)] = &found[..] else {
        panic!("a tag with its recipe on disk is derivable: {found:?}");
    };
    assert_eq!(
        derivative.recipe(),
        &Recipe::PixiEnvironment {
            environment: "default".to_owned(),
            lock: crate::flows::agent_worktrees::inside_the_clone(
                &world.clone_root(),
                &site.join("pixi.lock")
            )
            .expect("the lockfile's place"),
        }
    );
}

#[test]
fn the_reader_stores_no_recorded_path_at_all() {
    // `conda-meta/pixi` records the manifest as an absolute path written by
    // whoever ran the install, so on every environment installed in a container
    // it is a `/workspaces/<id>/…` that does not resolve on the host — the same
    // trap devlaunch#445 and devlaunch#446 answer by never resolving a recorded
    // path. The constructive form is that the field does not exist to be
    // resolved, and this is the test that says the value cannot carry it.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    lock(&site, &["default"]);

    let found = world.walk(&site);

    let rendered = format!("{found:?}");
    assert!(
        !rendered.contains("/workspaces/devlaunch-container"),
        "the recorded manifest path reached the value: {rendered}"
    );
    assert!(
        !rendered.contains("manifest"),
        "nothing about a manifest is carried: {rendered}"
    );
}

#[test]
fn a_lock_absent_stands_the_environment() {
    // Measured: with the lock gone, `pixi install --frozen --offline` restores
    // 0 files. Nothing on disk re-derives it, so it stands and is named.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    std::fs::write(env.join("big.so"), "a great many bytes\n").expect("env bytes");

    let found = world.walk(&site);

    let [Tagged::CouldNotCost { why, bytes, .. }] = &found[..] else {
        panic!("no lock means no recipe: {found:?}");
    };
    assert_eq!(why, &NoRecipe::LockfileAbsent);
    assert!(
        bytes.known_bytes() > 0,
        "principle 2 is served by visibility where it is not served by reclamation"
    );
}

#[test]
fn a_lock_that_no_longer_names_the_environment_stands_with_its_pointer() {
    // Measured as a real population: add an `extra` environment, install it,
    // drop it from the manifest and reinstall. The directory survives, the
    // lockfile stops listing it, and pixi never mentions it again.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("extra"));
    pixi_record(&env, "extra");
    lock(&site, &["default"]);

    let found = world.walk(&site);

    let [Tagged::CouldNotCost { why, .. }] = &found[..] else {
        panic!("an environment nothing names is not derivable: {found:?}");
    };
    assert_eq!(
        why,
        &NoRecipe::LockfileDoesNotNameIt {
            environment: "extra".to_owned()
        }
    );
    assert!(
        why.describe().contains("pixi clean -e extra"),
        "principle 2 is served by a pointer where it is not served by reclamation: {}",
        why.describe()
    );
}

#[test]
fn a_stale_lock_still_re_derives_what_was_there() {
    // A stale *lock* is not a stale *environment*: the environment on disk was
    // itself produced from that lock, so the lock reproduces exactly what is
    // there. Measured: `--frozen --offline` against a lock whose manifest has
    // moved on restored all 5507 files. The manifest is therefore never read,
    // and this fixture puts one there that disagrees to prove it is not.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    lock(&site, &["default"]);
    std::fs::write(
        site.join("pixi.toml"),
        "[dependencies]\nsomething-the-lock-has-never-heard-of = \"*\"\n",
    )
    .expect("a manifest the lock does not match");

    let found = world.walk(&site);

    assert!(
        matches!(&found[..], [Tagged::Derivable(_)]),
        "a lock that names the environment re-derives it whatever the manifest says: \
         {found:?}"
    );
}

#[test]
fn a_tag_no_reader_recognises_stands_and_is_named() {
    // cargo's `target`, pytest's cache, a `uv venv` somebody `uv pip install`ed
    // into: the tag is a claim about purpose and devlaunch has nothing that
    // re-derives them. One general gate, an open set of readers, exactly one
    // implemented, and a tag with no reader stands.
    let world = World::new();
    let site = world.site("agent-one");
    let target = tagged(&site.join("rust").join("target"));
    std::fs::write(target.join("libthing.rlib"), "built bytes\n").expect("build output");
    lock(&site, &["default"]);

    let found = world.walk(&site);

    let [Tagged::CouldNotCost { why, .. }] = &found[..] else {
        panic!("no reader recognises a cargo target: {found:?}");
    };
    assert_eq!(why, &NoRecipe::NoReaderRecognisedIt);
}

#[test]
fn a_lockfile_above_the_site_is_not_this_sites_recipe() {
    // The walk up stops at the site. A lockfile in the clone root belongs to a
    // tree this pass is not deciding about, and reading it would be resolving
    // one directory's record against another directory's contents.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    lock(&world.clone_root(), &["default"]);

    let found = world.walk(&site);

    assert!(
        matches!(
            &found[..],
            [Tagged::CouldNotCost {
                why: NoRecipe::LockfileAbsent,
                ..
            }]
        ),
        "the walk up stops at the site: {found:?}"
    );
}

#[test]
fn the_environments_block_is_read_the_way_pixi_writes_it() {
    // The four-line scan, against the shape a real lockfile has: a top-level
    // `environments:` map, one key per environment, ending at the next line in
    // column zero. `packages:` below it is not an environment.
    let text = "version: 7\n\
                platforms:\n\
                - name: linux-64\n\
                environments:\n  \
                default:\n    \
                channels:\n    \
                - url: https://conda.anaconda.org/conda-forge/\n    \
                packages:\n      \
                linux-64:\n      \
                - conda: https://example.invalid/a.conda\n  \
                py312:\n    \
                channels: []\n\
                packages:\n\
                - conda: https://example.invalid/a.conda\n";

    assert_eq!(
        environments_in(text),
        vec!["default".to_owned(), "py312".to_owned()]
    );
}

#[test]
fn a_lockfile_with_no_environments_block_names_nothing() {
    assert!(environments_in("version: 7\npackages: []\n").is_empty());
    assert!(environments_in("").is_empty());
}

// ---------------------------------------------------------------------------
// the claimant fold
// ---------------------------------------------------------------------------

#[test]
fn a_claim_in_force_pins_the_derivative_and_an_account_of_content_does_not() {
    // The fold, at the level it is decided: ask of each reason whether it is a
    // claimant's or git's account of the site's content. The two rows below are
    // the whole of devlaunch#468 §6 in one assertion, and they go through
    // `Reason::subject` — the one expression of what a claimant is — rather
    // than through a second list of arms here.
    let world = World::new();
    let site = world.site("agent-one");
    let env = tagged(&site.join(".pixi").join("envs").join("default"));
    pixi_record(&env, "default");
    lock(&site, &["default"]);
    let place = Place::ASite(
        crate::flows::agent_worktrees::inside_the_clone(&world.clone_root(), &site)
            .expect("the site's place"),
    );

    let claimant = Reason::CouldNotProve {
        at: place.clone(),
        blank: Blank::ThirdPartyClaim(Some("a portable device".to_owned())),
    };
    let content = Reason::CouldNotProve {
        at: place,
        blank: Blank::NothingToAskThrough,
    };

    let claimed = tagged_in(
        &world.clone_root(),
        &site,
        std::slice::from_ref(&claimant),
        std::slice::from_ref(&site),
    );
    assert!(
        matches!(&claimed[..], [Tagged::Claimed { by, .. }] if by.as_ref() == &claimant),
        "a lock is a claim over the directory and admits no distinction between its \
         parts: {claimed:?}"
    );

    let derivable = tagged_in(
        &world.clone_root(),
        &site,
        std::slice::from_ref(&content),
        std::slice::from_ref(&site),
    );
    assert!(
        matches!(&derivable[..], [Tagged::Derivable(_)]),
        "git's account of the site's content was never about these bytes: {derivable:?}"
    );
}

#[test]
fn a_site_inside_a_tag_makes_it_a_claim_the_tag_does_not_speak_for() {
    let world = World::new();
    let site = world.site("agent-one");
    // A tag planted above a nested site's place. Absurd in the wild and
    // constructible in a minute, which is the only reason it needs a verdict.
    let over = tagged(&site.join(".claude"));
    let nested = dir(&over.join("worktrees").join("agent-two"));

    let found = tagged_in(
        &world.clone_root(),
        &site,
        &[],
        &[site.clone(), nested.clone()],
    );

    let [Tagged::Claimed { by, .. }] = &found[..] else {
        panic!("a site under a tag is never a candidate: {found:?}");
    };
    assert_eq!(by.subject(), Subject::AClaim);
    assert!(matches!(
        by.as_ref(),
        Reason::CouldNotProve {
            blank: Blank::ASiteSitsInside,
            ..
        }
    ));
}
