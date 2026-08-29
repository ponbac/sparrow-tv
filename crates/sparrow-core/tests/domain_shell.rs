use sparrow_core::{
    CoreError, InputField, InputReason, PageLimit, SourceConfigurationInput, SparrowCore,
};

#[test]
fn blank_m3u_source_is_rejected_with_a_safe_typed_error() {
    let result = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
        "  \n ",
        None::<String>,
    ));

    assert!(matches!(
        result,
        Err(CoreError::InvalidInput {
            field: InputField::M3u,
            reason: InputReason::Required,
        })
    ));
}

#[test]
fn malformed_or_unsupported_source_locations_are_rejected_at_the_core_boundary() {
    for location in [
        "not a source location",
        "ftp://provider.fixture.invalid/list.m3u",
    ] {
        let result = SparrowCore::parse_source_configuration(SourceConfigurationInput::new(
            location,
            None::<String>,
        ));

        assert!(matches!(
            result,
            Err(CoreError::InvalidInput {
                field: InputField::M3u,
                reason: InputReason::UnsupportedLocation,
            })
        ));
    }
}

#[test]
fn page_limits_are_refined_at_the_public_boundary() {
    for value in [0, PageLimit::MAX + 1] {
        assert!(matches!(
            PageLimit::new(value),
            Err(CoreError::InvalidInput {
                field: InputField::PageLimit,
                reason: InputReason::OutOfRange,
            })
        ));
    }
    assert_eq!(
        PageLimit::new(PageLimit::MAX)
            .expect("maximum is valid")
            .get(),
        PageLimit::MAX
    );
}
