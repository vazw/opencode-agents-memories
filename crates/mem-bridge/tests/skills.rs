//! Issue 69 — the `skills` wire method's engine-level oracles.
//!
//! Every oracle dispatches through [`Engine::dispatch`] — the fixture
//! oracles' own seam — except the pure path helper, which is called
//! directly. The manager is the REAL W6.4 [`SkillsManager`] over a temp
//! storage root, seeded through its own `create` path.

mod common;

use std::path::Path;
use std::sync::Arc;

use mem_bridge::engine::{Engine, EngineConfig, EngineRuntime};
use mem_bridge::jsonrpc::{Request, Response, parse_line};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_bridge::skills::SkillsService;
use mem_skills::SkillsManager;
use mem_skills::schema::SkillInput;
use mem_store::serde_json::{Value, json};

use common::{FixedScopeResolver, TestDir, build_fixture_runtime};

/// The `container_tag` the fixture's [`FixedScopeResolver`] derives from any
/// non-blank project id (its `Default` value).
const TAG: &str = "opencode_project_0123456789abcdef";

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn skills_manager(dir: &TestDir) -> Arc<SkillsManager> {
    Arc::new(SkillsManager::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ))
}

/// The fixture engine over the same temp root, with the real skills service.
async fn engine_over(dir: &TestDir, manager: Arc<SkillsManager>) -> Engine {
    let fixture = build_fixture_runtime(dir.path()).await;
    let resolver: Arc<dyn ProjectScopeResolver> = Arc::new(FixedScopeResolver::default());
    let runtime = EngineRuntime {
        capture: fixture.capture,
        inject: fixture.inject,
        recall: fixture.recall,
        skills: Some(Arc::new(SkillsService::new(manager, resolver))),
    };
    Engine::with_runtime(EngineConfig::default(), Arc::new(runtime))
}

async fn dispatch_line(engine: &Engine, line: &str) -> Response {
    let request: Request = parse_line(line).unwrap();
    engine.dispatch(&request).await
}

fn result_of(response: &Response) -> &Value {
    response
        .result
        .as_ref()
        .unwrap_or_else(|| panic!("expected a result, got {:?}", response.error))
}

/// One seeded skill: `description: None` keeps the manager's empty string.
fn input(
    id: &str,
    name: &str,
    description: Option<&str>,
    content: &str,
    autoinvoke: bool,
    enabled: bool,
    container_tag: Option<&str>,
) -> SkillInput {
    SkillInput {
        id: id.to_string(),
        name: name.to_string(),
        description: description.map(str::to_string),
        content: content.to_string(),
        autoinvoke: Some(autoinvoke),
        enabled: Some(enabled),
        container_tag: container_tag.map(str::to_string),
    }
}

#[test]
fn test_skills_returns_the_catalogue_with_absolute_paths_and_the_skill_info_fields() {
    block_on(async {
        let dir = TestDir::new("issue69-skills-fields");
        let manager = skills_manager(&dir);
        manager
            .create(input(
                "git-release",
                "Git Release",
                Some("Prepare releases"),
                "Prepare releases",
                true,
                true,
                Some(TAG),
            ))
            .await
            .unwrap();
        manager
            .create(input(
                "docs",
                "Docs",
                None,
                "Write the docs",
                false,
                true,
                Some(TAG),
            ))
            .await
            .unwrap();
        let engine = engine_over(&dir, manager).await;

        // The handshake advertises both literals the plugin gates on.
        let hello = dispatch_line(
            &engine,
            r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{}}"#,
        )
        .await;
        let hello_result = result_of(&hello);
        assert_eq!(
            hello_result["capabilities"]["skills"].as_bool(),
            Some(true),
            "hello must advertise capabilities.skills: {hello_result}"
        );
        assert!(
            hello_result["methods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|method| method.as_str() == Some("skills")),
            "hello must list the skills method: {hello_result}"
        );

        let response = dispatch_line(
            &engine,
            r#"{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{"project_id":"/workspace/project"}}"#,
        )
        .await;
        let result = result_of(&response);
        let entries = result["skills"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "both enabled project skills: {result}");

        let git = entries
            .iter()
            .find(|entry| entry["id"] == "git-release")
            .expect("git-release present");
        assert_eq!(git["name"], "Git Release");
        assert_eq!(git["description"], "Prepare releases");
        assert_eq!(git["content"], "Prepare releases");
        assert_eq!(git["autoinvoke"].as_bool(), Some(true));

        let docs = entries
            .iter()
            .find(|entry| entry["id"] == "docs")
            .expect("docs present");
        assert_eq!(docs["name"], "Docs");
        assert_eq!(docs["content"], "Write the docs");
        assert_eq!(docs["autoinvoke"].as_bool(), Some(false));
        assert!(
            docs.get("description").is_none(),
            "an empty description is omitted: {docs}"
        );

        for entry in entries {
            let id = entry["id"].as_str().unwrap();
            let path = entry["path"].as_str().expect("path is a string");
            assert!(
                Path::new(path).is_absolute(),
                "every wire path is absolute: {path}"
            );
            assert_eq!(
                Path::new(path),
                dir.path()
                    .join("skills-content")
                    .join(id)
                    .join("SKILL.md")
                    .as_path()
            );
            let mut expected: Vec<&str> = vec!["autoinvoke", "content", "id", "name", "path"];
            if entry.get("description").is_some() {
                expected.push("description");
            }
            let mut actual: Vec<&str> = entry
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            expected.sort_unstable();
            actual.sort_unstable();
            assert_eq!(actual, expected, "the Skill.Info field set, no extras");
        }
    });
}

#[test]
fn test_skills_answers_an_empty_list_for_an_unknown_project() {
    block_on(async {
        let dir = TestDir::new("issue69-skills-empty");
        let engine = engine_over(&dir, skills_manager(&dir)).await;
        let response = dispatch_line(
            &engine,
            r#"{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{"project_id":"/workspace/never-seen"}}"#,
        )
        .await;
        assert!(
            response.error.is_none(),
            "an empty catalogue is not an error: {:?}",
            response.error
        );
        assert_eq!(result_of(&response)["skills"], json!([]));
    });
}

#[test]
fn test_skills_excludes_disabled_and_foreign_project_skills() {
    block_on(async {
        let dir = TestDir::new("issue69-skills-scope");
        let manager = skills_manager(&dir);
        // 4 rows: 1 matching enabled, 1 global enabled, 1 matching disabled,
        // 1 foreign-tag enabled. The rule returns exactly 2.
        for (id, enabled, tag) in [
            ("matching", true, Some(TAG)),
            ("global", true, None),
            ("disabled", false, Some(TAG)),
            (
                "foreign",
                true,
                Some("opencode_otherproject_fedcba9876543210"),
            ),
        ] {
            manager
                .create(input(id, id, None, "body", true, enabled, tag))
                .await
                .unwrap();
        }
        let engine = engine_over(&dir, manager).await;
        let response = dispatch_line(
            &engine,
            r#"{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{"project_id":"/workspace/project"}}"#,
        )
        .await;
        let result = result_of(&response);
        let ids: Vec<&str> = result["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 2, "enabled project + global only: {ids:?}");
        assert!(ids.contains(&"matching"));
        assert!(ids.contains(&"global"));
        assert!(!ids.contains(&"disabled"));
        assert!(!ids.contains(&"foreign"));
    });
}

#[test]
fn test_skills_requires_a_non_blank_project_id() {
    block_on(async {
        let dir = TestDir::new("issue69-skills-params");
        let manager = skills_manager(&dir);
        manager
            .create(input("global", "Global", None, "body", true, true, None))
            .await
            .unwrap();
        let engine = engine_over(&dir, manager).await;
        for (case, params) in [
            ("missing", r#"{}"#),
            ("blank", r#"{"project_id":"   "}"#),
            ("non-string", r#"{"project_id":7}"#),
        ] {
            let response = dispatch_line(
                &engine,
                &format!(
                    r#"{{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{params}}}"#
                ),
            )
            .await;
            assert_eq!(
                response.error.as_ref().map(|error| error.code),
                Some(-32602),
                "a {case} project_id is Invalid params, never a global read"
            );
        }
    });
}

#[test]
fn test_skills_omits_an_empty_description() {
    block_on(async {
        let dir = TestDir::new("issue69-skills-description");
        let manager = skills_manager(&dir);
        manager
            .create(input(
                "empty-description",
                "Empty",
                Some(""),
                "body",
                true,
                true,
                Some(TAG),
            ))
            .await
            .unwrap();
        manager
            .create(input(
                "described",
                "Described",
                Some("kept"),
                "body",
                true,
                true,
                Some(TAG),
            ))
            .await
            .unwrap();
        let engine = engine_over(&dir, manager).await;
        let response = dispatch_line(
            &engine,
            r#"{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{"project_id":"/workspace/project"}}"#,
        )
        .await;
        let result = result_of(&response);
        let entries = result["skills"].as_array().unwrap();
        let empty = entries
            .iter()
            .find(|entry| entry["id"] == "empty-description")
            .expect("empty-description present");
        assert!(
            empty.get("description").is_none(),
            "the manager trims an empty description; the wire omits it: {empty}"
        );
        let described = entries
            .iter()
            .find(|entry| entry["id"] == "described")
            .expect("described present");
        assert_eq!(described["description"], "kept");
    });
}

#[test]
fn test_catalogue_path_absolutises_a_relative_storage_root() {
    let relative = mem_bridge::skills::absolute_storage_root(Path::new("rel/root"));
    assert!(
        relative.is_absolute(),
        "a relative configured root still yields an absolute SKILL.md path: {relative:?}"
    );
    assert!(relative.ends_with("rel/root"));
    let absolute = std::env::temp_dir().join("issue69-absolute-root");
    assert_eq!(
        mem_bridge::skills::absolute_storage_root(&absolute),
        absolute,
        "an absolute root is returned unchanged"
    );
}

#[test]
fn test_skills_degrades_without_a_runtime() {
    block_on(async {
        let engine = Engine::new(EngineConfig::default());
        let response = dispatch_line(
            &engine,
            r#"{"id":"skills-1","jsonrpc":"2.0","method":"skills","params":{"project_id":"/workspace/project"}}"#,
        )
        .await;
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(-32001),
            "a runtime-less engine degrades, never -32601/-32603"
        );
    });
}
