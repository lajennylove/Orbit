//! Compile-fail tests for the derive's error messages. Run with `TRYBUILD=overwrite` to update
//! the expected `.stderr` files.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
