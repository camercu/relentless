//! Tests for `RetryError` and `RetryResult`.
//!
//! Verifies the variant payloads (Exhausted carries last: Result<T,E>; Aborted carries last: E),
//! Display output, the `std::error::Error` source chain under the `std` feature, accessor methods,
//! and the `RetryResult` type alias.

use core::convert::Infallible;
#[cfg(feature = "alloc")]
use core::ops::ControlFlow;

use relentless::{RetryError, StopReason};

#[test]
fn retry_error_exhausted_variant() {
    let err: relentless::RetryError<String, Result<(), String>> =
        relentless::RetryError::Exhausted {
            last: Err("connection refused".to_string()),
        };

    match err {
        relentless::RetryError::Exhausted { ref last } => {
            assert_eq!(last, &Err("connection refused".to_string()));
        }
        _ => panic!("expected Exhausted variant"),
    }
}

#[test]
fn retry_error_aborted_variant() {
    let err: relentless::RetryError<String, Result<(), String>> = relentless::RetryError::Aborted {
        last: "fatal".to_string(),
    };

    match err {
        relentless::RetryError::Aborted { ref last } => {
            assert_eq!(last, "fatal");
        }
        _ => panic!("expected Aborted variant"),
    }
}

#[test]
fn retry_error_exhausted_with_ok_last() {
    let err: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted { last: Ok(42) };

    if let relentless::RetryError::Exhausted { last } = err {
        assert_eq!(last, Ok(42));
    }
}

#[test]
fn retry_error_display_includes_meaningful_content() {
    let err: relentless::RetryError<String, Result<(), String>> =
        relentless::RetryError::Exhausted {
            last: Err("timeout".to_string()),
        };

    let msg = format!("{err}");
    assert!(
        msg.contains("timeout"),
        "Display should include the error message: {msg}"
    );

    let err2: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal".to_string(),
        };
    let msg2 = format!("{err2}");
    assert!(
        msg2.contains("fatal"),
        "Display should include the error message: {msg2}"
    );
}

#[test]
#[cfg(feature = "std")]
fn retry_error_implements_std_error_when_e_is_error_and_static() {
    let inner = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
    let err: relentless::RetryError<std::io::Error, Result<(), std::io::Error>> =
        relentless::RetryError::Exhausted { last: Err(inner) };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Exhausted should chain to the inner error via source()"
    );
}

#[test]
#[cfg(feature = "std")]
fn retry_error_exhausted_ok_source_is_none() {
    let err: relentless::RetryError<std::io::Error, Result<(), std::io::Error>> =
        relentless::RetryError::Exhausted { last: Ok(()) };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_none(),
        "Exhausted with Ok has no source error"
    );
}

#[test]
#[cfg(feature = "std")]
fn retry_error_aborted_source_is_inner_error() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fatal");
    let err: relentless::RetryError<std::io::Error, Result<(), std::io::Error>> =
        relentless::RetryError::Aborted { last: inner };

    let dyn_err: &dyn std::error::Error = &err;
    assert!(
        dyn_err.source().is_some(),
        "Aborted should chain to the inner error via source()"
    );
}

#[test]
fn retry_error_derives_clone_and_partial_eq() {
    let err: relentless::RetryError<String, Result<(), String>> =
        relentless::RetryError::Exhausted {
            last: Err("fail".to_string()),
        };

    let cloned = err.clone();
    assert_eq!(err, cloned);
}

#[test]
fn retry_error_accessors_expose_last_outcome_and_error() {
    let exhausted: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted {
            last: Err("timeout".to_string()),
        };
    let expected_error = "timeout".to_string();
    assert_eq!(exhausted.last(), Some(&Err(expected_error.clone())));
    assert_eq!(exhausted.last_error(), Some(&expected_error));

    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal".to_string(),
        };
    assert_eq!(rejected.last(), None);
    assert_eq!(rejected.last_error(), Some(&"fatal".to_string()));
}

#[test]
fn retry_error_into_accessors_extract_owned_values() {
    let exhausted: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted {
            last: Err("timeout".to_string()),
        };
    assert_eq!(exhausted.into_last(), Some(Err("timeout".to_string())));

    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal".to_string(),
        };
    assert_eq!(rejected.into_last_error(), Some("fatal".to_string()));
}

#[test]
fn retry_error_is_usable_as_result_error_type() {
    fn fallible() -> Result<i32, relentless::RetryError<String, Result<(), String>>> {
        Err(relentless::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    assert!(fallible().is_err());
}

#[test]
fn retry_result_alias_matches_retry_error_shape() {
    fn fallible() -> relentless::RetryResult<i32, String> {
        Err(relentless::RetryError::Exhausted {
            last: Err("fail".to_string()),
        })
    }

    let result: Result<i32, relentless::RetryError<String, Result<i32, String>>> = fallible();
    assert!(result.is_err());
}

/// 4.1.8
#[test]
fn retry_error_stop_reason_matches_variant() {
    use relentless::StopReason;

    let exhausted: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted {
            last: Err("fail".to_string()),
        };
    assert_eq!(exhausted.stop_reason(), StopReason::Exhausted);

    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal".to_string(),
        };
    assert_eq!(rejected.stop_reason(), StopReason::Aborted);
}

/// 4.1.9
#[test]
fn retry_error_display_exact_format() {
    let exhausted: relentless::RetryError<String, Result<(), String>> =
        relentless::RetryError::Exhausted {
            last: Err("connection refused".to_string()),
        };
    let msg = format!("{exhausted}");
    assert_eq!(
        msg, "retries exhausted: connection refused",
        "Exhausted Display format should match spec"
    );

    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal error".to_string(),
        };
    let msg2 = format!("{rejected}");
    assert_eq!(
        msg2, "aborted: fatal error",
        "Aborted Display format should match spec"
    );

    // Exhausted with Ok(T) — no error to display, just "retries exhausted"
    let exhausted_ok: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted { last: Ok(42) };
    let msg3 = format!("{exhausted_ok}");
    assert_eq!(msg3, "retries exhausted");
}

/// 4.1.4
#[test]
fn retry_error_last_returns_some_for_exhausted_none_for_aborted() {
    let exhausted: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted {
            last: Err("fail".to_string()),
        };
    assert!(exhausted.last().is_some());

    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "fatal".to_string(),
        };
    assert!(rejected.last().is_none());
}

/// 4.1.5
#[test]
fn retry_error_into_last_exhausted_ok_returns_some_ok() {
    let exhausted_ok: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted { last: Ok(99) };
    assert_eq!(exhausted_ok.into_last(), Some(Ok(99_i32)));
}

/// 4.1.6
#[test]
fn retry_error_last_error_returns_none_for_exhausted_ok() {
    let exhausted_ok: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted { last: Ok(1) };
    assert!(exhausted_ok.last_error().is_none());

    let exhausted_err: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Exhausted {
            last: Err("oops".to_string()),
        };
    assert_eq!(exhausted_err.last_error(), Some(&"oops".to_string()));
}

/// 4.1.7
#[test]
fn retry_error_into_last_error_for_aborted() {
    let rejected: relentless::RetryError<String, Result<i32, String>> =
        relentless::RetryError::Aborted {
            last: "rejected-error".to_string(),
        };
    assert_eq!(
        rejected.into_last_error(),
        Some("rejected-error".to_string())
    );
}

/// 16.5
#[test]
fn stop_reason_available_without_extra_bounds() {
    use relentless::StopReason;

    // Neither T nor E implement any special trait — just Copy.
    #[derive(Copy, Clone)]
    struct Opaque;

    let exhausted: relentless::RetryError<Opaque, Result<Opaque, Opaque>> =
        relentless::RetryError::Exhausted { last: Err(Opaque) };
    assert_eq!(exhausted.stop_reason(), StopReason::Exhausted);

    let rejected: relentless::RetryError<Opaque, Result<Opaque, Opaque>> =
        relentless::RetryError::Aborted { last: Opaque };
    assert_eq!(rejected.stop_reason(), StopReason::Aborted);
}

// RetryError accessors and Display over non-`Result` outcome shapes.

type StrErr = RetryError<&'static str, Result<i32, &'static str>>;

#[test]
fn stop_reason_matches_the_variant() {
    let aborted: StrErr = RetryError::Aborted { last: "x" };
    let exhausted: StrErr = RetryError::Exhausted { last: Err("y") };
    assert_eq!(aborted.stop_reason(), StopReason::Aborted);
    assert_eq!(exhausted.stop_reason(), StopReason::Exhausted);
}

#[test]
fn last_retains_the_outcome_only_on_exhausted() {
    let exhausted: StrErr = RetryError::Exhausted { last: Ok(3) };
    let aborted: StrErr = RetryError::Aborted { last: "boom" };
    assert_eq!(exhausted.last(), Some(&Ok(3)));
    assert_eq!(aborted.last(), None);
}

#[test]
fn last_is_available_for_non_result_outcomes() {
    // An `Option`-outcome op (or any custom `Outcome`) produces this error
    // shape; `last`/`into_last` are outcome-agnostic and must work on it,
    // not only on the `Result` shape.
    type OptErr = RetryError<Infallible, Option<i32>>;
    let exhausted: OptErr = RetryError::Exhausted { last: None };
    assert_eq!(exhausted.last(), Some(&None));
    assert_eq!(exhausted.into_last(), Some(None));
}

#[test]
fn last_error_collapses_to_the_error() {
    let aborted: StrErr = RetryError::Aborted { last: "boom" };
    let exhausted_err: StrErr = RetryError::Exhausted { last: Err("net") };
    let exhausted_ok: StrErr = RetryError::Exhausted { last: Ok(1) };
    assert_eq!(aborted.last_error(), Some(&"boom"));
    assert_eq!(exhausted_err.last_error(), Some(&"net"));
    assert_eq!(exhausted_ok.last_error(), None);
}

#[cfg(feature = "alloc")]
#[test]
fn display_reports_the_terminal_reason() {
    let aborted: StrErr = RetryError::Aborted { last: "boom" };
    let exhausted: StrErr = RetryError::Exhausted { last: Err("net") };
    assert_eq!(aborted.to_string(), "aborted: boom");
    assert_eq!(exhausted.to_string(), "retries exhausted: net");
}

#[cfg(feature = "alloc")]
#[test]
fn option_error_display_reports_exhaustion() {
    // Option's only reachable terminal error is exhaustion (abort is
    // `Infallible`), so the message needs no outcome detail.
    let exhausted: RetryError<Infallible, Option<i32>> = RetryError::Exhausted { last: None };
    assert_eq!(exhausted.to_string(), "retries exhausted");
}

#[cfg(feature = "std")]
#[test]
fn option_error_is_a_std_error_with_no_source() {
    let exhausted: RetryError<Infallible, Option<i32>> = RetryError::Exhausted { last: None };
    let err: &dyn std::error::Error = &exhausted;
    assert!(err.source().is_none());
}

#[cfg(feature = "alloc")]
#[test]
fn control_flow_error_display_reports_exhaustion() {
    let exhausted: RetryError<Infallible, ControlFlow<i32, i32>> = RetryError::Exhausted {
        last: ControlFlow::Continue(0),
    };
    assert_eq!(exhausted.to_string(), "retries exhausted");
}

#[cfg(feature = "std")]
#[test]
fn control_flow_error_is_a_std_error_with_no_source() {
    let exhausted: RetryError<Infallible, ControlFlow<i32, i32>> = RetryError::Exhausted {
        last: ControlFlow::Continue(0),
    };
    let err: &dyn std::error::Error = &exhausted;
    assert!(err.source().is_none());
}

/// 4.1.9
#[test]
fn retry_error_display_format() {
    use relentless::RetryError;

    let e: RetryError<String, Result<(), String>> = RetryError::Exhausted {
        last: Err("network timeout".to_string()),
    };
    assert_eq!(format!("{e}"), "retries exhausted: network timeout");

    let r: RetryError<String, Result<i32, String>> = RetryError::Aborted {
        last: "fatal".to_string(),
    };
    assert_eq!(format!("{r}"), "aborted: fatal");
}
