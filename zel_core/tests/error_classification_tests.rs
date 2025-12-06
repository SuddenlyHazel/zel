//! Comprehensive unit tests for error classification system
//!
//! These tests verify that the error classification system uses type-based detection
//! (not string matching) to correctly categorize errors into:
//! - Infrastructure: Automatically detected (IO, timeout, Iroh/Quinn errors)
//! - SystemFailure: Explicitly marked using .as_system_failure()
//! - Application: Default for unmarked errors

use anyhow::{anyhow, Error};
use std::io;
use std::sync::Arc;
use tokio::time::Duration;
use zel_core::protocol::{ErrorClassificationExt, ErrorClassifier, ErrorSeverity};

/// Test 1: Verify std::io::Error is classified as Infrastructure
#[test]
fn test_io_error_is_infrastructure() {
    let classifier = ErrorClassifier::default();

    // Create an IO error
    let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection failed");
    let err: Error = io_err.into();

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::Infrastructure,
        "std::io::Error should be classified as Infrastructure"
    );
}

/// Test 2: Verify different IO error kinds are all Infrastructure
#[test]
fn test_various_io_errors_are_infrastructure() {
    let classifier = ErrorClassifier::default();

    let test_cases = vec![
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::NotConnected,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::TimedOut,
    ];

    for kind in test_cases {
        let io_err = io::Error::new(kind, format!("IO error: {:?}", kind));
        let err: Error = io_err.into();

        assert_eq!(
            classifier.classify(&err),
            ErrorSeverity::Infrastructure,
            "IO error kind {:?} should be Infrastructure",
            kind
        );
    }
}

/// Test 3: Verify tokio timeout error is classified as Infrastructure
#[tokio::test]
async fn test_timeout_error_is_infrastructure() {
    let classifier = ErrorClassifier::default();

    // Create a real timeout error by timing out a future
    let result = tokio::time::timeout(Duration::from_millis(1), async {
        tokio::time::sleep(Duration::from_secs(10)).await;
    })
    .await;

    // This should be a timeout error
    assert!(result.is_err());
    let timeout_err = result.unwrap_err();
    let err: Error = timeout_err.into();

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::Infrastructure,
        "tokio::time::error::Elapsed should be classified as Infrastructure"
    );
}

/// Test 4: Verify errors marked with .as_system_failure() are classified as SystemFailure
#[test]
fn test_marked_system_failure() {
    let classifier = ErrorClassifier::default();

    // Create error and mark as system failure
    let err = anyhow!("Database connection pool exhausted").as_system_failure();

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::SystemFailure,
        "Errors marked with .as_system_failure() should be classified as SystemFailure"
    );
}

/// Test 5: Verify multiple errors can be marked as system failures
#[test]
fn test_multiple_system_failures() {
    let classifier = ErrorClassifier::default();

    let test_cases = vec![
        "Database unavailable",
        "Upstream service timeout",
        "Cache cluster unreachable",
        "Message queue full",
    ];

    for msg in test_cases {
        let err = anyhow!(msg).as_system_failure();
        assert_eq!(
            classifier.classify(&err),
            ErrorSeverity::SystemFailure,
            "System failure '{}' should be classified correctly",
            msg
        );
    }
}

/// Test 6: Verify unmarked errors default to Application
#[test]
fn test_unmarked_error_is_application() {
    let classifier = ErrorClassifier::default();

    // Create plain error without any marking
    let err = anyhow!("User not found");

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::Application,
        "Unmarked errors should default to Application"
    );
}

/// Test 7: Verify multiple unmarked errors are Application
#[test]
fn test_various_unmarked_errors_are_application() {
    let classifier = ErrorClassifier::default();

    let test_cases = vec![
        "User not found",
        "Invalid email format",
        "Permission denied",
        "Resource already exists",
        "Validation failed",
    ];

    for msg in test_cases {
        let err = anyhow!(msg);
        assert_eq!(
            classifier.classify(&err),
            ErrorSeverity::Application,
            "Unmarked error '{}' should be Application",
            msg
        );
    }
}

/// Test 8: CRITICAL - Verify NO false positives from string matching
/// This proves type-based detection is used, NOT string matching
#[test]
fn test_no_false_positives_from_string_keywords() {
    let classifier = ErrorClassifier::default();

    // Create errors with misleading keywords that might trigger naive string matching
    // These contain words like "connection", "timeout", "network", etc.
    // but they are NOT actual infrastructure errors
    let test_cases = vec![
        "User connection_policy violated",
        "Request timeout limit exceeded by user",
        "Connection pool size validation failed",
        "Network diagram rendering error",
        "IO permission check failed",
        "Database connection string invalid format",
        "Timeout configuration parsing error",
    ];

    for msg in test_cases {
        let err = anyhow!(msg);
        assert_eq!(
            classifier.classify(&err),
            ErrorSeverity::Application,
            "Error '{}' should NOT be classified as Infrastructure based on keywords alone",
            msg
        );
    }
}

/// Test 9: Verify .as_application() extension method works
#[test]
fn test_marked_application() {
    let classifier = ErrorClassifier::default();

    // Explicitly mark as application (though this is default behavior)
    let err = anyhow!("Business logic error").as_application();

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::Application,
        "Errors marked with .as_application() should be Application"
    );
}

/// Test 10: Verify .as_infrastructure() extension method works
#[test]
fn test_marked_infrastructure() {
    let classifier = ErrorClassifier::default();

    // Mark as infrastructure by wrapping in IO error
    let err = anyhow!("Network failure").as_infrastructure();

    // Classify
    let severity = classifier.classify(&err);

    // Assert
    assert_eq!(
        severity,
        ErrorSeverity::Infrastructure,
        "Errors marked with .as_infrastructure() should be Infrastructure"
    );
}

/// Test 11: Verify custom classifier function works
#[test]
fn test_custom_classifier() {
    // Create custom classifier that treats errors containing "CRITICAL" as system failures
    let classifier = ErrorClassifier::Custom(Arc::new(|err| {
        if err.to_string().contains("CRITICAL") {
            ErrorSeverity::SystemFailure
        } else {
            ErrorSeverity::Application
        }
    }));

    // Test critical error
    let critical_err = anyhow!("CRITICAL: System overload");
    assert_eq!(
        classifier.classify(&critical_err),
        ErrorSeverity::SystemFailure,
        "Custom classifier should identify critical errors"
    );

    // Test normal error
    let normal_err = anyhow!("Normal error");
    assert_eq!(
        classifier.classify(&normal_err),
        ErrorSeverity::Application,
        "Custom classifier should classify normal errors as Application"
    );
}

/// Test 12: Verify AllErrors classifier marks everything as SystemFailure
#[test]
fn test_all_errors_classifier() {
    let classifier = ErrorClassifier::AllErrors;

    // Test various error types - all should be SystemFailure
    let test_cases = vec![
        anyhow!("Any error"),
        anyhow!("User error"),
        anyhow!("Validation failure"),
    ];

    for err in test_cases {
        assert_eq!(
            classifier.classify(&err),
            ErrorSeverity::SystemFailure,
            "AllErrors classifier should mark everything as SystemFailure"
        );
    }
}

/// Test 13: Verify error chain is traversed for infrastructure detection
#[test]
fn test_error_chain_infrastructure_detection() {
    let classifier = ErrorClassifier::default();

    // Create a chain where IO error is the source
    let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "connection reset");
    let chained_err = Error::from(io_err).context("Failed to send data");

    // Classify - should detect IO error in chain
    let severity = classifier.classify(&chained_err);

    assert_eq!(
        severity,
        ErrorSeverity::Infrastructure,
        "Should detect infrastructure error in error chain"
    );
}

/// Test 14: Verify type-based detection with explicit type checking
#[test]
fn test_type_based_detection_explicit() {
    let classifier = ErrorClassifier::default();

    // Create IO error
    let io_err = io::Error::new(io::ErrorKind::Other, "test");
    let err: Error = io_err.into();

    // Verify the error contains std::io::Error type
    assert!(
        err.chain().any(|e| e.is::<std::io::Error>()),
        "Error should contain std::io::Error in chain"
    );

    // Verify it's classified as Infrastructure
    assert_eq!(
        classifier.classify(&err),
        ErrorSeverity::Infrastructure,
        "Type-based detection should identify std::io::Error"
    );
}

/// Test 15: Verify system failure marker takes precedence
#[test]
fn test_system_failure_marker_precedence() {
    let classifier = ErrorClassifier::default();

    // Create an error that would be Infrastructure, but mark it as SystemFailure
    let io_err = io::Error::new(io::ErrorKind::Other, "infrastructure error");
    let err: Error = io_err.into();

    // First verify it would be Infrastructure
    assert_eq!(
        classifier.classify(&err),
        ErrorSeverity::Infrastructure,
        "IO error should be Infrastructure"
    );

    // Now wrap it with system failure marker
    let marked_err = err.context("Wrapped").as_system_failure();

    // Should now be SystemFailure (marker takes precedence)
    assert_eq!(
        classifier.classify(&marked_err),
        ErrorSeverity::SystemFailure,
        "SystemFailure marker should take precedence over infrastructure detection"
    );
}

/// Test 16: Verify .as_application_error() alias works
#[test]
fn test_application_error_alias() {
    let classifier = ErrorClassifier::default();

    // Test both methods produce the same result
    let err1 = anyhow!("Test").as_application();
    let err2 = anyhow!("Test").as_application_error();

    assert_eq!(
        classifier.classify(&err1),
        ErrorSeverity::Application,
        ".as_application() should work"
    );

    assert_eq!(
        classifier.classify(&err2),
        ErrorSeverity::Application,
        ".as_application_error() should work"
    );
}

/// Test 17: Integration test - mixed error scenarios
#[test]
fn test_mixed_error_scenarios() {
    let classifier = ErrorClassifier::default();

    // Infrastructure error
    let infra = io::Error::new(io::ErrorKind::ConnectionRefused, "failed");
    assert_eq!(
        classifier.classify(&Error::from(infra)),
        ErrorSeverity::Infrastructure
    );

    // System failure
    let sys_fail = anyhow!("DB down").as_system_failure();
    assert_eq!(classifier.classify(&sys_fail), ErrorSeverity::SystemFailure);

    // Application error
    let app_err = anyhow!("User not found");
    assert_eq!(classifier.classify(&app_err), ErrorSeverity::Application);

    // Explicitly marked application
    let app_marked = anyhow!("Validation failed").as_application();
    assert_eq!(classifier.classify(&app_marked), ErrorSeverity::Application);

    // Explicitly marked infrastructure
    let infra_marked = anyhow!("Network issue").as_infrastructure();
    assert_eq!(
        classifier.classify(&infra_marked),
        ErrorSeverity::Infrastructure
    );
}

/// Test 18: Verify context preservation in error chain
#[test]
fn test_context_preservation() {
    let classifier = ErrorClassifier::default();

    // Create IO error with context
    let io_err = io::Error::new(io::ErrorKind::TimedOut, "timeout");
    let err = Error::from(io_err)
        .context("Failed to connect")
        .context("During initialization");

    // Should still be detected as Infrastructure
    assert_eq!(
        classifier.classify(&err),
        ErrorSeverity::Infrastructure,
        "Infrastructure detection should work through context chain"
    );
}
