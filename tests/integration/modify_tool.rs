use togi::tools::modify::{Modify, ModifyArgs};

const LARGE_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

fn tool() -> Box<dyn rig::tool::ToolDyn> {
    crate::support::inject_cwd(std::env::temp_dir(), Modify)
}

#[test]
fn modify_args_schema_should_expose_public_fields_and_hide_injected_cwd() {
    let schema = serde_json::to_value(schemars::schema_for!(ModifyArgs)).unwrap();
    let properties = schema["properties"].as_object().unwrap();

    assert!(properties.contains_key("path"));
    assert!(properties.contains_key("content"));
    assert!(properties.contains_key("old_text"));
    assert!(properties.contains_key("new_text"));
    assert!(properties.contains_key("edits"));
    assert!(properties.contains_key("content_base64"));
    assert!(properties.contains_key("dry_run"));
    assert!(!properties.contains_key("cwd"));
}

#[tokio::test]
async fn call_should_create_text_file_when_content_is_provided() {
    let path = crate::support::temp_path("modify-create.txt");
    crate::support::remove_file(&path);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello world\n",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("created"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world\n");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_include_diff_when_editing_text_file() {
    let path = crate::support::temp_path("modify-diff-edit.txt");
    std::fs::write(&path, "a\nb\nc\n").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "b",
        "new_text": "B",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("edited"));
    assert!(output.contains("@@"));
    assert!(output.contains("-b\n"));
    assert!(output.contains("+B\n"));
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_include_diff_when_creating_text_file() {
    let path = crate::support::temp_path("modify-diff-create.txt");
    crate::support::remove_file(&path);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello\nworld\n",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("created"));
    assert!(output.contains("--- /dev/null"));
    assert!(output.contains("@@ -0,0 +1,2 @@"));
    assert!(output.contains("+hello\n"));
    assert!(output.contains("+world\n"));
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_report_no_changes_when_overwriting_with_same_content() {
    let path = crate::support::temp_path("modify-diff-same.txt");
    std::fs::write(&path, "same\n").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "same\n",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("overwrote"));
    assert!(output.contains("(no changes)"));
    assert!(!output.contains("@@"));
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_overwrite_existing_text_file() {
    let path = crate::support::temp_path("modify-overwrite.txt");
    std::fs::write(&path, "old").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "new",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("overwrote"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_apply_single_unique_replacement() {
    let path = crate::support::temp_path("modify-edit.txt");
    std::fs::write(&path, "alpha beta gamma").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "beta",
        "new_text": "BETA",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("edited"));
    assert!(output.contains("1 replacement"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha BETA gamma");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_combine_edits_array_with_single_old_text() {
    let path = crate::support::temp_path("modify-combine.txt");
    std::fs::write(&path, "one two three").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "edits": [{"old_text": "one", "new_text": "1"}],
        "old_text": "three",
        "new_text": "3",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("2 replacements"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "1 two 3");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_reject_conflicting_content_and_edit_instructions() {
    let path = crate::support::temp_path("modify-conflict.txt");
    std::fs::write(&path, "data").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "whole new body",
        "old_text": "data",
    });

    let result = tool.call(args.to_string()).await;

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_require_write_or_edit_instructions() {
    let path = crate::support::temp_path("modify-noop.txt");
    std::fs::write(&path, "content").unwrap();
    let tool = tool();
    let args = serde_json::json!({ "path": path.display().to_string() });

    let result = tool.call(args.to_string()).await;

    assert!(result.is_err());
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_not_write_when_text_create_is_dry_run() {
    let path = crate::support::temp_path("modify-dry-run.txt");
    crate::support::remove_file(&path);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello world\n",
        "dry_run": true,
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains("would create"));
    assert!(!path.exists());
}

#[tokio::test]
async fn call_should_show_edit_diff_without_writing_when_dry_run() {
    let path = crate::support::temp_path("modify-dry-run-edit.txt");
    std::fs::write(&path, "before").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "before",
        "new_text": "after",
        "dry_run": true,
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains("-before"));
    assert!(output.contains("+after"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_report_deletion_when_new_text_is_omitted() {
    let path = crate::support::temp_path("modify-delete.txt");
    std::fs::write(&path, "keep remove keep").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "remove ",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("1 deletion"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep keep");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_report_mixed_replacements_and_deletions() {
    let path = crate::support::temp_path("modify-mixed.txt");
    std::fs::write(&path, "apple banana cherry").unwrap();
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "edits": [
            {"old_text": "apple", "new_text": "Apfel"},
            {"old_text": "cherry"},
        ],
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("1 replacement"));
    assert!(output.contains("1 deletion"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Apfel banana ");
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_create_binary_file_when_content_base64_is_provided() {
    use base64::Engine;

    let path = crate::support::temp_path("modify-base64-create.bin");
    crate::support::remove_file(&path);
    let data = b"\x00\x89PNG\r\n\x1a\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": b64,
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("created"));
    assert!(output.contains("(binary — no diff available)"));
    assert_eq!(std::fs::read(&path).unwrap(), data);
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_reject_content_base64_with_text_content() {
    let path = crate::support::temp_path("modify-base64-conflict.bin");
    crate::support::remove_file(&path);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": "AAAA",
        "content": "text",
    });

    let result = tool.call(args.to_string()).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("content_base64") || err.contains("ConflictingBase64"));
}

#[tokio::test]
async fn call_should_reject_invalid_content_base64() {
    let path = crate::support::temp_path("modify-base64-bad.bin");
    crate::support::remove_file(&path);
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": "not-valid!!!",
    });

    let result = tool.call(args.to_string()).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("base64"));
}

#[tokio::test]
async fn call_should_not_write_binary_file_when_dry_run() {
    use base64::Engine;

    let path = crate::support::temp_path("modify-base64-dry.bin");
    crate::support::remove_file(&path);
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"binary");
    let tool = tool();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": b64,
        "dry_run": true,
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains("would create"));
    assert!(!path.exists());
}

#[tokio::test]
async fn call_should_reject_editing_files_larger_than_max_file_size() {
    let dir = std::env::temp_dir();
    let path = crate::support::temp_path("modify-large-edit.txt");
    {
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_FILE_SIZE + 1).unwrap();
    }
    let tool = crate::support::inject_cwd(&dir, Modify);
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "nonexistent",
        "new_text": "replacement",
    });

    let result = tool.call(args.to_string()).await;

    assert!(result.is_err(), "editing a >100MB file should be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("exceeds the maximum") || err.contains("shell"),
        "error should suggest shell alternative, got: {err}"
    );
    crate::support::remove_file(&path);
}

#[tokio::test]
async fn call_should_skip_diff_when_overwriting_large_file() {
    let dir = std::env::temp_dir();
    let path = crate::support::temp_path("modify-large-write.txt");
    {
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(LARGE_FILE_THRESHOLD + 1).unwrap();
    }
    let tool = crate::support::inject_cwd(&dir, Modify);
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "new content after overwrite\n",
    });

    let output = tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("overwrote"));
    assert!(
        output.contains("diff skipped"),
        "large file overwrite should skip diff, got: {output}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "new content after overwrite\n"
    );
    crate::support::remove_file(&path);
}
