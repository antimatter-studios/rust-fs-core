//! One check gates a pull request, and it stands for every job (#154, #156).
//!
//! The rules are in `am-ci-guard`, beside this crate in this workspace, and
//! shared with every driver in the family. This file is the whole of what a
//! repository has to say for itself: where it is.
//!
//! It used to be 173 lines, and the same 173 lines lived in ten other
//! repositories as ten more variants. See the crate docs for why the answer
//! is one source fetched at run time rather than an eleventh copy with a
//! drift test beside it.

#[test]
fn one_check_gates_a_pull_request_and_stands_for_every_job() {
    am_ci_guard::AggregateGate::new(env!("CARGO_MANIFEST_DIR")).verify();
}
