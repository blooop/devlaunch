//! A cold-capable [`Launch`] built from `devlaunch_core::api` and nothing else.
//!
//! This is devlaunch#340 as a test. `api` is the tier `wf` is entitled to link
//! against (#251 §7), and until this compiled it did not carry a launcher: five of
//! `Launch::new`'s seven parameter types lived outside `api`, and the two that
//! decide whether a launch can go cold at all -- the [`ColdMachinery`] and the
//! [`Provision`] implementations that really do the work -- lived in the `dl`
//! binary, where nothing but `dl` could reach them. A second consumer could name
//! the launcher and could not build one.
//!
//! So the assertion is the import list. Every parameter is named through `api`;
//! the only path from anywhere else is the runner, which is its own crate and its
//! own promised seam (`devlaunch-runner`, snapshot of its own since #338) and is
//! not a parameter of `Launch::new`.
//!
//! What it checks at runtime is the other half of devlaunch#145: **building** a
//! cold-capable launcher reads nothing. The records are not opened, the migration
//! does not run, and the sinks are still empty when the launcher exists -- which is
//! what makes [`ColdPath`] a *way to get* the records rather than the records.

use std::path::Path;

use devlaunch_core::api::{
    ColdMachinery, ColdPath, CommandContext, Host, Launch, LaunchNotice, Notices, Provision,
    ProvisionEvent, RecordsNotice, Refresh, SelfInvocation, ToolProvisioning,
};
use devlaunch_test_support::FakeRunner;

#[test]
fn a_cold_capable_launch_is_built_from_the_api_module_alone() {
    let runner = FakeRunner::new();
    // Never written to and never read: a launcher that touched it while being
    // built would be the failure this test is here for.
    let cache = Path::new("/nonexistent/devlaunch-340");

    let mut context = CommandContext::new(&runner);
    let updater = SelfInvocation::new("dl".to_owned());
    let completions = cache.join("completions.json");
    let mut refresh = Refresh::new(&updater, &completions);

    // The three vocabularies a launch reports in, each collected rather than said:
    // `Vec<T>` is core's own sink for `T`, so a consumer needs nothing of its own
    // to hold a launch's events.
    let mut records_said: Vec<RecordsNotice> = Vec::new();
    let mut provision_said: Vec<ProvisionEvent> = Vec::new();
    let mut launch_said: Vec<LaunchNotice> = Vec::new();

    let mut cold = ColdPath::new(&runner, &mut records_said);
    let provision = ToolProvisioning::from_env(cache, &mut provision_said);
    let host = Host::from_process(cache);
    let mut forward = |_line: &str| {};

    // The two implementations are the real ones, named through the traits the
    // constructor asks for -- not test doubles standing in for them.
    let cold_machinery: &mut dyn ColdMachinery<'_> = &mut cold;
    let provisioner: &dyn Provision = &provision;
    let said: &mut dyn Notices<LaunchNotice> = &mut launch_said;

    let launch = Launch::new(
        &mut context,
        &mut refresh,
        cold_machinery,
        provisioner,
        &host,
        &mut forward,
        said,
    );
    drop(launch);

    // Nothing was spawned, nothing was said, and -- the point of devlaunch#145 --
    // no records were opened to say it with.
    assert_eq!(runner.call_count(), 0);
    assert!(records_said.is_empty(), "{records_said:?}");
    assert!(launch_said.is_empty(), "{launch_said:?}");
}
