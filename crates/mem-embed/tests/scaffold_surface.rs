mod common;

#[test]
fn test_embed_task_labels_are_document_and_query() {
    use mem_embed::embedder::EmbedTask;
    assert_eq!(EmbedTask::Document.as_str(), "document");
    assert_eq!(EmbedTask::Query.as_str(), "query");
}

#[test]
fn test_embed_error_retryable_classification() {
    use mem_embed::error::EmbedError;
    assert!(
        EmbedError::RemoteHttp {
            status: 429,
            status_text: String::new()
        }
        .is_retryable()
    );
    assert!(
        EmbedError::RemoteHttp {
            status: 500,
            status_text: String::new()
        }
        .is_retryable()
    );
    assert!(
        !EmbedError::RemoteHttp {
            status: 401,
            status_text: String::new()
        }
        .is_retryable()
    );
    assert!(
        !EmbedError::RemoteHttp {
            status: 404,
            status_text: String::new()
        }
        .is_retryable()
    );
    assert!(
        EmbedError::RemoteNetwork {
            message: String::new()
        }
        .is_retryable()
    );
    assert!(EmbedError::RemoteTimeout { timeout_ms: 1 }.is_retryable());
    assert!(
        !EmbedError::RemoteInvalidJson {
            message: String::new()
        }
        .is_retryable()
    );
    assert!(
        !EmbedError::RuntimeInit {
            message: String::new()
        }
        .is_retryable()
    );
    assert!(
        !EmbedError::ModelFilesMissing {
            model: String::new(),
            path: String::new().into()
        }
        .is_retryable()
    );
    assert!(!EmbedError::Timeout { timeout_ms: 1 }.is_retryable());
    assert!(
        !EmbedError::WarmupFailed {
            message: String::new()
        }
        .is_retryable()
    );
}

#[test]
fn test_embed_error_display_names_status_and_paths() {
    use mem_embed::error::EmbedError;
    let e = EmbedError::RemoteHttp {
        status: 429,
        status_text: "Error".to_string(),
    };
    assert_eq!(e.to_string(), "API embedding failed: 429 Error");
    let e = EmbedError::ModelFilesMissing {
        model: "m".to_string(),
        path: "p".to_string().into(),
    };
    assert!(e.to_string().contains("m") && e.to_string().contains("p"));
    let e = EmbedError::InvalidModelId {
        model: "x".to_string(),
    };
    assert_eq!(e.to_string(), "Invalid embedding model id: x");
}

#[test]
fn test_common_temp_dir_guard_removes_tree() {
    let g = common::temp_dir("scaffold");
    let f = g.path().join("f.txt");
    std::fs::write(&f, "x").unwrap();
    let p = g.path().to_path_buf();
    drop(g);
    assert!(!p.exists());
}
