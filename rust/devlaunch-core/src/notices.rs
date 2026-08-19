//! Where a flow's notices go, as they happen.
//!
//! Every flow in this crate reports the way `dl.py`'s `logging.*` calls did:
//! something happened, and somebody upstairs may want to say so. What it must not
//! do is *decide* when the saying happens — and a `&mut Vec<T>` decided it, because
//! a vector can only be read once the call that filled it has returned. That is the
//! difference between `Cloning repository …` printed in front of a three-minute
//! clone and printed after it, and between Python's
//! `Workspace X is already running.` before the provisioning pass and this build's
//! after it.
//!
//! So the channel is a *sink*: one method, called at the moment the notice
//! happens. The two implementations are the two things a caller wants:
//!
//! - [`Vec<T>`] collects, which is what a test asserting on a sequence needs and
//!   what a command that prints its notices under a report still wants.
//! - anything the caller writes — the `dl` binary's printer — says it now.
//!
//! [`Wrapped`] is the third piece, and it is what keeps one vocabulary per layer:
//! a storage flow reports [`CacheNotice`](crate::flows::repo_manager::CacheNotice)
//! whether it was called by a launch (whose vocabulary is `LaunchNotice`) or by a
//! lifecycle command, and the wrapping happens in one place per boundary rather
//! than at every push.

/// A channel for notices of one vocabulary.
///
/// Generic over the notice rather than one trait per flow: the vocabularies are
/// different types but the channel is the same idea, and a caller that wants to
/// print all of them writes one implementation per vocabulary and nothing else.
pub trait Notices<T> {
    /// Take one notice, now.
    fn say(&mut self, notice: T);
}

/// The plural form, on the trait object every flow signature names.
///
/// An inherent method on `dyn Notices<T>` rather than a defaulted trait method,
/// because a defaulted one taking `impl IntoIterator` would need `Self: Sized` and
/// so could not be called through the `&mut dyn` the flows are handed. There is
/// nothing for a sink to get wrong here: saying several is saying each.
impl<T> dyn Notices<T> + '_ {
    pub(crate) fn say_all(&mut self, notices: impl IntoIterator<Item = T>) {
        for notice in notices {
            self.say(notice);
        }
    }
}

/// Collect them, in order.
///
/// The sink a test uses, and the one a command that prints its notices underneath a
/// report of its own still wants: `--prune` says what it is about to do, asks, and
/// only then reports the warnings the scan produced.
impl<T> Notices<T> for Vec<T> {
    fn say(&mut self, notice: T) {
        self.push(notice);
    }
}

/// A sink of `T`s that puts each one into a sink of `U`s.
///
/// The boundary between two layers' vocabularies, as a value. `wrap` is a function
/// pointer and not a closure because every use of it *is* a plain function — an
/// enum variant's constructor, or a `match` over the arms — and a function pointer
/// keeps the type free of the lifetime a closure would carry.
pub(crate) struct Wrapped<'a, T, U> {
    inner: &'a mut dyn Notices<U>,
    wrap: fn(T) -> U,
}

impl<'a, T, U> Wrapped<'a, T, U> {
    pub fn new(inner: &'a mut dyn Notices<U>, wrap: fn(T) -> U) -> Self {
        Self { inner, wrap }
    }
}

impl<T, U> Notices<T> for Wrapped<'_, T, U> {
    fn say(&mut self, notice: T) {
        self.inner.say((self.wrap)(notice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Inner {
        One,
        Two,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Outer {
        Wrapped(Inner),
        Own,
    }

    #[test]
    fn a_vector_collects_in_order() {
        let mut collected: Vec<Inner> = Vec::new();
        let said: &mut dyn Notices<Inner> = &mut collected;
        said.say(Inner::One);
        said.say_all([Inner::Two, Inner::One]);
        assert_eq!(collected, [Inner::One, Inner::Two, Inner::One]);
    }

    #[test]
    fn a_wrapped_sink_lands_in_the_outer_vocabulary_where_it_happened() {
        // The ordering property the whole module exists for: an inner notice said
        // between two outer ones lands between them, not after both.
        let mut said: Vec<Outer> = Vec::new();
        said.say(Outer::Own);
        Wrapped::new(&mut said, Outer::Wrapped).say(Inner::Two);
        said.say(Outer::Own);
        assert_eq!(said, [Outer::Own, Outer::Wrapped(Inner::Two), Outer::Own]);
    }

    #[test]
    fn a_sink_can_be_a_caller_written_one() {
        // What the `dl` binary does: say it now. Held as a `dyn` to pin that the
        // trait is object-safe, which is what the flows' signatures need.
        struct Counting(usize);
        impl Notices<Inner> for Counting {
            fn say(&mut self, _notice: Inner) {
                self.0 += 1;
            }
        }
        let mut counting = Counting(0);
        let sink: &mut dyn Notices<Inner> = &mut counting;
        sink.say(Inner::One);
        sink.say(Inner::Two);
        assert_eq!(counting.0, 2);
    }
}
