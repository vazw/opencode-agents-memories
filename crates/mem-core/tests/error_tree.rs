//! Typed-error contract tests for `mem_core::error`.

use std::error::Error as _;
use std::path::PathBuf;

use mem_core::error::{ConfigError, Error, LoggingError};

#[test]
fn test_project_remote_fields_error_names_fields_and_path() {
    let error = ConfigError::ProjectRemoteProviderFields {
        fields: vec!["memoryProvider".to_owned(), "memoryApiUrl".to_owned()],
        global_config_path: PathBuf::from("/home/tester/.config/opencode/opencode-mem.jsonc"),
    };

    let display = error.to_string();
    assert!(
        display.contains("memoryProvider, memoryApiUrl"),
        "display must name the offending fields in order: {display}"
    );
    assert!(
        display.contains("/home/tester/.config/opencode/opencode-mem.jsonc"),
        "display must name the global config path: {display}"
    );
    assert_eq!(
        display,
        "Project config cannot set remote provider fields: memoryProvider, memoryApiUrl. \
Move them to the global config at /home/tester/.config/opencode/opencode-mem.jsonc."
    );

    // Fields stay structured, so callers can match on them.
    match &error {
        ConfigError::ProjectRemoteProviderFields {
            fields,
            global_config_path,
        } => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0], "memoryProvider");
            assert_eq!(fields[1], "memoryApiUrl");
            assert_eq!(
                global_config_path,
                &PathBuf::from("/home/tester/.config/opencode/opencode-mem.jsonc")
            );
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn test_invalid_value_error_names_key() {
    let error = ConfigError::InvalidValue {
        key: "autoCaptureMaxContextBytes",
        value: "1024".to_owned(),
    };

    let display = error.to_string();
    assert!(
        display.contains("autoCaptureMaxContextBytes"),
        "display must name the offending key: {display}"
    );
    assert_eq!(display, "Invalid autoCaptureMaxContextBytes config: 1024");

    let expected = ConfigError::InvalidValueExpected {
        key: "embeddingDtype",
        value: "banana".to_owned(),
        expected: "one of auto, fp32",
    };
    assert_eq!(
        expected.to_string(),
        "Invalid embeddingDtype config: banana (expected one of auto, fp32)"
    );
}

#[test]
fn test_error_source_chain_preserved() {
    // Top-level -> ConfigError hop.
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "secret file gone");
    let top = Error::Config(ConfigError::SecretFileRead {
        path: PathBuf::from("/tmp/key.txt"),
        source: io_error,
    });

    let source = top
        .source()
        .expect("Error::Config must chain to ConfigError");
    let inner = source
        .downcast_ref::<ConfigError>()
        .expect("source must be the ConfigError itself");
    let ConfigError::SecretFileRead { path, .. } = inner else {
        panic!("wrong inner variant: {inner:?}")
    };
    assert_eq!(path, &PathBuf::from("/tmp/key.txt").as_path());

    // ConfigError -> io::Error hop.
    let io_error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let secret = ConfigError::SecretFileRead {
        path: PathBuf::from("/tmp/key.txt"),
        source: io_error,
    };
    let source = secret
        .source()
        .expect("SecretFileRead must chain to io error");
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("source must be an io error");
    assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);

    // Variants without a cause must not invent one.
    assert!(
        ConfigError::MissingHomeDirectory.source().is_none(),
        "MissingHomeDirectory has no source"
    );
    assert!(
        LoggingError::AlreadyInstalled.source().is_none(),
        "AlreadyInstalled has no source"
    );
    assert!(
        top.source()
            .and_then(|e| e.downcast_ref::<ConfigError>())
            .and_then(|e| e.source())
            .is_some(),
        "secret io cause must survive the full chain"
    );

    // LoggingError surfaces as Error::Logging with the same chain rules.
    let logging = Error::Logging(LoggingError::AlreadyInstalled);
    assert_eq!(
        logging.to_string(),
        LoggingError::AlreadyInstalled.to_string()
    );
}
