use sparrow_core::{
    ChannelId, CoreError, InputField, InputReason, PageLimit, SearchTerm, SourceConfigurationInput,
    SparrowCore,
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
fn channel_identifiers_are_parsed_canonically_at_the_public_boundary() {
    let canonical = format!("ch1_{}", "0a".repeat(32));
    let parsed = ChannelId::parse(canonical.clone()).expect("canonical Channel ID is valid");

    assert_eq!(parsed.as_str(), canonical);
    assert_eq!(format!("{parsed:?}"), "ChannelId(<redacted>)");

    for malformed in [
        String::new(),
        format!("ch1_{}", "a".repeat(63)),
        format!("ch1_{}", "a".repeat(65)),
        format!("CH1_{}", "a".repeat(64)),
        format!("ch1_{}A", "a".repeat(63)),
        format!("ch1_{}g", "a".repeat(63)),
        format!(" ch1_{}", "a".repeat(64)),
    ] {
        assert!(matches!(
            ChannelId::parse(malformed),
            Err(CoreError::InvalidInput {
                field: InputField::ChannelId,
                reason: InputReason::InvalidFormat,
            })
        ));
    }
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

#[test]
fn search_terms_are_bounded_and_canonical_at_the_public_boundary() {
    let term = SearchTerm::parse("  ＮEWS\u{a0}\tCaFÉ  ").expect("fixture term is valid");
    assert_eq!(term.as_str(), "news café");
    assert_eq!(format!("{term:?}"), "SearchTerm(<redacted>)");

    for blank in ["", "   \n\t", "\u{a0}"] {
        assert!(matches!(
            SearchTerm::parse(blank),
            Err(CoreError::InvalidInput {
                field: InputField::SearchTerm,
                reason: InputReason::Required,
            })
        ));
    }

    assert_eq!(
        SearchTerm::parse("a".repeat(256))
            .expect("the maximum decoded byte length is valid")
            .as_str()
            .len(),
        256
    );
    assert!(matches!(
        SearchTerm::parse("a".repeat(257)),
        Err(CoreError::InvalidInput {
            field: InputField::SearchTerm,
            reason: InputReason::TooLong { max_bytes: 256 },
        })
    ));
}
