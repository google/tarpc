#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
    #[cfg(all(feature = "serde-transport", feature = "tcp"))]
    t.compile_fail("tests/compile_fail/serde_transport/*.rs");
    #[cfg(not(feature = "serde1"))]
    t.compile_fail("tests/compile_fail/no_serde1/*.rs");
    #[cfg(feature = "serde1")]
    t.compile_fail("tests/compile_fail/serde1/*.rs");
}
