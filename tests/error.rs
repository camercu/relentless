//! Acceptance tests for error types.
//!
//! These tests verify:
//! - RetryError::Exhausted carries `last: Result<T, E>`
//! - RetryError::Rejected carries `last: E`
//! - Display is implemented unconditionally
//! - std::error::Error is implemented when `std` + `E: Error + 'static`
//! - source() chains correctly for both variants
//! - Clone, PartialEq derives
//! - Accessor methods: last(), last_error(), into_last(), into_last_error()
//! - RetryResult alias matches Result<T, RetryError<T, E>>

// ---------------------------------------------------------------------------
// RetryError variants
// ---------------------------------------------------------------------------

#[test]
fn retry_error_exhausted_variant() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("connection refused".to_string()),
    };

    match err {
        tenacious::RetryError::Exhausted { ref last } => {
            assert_eq!(last, &Err("connection refused".to_string()));
        }
        _ => panic!("expected Exhausted variant"),
    }
}

#[test]
fn retry_error_rejected_variant() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };

    match err {
        tenacious::RetryError::Rejected { ref last } => {
            assert_eq!(last, "fatal");
        }
        _ => panic!("expected Rejected variant"),
    }
}

#[test]
fn retry_error_exhausted_with_ok_last() {
    let err: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted { last: Ok(42) };

    if let tenacious::RetryError::Exhausted { last } = err {
        assert_eq!(last, Ok(42));
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[test]
fn retry_error_display_includes_meaningful_content() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };

    let msg = format!("{}", err);
    assert!(
        msg.contains("timeout"),
        "Display should include the error message: {msg}"
    );

    let err2: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    let msg2 = format!("{}", err2);
    assert!(
        msg2.contains("fatal"),
        "Display should include the error message: {msg2}"
    );
}

// ---------------------------------------------------------------------------
// std::error::Error (feature = "std")
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "std")]
fn retry_error_implements_std_error_when_e_is_error_and_static() {
    let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Exhausted { last: Err(inner) };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Exhausted should chain to the inner error via source()"
    );
}

#[test]
#[cfg(feature = "std")]
fn retry_error_exhausted_ok_source_is_none() {
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Exhausted { last: Ok(()) };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_none(),
        "Exhausted with Ok has no source error"
    );
}

#[test]
#[cfg(feature = "std")]
fn retry_error_rejected_source_is_inner_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fatal");
    let err: tenacious::RetryError<(), std::io::Error> =
        tenacious::RetryError::Rejected { last: inner };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Rejected should chain to the inner error via source()"
    );
}

// ---------------------------------------------------------------------------
// Clone, PartialEq
// ---------------------------------------------------------------------------

#[test]
fn retry_error_derives_clone_and_partial_eq() {
    let err: tenacious::RetryError<(), String> = tenacious::RetryError::Exhausted {
        last: Err("fail".to_string()),
    };

    let cloned = err.clone();
    assert_eq!(err, cloned);
}

// ---------------------------------------------------------------------------
// Accessor methods
// ---------------------------------------------------------------------------

#[test]
fn retry_error_accessors_expose_last_outcome_and_error() {
    let exhausted: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };
    let expected_error = "timeout".to_string();
    assert_eq!(exhausted.last(), Some(&Err(expected_error.clone())));
    assert_eq!(exhausted.last_error(), Some(&expected_error));

    let rejected: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    assert_eq!(rejected.last(), None);
    assert_eq!(rejected.last_error(), Some(&"fatal".to_string()));
}

#[test]
fn retry_error_into_accessors_extract_owned_values() {
    let exhausted: tenacious::RetryError<i32, String> = tenacious::RetryError::Exhausted {
        last: Err("timeout".to_string()),
    };
    assert_eq!(exhausted.into_last(), Some(Err("timeout".to_string())));

    let rejected: tenacious::RetryError<i32, String> = tenacious::RetryError::Rejected {
        last: "fatal".to_string(),
    };
    assert_eq!(rejected.into_last_error(), Some("fatal".to_string()));
}

// ---------------------------------------------------------------------------
// RetryResult alias
// ---------------------------------------------------------------------------

#[test]
fn retry_error_is_usable_as_result_error_type() {
    fn fallible() -> Result<i32, tenacious::RetryError<(), String>> {
        Err(tenacious::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    assert!(fallible().is_err());
}

#[test]
fn retry_result_alias_matches_retry_error_shape() {
    fn fallible() -> tenacious::RetryResult<i32, String> {
        Err(tenacious::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    let result: Result<i32, tenacious::RetryError<i32, String>> = fallible();
    assert!(result.is_err());
}
