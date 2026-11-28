// Read-only probe: resolve a config and print the build's fingerprint.
// CONFIG_HOME selects the `$HOME` whose `.config/opencode/opencode-mem.jsonc`
// is read. Loads no store, writes nothing.
use std::collections::BTreeMap;

use mem_core::config::{Config, ConfigPaths};
use mem_core::embedding::EmbeddingIdentity;

fn main() {
    let home = std::path::PathBuf::from(
        std::env::var("CONFIG_HOME").unwrap_or_else(|_| "/home/opencode".to_string()),
    );
    let paths = ConfigPaths::standard(home);
    let env: BTreeMap<String, String> = [("HOME".to_string(), "/home/opencode".to_string())]
        .into_iter()
        .collect();
    let cfg = Config::load(&paths, &env).expect("config loads");
    let identity = EmbeddingIdentity::from_config(&cfg);
    println!("model={}", identity.model);
    println!("dimensions={}", identity.dimensions);
    println!("pooling={:?}", identity.pooling);
    println!("dtype={:?}", identity.dtype);
    println!("use_task_prefixes={}", identity.use_task_prefixes);
    println!("query_prefix={:?}", identity.query_prefix);
    println!("document_prefix={:?}", identity.document_prefix);
    println!("max_tokens={:?}", identity.max_tokens);
    println!(
        "FINGERPRINT={}",
        identity.fingerprint().expect("serializes")
    );
}
