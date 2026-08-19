//! dl's records, opened only if the command turns out to need them.
//!
//! This is the binary's side of devlaunch#145. Core states the requirement in a
//! type — [`ColdMachinery`] is *a way to get* a clone manager and a metadata
//! store, never the things themselves — precisely so that a warm launch can be
//! shown never to have read `metadata.json`. Something has to hold the other end
//! of that, and it is here rather than in core because opening the records is also
//! where the load's notices are *said*, and the sentences are the binary's.
//!
//! One value per command, like [`crate::session::Records`] itself: the records are
//! opened at most once, the load's notices and the migration's refusal are
//! reported at the moment the open happens, and a command that never asks has
//! provably not touched the file.

use devlaunch_core::flows::launch::{Cold, ColdMachinery, ColdRefused};
use devlaunch_core::runner::Runner;

use crate::commands;
use crate::render;
use crate::session::{self, Records, StartupError};

/// The records, opened on the first ask and kept for the rest of the command.
pub(crate) struct ColdPath<'r> {
    runner: &'r dyn Runner,
    records: Option<Records<'r>>,
}

impl<'r> ColdPath<'r> {
    pub(crate) fn new(runner: &'r dyn Runner) -> Self {
        Self {
            runner,
            records: None,
        }
    }

    /// dl's records, opening them the first time and reporting what that had to
    /// say.
    ///
    /// The report happens here, once, rather than at each call site: a caller that
    /// forgot it would silently drop the sentences a damaged `metadata.json`
    /// prints, and two callers that both remembered would print them twice.
    pub(crate) fn records(&mut self) -> Result<&mut Records<'r>, StartupError> {
        if self.records.is_none() {
            let records = session::open_records(self.runner)?;
            commands::report(&records);
            self.records = Some(records);
        }
        Ok(self.records.as_mut().expect("the records were just opened"))
    }
}

impl<'r> ColdMachinery<'r> for ColdPath<'r> {
    fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
        match self.records() {
            Ok(records) => Ok(Cold {
                clones: &records.clones,
                storage: &mut records.storage,
            }),
            // Rendered into the refusal rather than printed here, because core
            // carries this reason into the one sentence the launch refuses with
            // (`Repository 'owner/repo': …`) and printing it as well would say it
            // twice.
            Err(refused) => Err(ColdRefused {
                reason: render::startup_reason(&refused),
            }),
        }
    }
}
