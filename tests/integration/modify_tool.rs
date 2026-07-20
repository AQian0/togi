use togi::tools::modify::{Modify, ModifyArgs};

const LARGE_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

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
    assert!(properties.contains_key("encoding"));
    assert!(properties.contains_key("dry_run"));
    assert!(!properties.contains_key("cwd"));
}

#[tokio::test]
async fn call_should_create_text_file_when_content_is_provided() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("create.txt");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello world\n",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-created")));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world\n");
}

#[tokio::test]
async fn call_should_overwrite_existing_text_file() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("overwrite.txt");
    std::fs::write(&path, "old").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "new",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-overwrote")));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
}

#[tokio::test]
async fn call_should_report_no_changes_when_overwriting_with_same_content() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("same.txt");
    std::fs::write(&path, "same\n").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "same\n",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-overwrote")));
    assert!(output.contains(&togi::t!("common-no-changes")));
    assert!(!output.contains("@@"));
}

#[tokio::test]
async fn call_should_include_diff_when_editing_text_file() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("diff-edit.txt");
    std::fs::write(&path, "a\nb\nc\n").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "b",
        "new_text": "B",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 1)));
    assert!(output.contains("@@"));
    assert!(output.contains("-b\n"));
    assert!(output.contains("+B\n"));
}

#[tokio::test]
async fn call_should_include_diff_when_creating_text_file() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("diff-create.txt");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello\nworld\n",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-created")));
    assert!(output.contains("--- /dev/null"));
    assert!(output.contains("@@ -0,0 +1,2 @@"));
    assert!(output.contains("+hello\n"));
    assert!(output.contains("+world\n"));
}

#[tokio::test]
async fn call_should_apply_single_unique_replacement() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("edit.txt");
    std::fs::write(&path, "alpha beta gamma").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "beta",
        "new_text": "BETA",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 1)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha BETA gamma");
}

#[tokio::test]
async fn call_should_combine_edits_array_with_single_old_text() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("combine.txt");
    std::fs::write(&path, "one two three").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "edits": [{"old_text": "one", "new_text": "1"}],
        "old_text": "three",
        "new_text": "3",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 2)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "1 two 3");
}

#[tokio::test]
async fn call_should_report_deletion_when_new_text_is_omitted() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("delete.txt");
    std::fs::write(&path, "keep remove keep").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "remove ",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-deletions", count = 1)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep keep");
}

#[tokio::test]
async fn call_should_report_mixed_replacements_and_deletions() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("mixed.txt");
    std::fs::write(&path, "apple banana cherry").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "edits": [
            {"old_text": "apple", "new_text": "Apfel"},
            {"old_text": "cherry"},
        ],
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 1)));
    assert!(output.contains(&togi::t!("modify-deletions", count = 1)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Apfel banana ");
}

#[tokio::test]
async fn call_should_edit_utf16le_bom_file_preserving_encoding() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("utf16le.txt");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "hello 世界".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "世界",
        "new_text": "Rust",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 1)));
    let written = std::fs::read(&path).unwrap();
    assert_eq!(&written[..2], &[0xFF, 0xFE]);
    let (decoded, had_errors) = encoding_rs::UTF_16LE.decode_without_bom_handling(&written[2..]);
    assert!(!had_errors);
    assert_eq!(decoded, "hello Rust");
}

#[tokio::test]
async fn call_should_edit_gbk_file_with_explicit_encoding() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("gbk.txt");
    let (bytes, _, had_errors) = encoding_rs::GBK.encode("中文 beta");
    assert!(!had_errors);
    std::fs::write(&path, &bytes).unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "beta",
        "new_text": "版本",
        "encoding": "gbk",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-replacements", count = 1)));
    let written = std::fs::read(&path).unwrap();
    let (decoded, _, had_errors) = encoding_rs::GBK.decode(&written);
    assert!(!had_errors);
    assert_eq!(decoded, "中文 版本");
}

#[tokio::test]
async fn call_should_write_text_file_with_explicit_encoding() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("write-gbk.txt");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "中文",
        "encoding": "gbk",
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-created")));
    let written = std::fs::read(&path).unwrap();
    let (decoded, _, had_errors) = encoding_rs::GBK.decode(&written);
    assert!(!had_errors);
    assert_eq!(decoded, "中文");
}

#[tokio::test]
async fn call_should_reject_conflicting_content_and_edit_instructions() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("conflict.txt");
    std::fs::write(&path, "data").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "whole new body",
        "old_text": "data",
    });

    let result = ctx.tool.call(args.to_string()).await;

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
}

#[tokio::test]
async fn call_should_require_write_or_edit_instructions() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("noop.txt");
    std::fs::write(&path, "content").unwrap();
    let args = serde_json::json!({ "path": path.display().to_string() });

    let result = ctx.tool.call(args.to_string()).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn call_should_not_write_when_text_create_is_dry_run() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("dry-run.txt");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content": "hello world\n",
        "dry_run": true,
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains(&togi::t!("modify-action-create")));
    assert!(!path.exists());
}

#[tokio::test]
async fn call_should_show_edit_diff_without_writing_when_dry_run() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("dry-run-edit.txt");
    std::fs::write(&path, "before").unwrap();
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "old_text": "before",
        "new_text": "after",
        "dry_run": true,
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains("-before"));
    assert!(output.contains("+after"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
}

#[tokio::test]
async fn call_should_create_binary_file_when_content_base64_is_provided() {
    use base64::Engine;

    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("base64-create.bin");
    let data = b"\x00\x89PNG\r\n\x1a\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": b64,
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains(&togi::t!("modify-action-created")));
    assert!(output.contains(&togi::t!("modify-binary-no-diff")));
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[tokio::test]
async fn call_should_reject_content_base64_with_text_content() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("base64-conflict.bin");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": "AAAA",
        "content": "text",
    });

    let result = ctx.tool.call(args.to_string()).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("content_base64") || err.contains("ConflictingBase64"));
}

#[tokio::test]
async fn call_should_reject_invalid_content_base64() {
    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("base64-bad.bin");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": "not-valid!!!",
    });

    let result = ctx.tool.call(args.to_string()).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("base64"));
}

#[tokio::test]
async fn call_should_not_write_binary_file_when_dry_run() {
    use base64::Engine;

    let ctx = crate::support::TestContext::new(Modify);
    let path = ctx.join("base64-dry.bin");
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"binary");
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "content_base64": b64,
        "dry_run": true,
    });

    let output = ctx.tool.call(args.to_string()).await.unwrap();

    assert!(output.contains("[dry run]"));
    assert!(output.contains(&togi::t!("modify-action-create")));
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

    assert!(output.contains(&togi::t!("modify-action-overwrote")));
    assert!(
        output.contains("diff"),
        "large file overwrite should skip diff, got: {output}"
    );
    assert!(
        !output.contains("@@"),
        "large file overwrite should not produce a diff, got: {output}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "new content after overwrite\n"
    );
    crate::support::remove_file(&path);
}
