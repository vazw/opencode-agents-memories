//! `runtime::ort_environment` contract (blueprint step 3; covers the
//! `onnxruntime-resolve.test.ts` disposition).
//!
//! The upstream Node/Bun module-resolution shim cases (binding path probing,
//! `require` redirection, pinned `onnxruntime-node` root) are N-A in the Rust
//! port: `ort` statically links ONNX Runtime, so there is no binding file to
//! locate. What survives as a behavioral contract is (a) exactly one
//! environment per process — the rc.14 `CURRENT_ENV` rule our `OnceLock`
//! exists to satisfy — and (b) the actionable init-error message that tells an
//! operator to switch to the remote provider instead.

#[test]
fn test_ort_environment_initializes_once_per_process() {
    let first = mem_embed::ort_environment().expect("first environment init");
    let second = mem_embed::ort_environment().expect("second environment init");
    // `Environment` is Arc-backed and `Clone`, so both handles are the same
    // process-wide environment (exchanging them is a no-op).
    let _ = (first, second);

    // Proof that the `OnceLock` is the sole builder in this process: any other
    // builder call now trips ort's single-environment rule.
    let err = ort::init()
        .with_name("runtime-init-test-second-env")
        .build()
        .expect_err("a second ort environment must be refused");
    assert!(
        err.to_string().contains("only one environment"),
        "expected ort's single-environment error, got: {err}"
    );
}

#[test]
fn test_runtime_init_error_message_names_cause_and_actionable_fallback() {
    let cause = "libonnxruntime.so: cannot open shared object file";
    let formatted = mem_embed::format_runtime_init_error(cause);
    assert!(formatted.contains(cause), "cause missing: {formatted}");
    assert!(
        formatted.contains("embeddingApiUrl"),
        "fallback missing: {formatted}"
    );
    assert!(
        formatted.contains("embeddingApiKey"),
        "fallback missing: {formatted}"
    );

    // The same actionable text is what an operator sees when the environment
    // itself is what failed to initialize.
    let error = mem_embed::EmbedError::RuntimeInit {
        message: cause.to_string(),
    };
    let display = error.to_string();
    assert!(display.contains(cause), "cause missing: {display}");
    assert!(
        display.contains("embeddingApiUrl"),
        "fallback missing: {display}"
    );
    assert!(
        display.contains("embeddingApiKey"),
        "fallback missing: {display}"
    );
}
