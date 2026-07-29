//! Emitting without an installed sink is a safe, silent no-op.
//! Own process on purpose: no other test may install a sink first.

#[test]
fn emitting_without_a_sink_is_a_no_op() {
    renew_diag::info!("nobody is listening: {}", 1);
    renew_diag::error!(target: "orphan", "still nobody: {:?}", (1, 2));
    // Reaching this line without a panic is the assertion.
}
