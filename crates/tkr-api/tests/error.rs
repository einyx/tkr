use tkr_api::Error;

#[test]
fn unknown_method_round_trips_via_display() {
    let e = Error::UnknownMethod("foo".into());
    assert!(format!("{e}").contains("foo"));
}
