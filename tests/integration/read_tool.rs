use std::path::{Path, PathBuf};

use togi::tools::read::Read;

const LARGE_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

fn make_tool(cwd: impl AsRef<Path>) -> crate::support::TestTool {
    crate::support::inject_cwd(cwd, Read)
}

fn write_sparse_large_text_file(path: &Path) {
    use std::io::Write;

    let mut file = std::fs::File::create(path).unwrap();
    for i in 0..2_000 {
        writeln!(
            file,
            "line {i:06}: some text content for large file testing"
        )
        .unwrap();
    }
    file.set_len(LARGE_FILE_THRESHOLD + 1).unwrap();
}

#[tokio::test]
async fn read_receives_injected_cwd() {
    let tool = make_tool(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let output = tool
        .call(r#"{"path":"Cargo.toml"}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("name = \"togi\""));
}

#[tokio::test]
async fn read_schema_does_not_expose_cwd_argument() {
    let tool = make_tool(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let definition = tool.definition();
    let properties = definition.parameters["properties"].as_object().unwrap();
    assert!(properties.contains_key("path"));
    assert!(!properties.contains_key("cwd"));
}

#[tokio::test]
async fn read_schema_exposes_encoding() {
    let tool = make_tool(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let definition = tool.definition();
    let properties = definition.parameters["properties"].as_object().unwrap();
    assert!(properties.contains_key("encoding"));
}

#[tokio::test]
async fn read_schema_exposes_offset_and_limit_bytes() {
    let tool = make_tool(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let definition = tool.definition();
    let properties = definition.parameters["properties"].as_object().unwrap();
    assert!(properties.contains_key("offset_bytes"));
    assert!(properties.contains_key("limit_bytes"));
}

#[tokio::test]
async fn read_binary_returns_hexdump() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("bin.bin");
    std::fs::write(&path, b"\x00\x01\x02Hello PNG\x89PNG").unwrap();

    let args = serde_json::json!({"path": path.display().to_string()});
    let output = ctx.tool.call(args.to_string()).await.unwrap();
    assert!(output.contains(&path.display().to_string()));
    assert!(output.contains("00000000"));
    assert!(
        !output.contains("1 | "),
        "text line numbers should not appear"
    );
}

#[tokio::test]
async fn read_binary_hex_respects_offset() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("bin-offset.bin");
    std::fs::write(&path, b"\x00\x01ABCDEFG").unwrap();

    let args = serde_json::json!({"path": path.display().to_string(), "offset_bytes": 2});
    let output = ctx.tool.call(args.to_string()).await.unwrap();
    assert!(output.contains(&togi::t!("read-binary-from-byte", offset = 2)));
    assert!(output.contains("00000002"));
    assert!(
        output.contains("41 42 43"),
        "expected hex from offset, got: {output}"
    );
}

#[tokio::test]
async fn read_binary_base64_encoding() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("b64.bin");
    std::fs::write(&path, b"\x00binary").unwrap();

    let args = serde_json::json!({"path": path.display().to_string(), "encoding": "base64"});
    let output = ctx.tool.call(args.to_string()).await.unwrap();
    assert!(output.contains(&path.display().to_string()));
    assert!(output.contains("base64"));
    assert!(output.contains("AGJpbmFyeQ=="));
}

#[tokio::test]
async fn read_utf16le_bom_text_file() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("utf16le.txt");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "hello 世界\n".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();

    let args = serde_json::json!({"path": path.display().to_string()});
    let output = ctx.tool.call(args.to_string()).await.unwrap();
    assert!(output.contains("1 | hello 世界"), "got: {output}");
}

#[tokio::test]
async fn read_gbk_text_file_with_explicit_encoding() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("gbk.txt");
    let (bytes, _, had_errors) = encoding_rs::GBK.encode("中文\n");
    assert!(!had_errors);
    std::fs::write(&path, &bytes).unwrap();

    let args = serde_json::json!({"path": path.display().to_string(), "encoding": "gbk"});
    let output = ctx.tool.call(args.to_string()).await.unwrap();
    assert!(output.contains("1 | 中文"), "got: {output}");
}

#[tokio::test]
async fn read_rejects_invalid_encoding() {
    let ctx = crate::support::TestContext::new(Read);
    let path = ctx.join("badenc.bin");
    std::fs::write(&path, b"\x00\xff").unwrap();

    let args = serde_json::json!({"path": path.display().to_string(), "encoding": "gzip"});
    let result = ctx.tool.call(args.to_string()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("encoding"));
}

#[tokio::test]
async fn read_large_file_uses_streaming_and_shows_truncation_notice() {
    let dir = std::env::temp_dir();
    let path = crate::support::temp_path("read-large.txt");
    write_sparse_large_text_file(&path);

    let tool = make_tool(&dir);
    let args = serde_json::json!({"path": path.display().to_string()});
    let output = tool.call(args.to_string()).await.unwrap();

    assert!(
        output.contains(&path.display().to_string()),
        "large file output should show the header, got: {output}"
    );
    assert!(
        output.contains("1 | "),
        "text file should render with line numbers, got: {output}"
    );

    crate::support::remove_file(&path);
}

#[tokio::test]
async fn read_large_file_offset_bytes() {
    let dir = std::env::temp_dir();
    let path = crate::support::temp_path("read-offset.txt");
    write_sparse_large_text_file(&path);

    let tool = make_tool(&dir);
    let args = serde_json::json!({
        "path": path.display().to_string(),
        "offset_bytes": 1_000_000,
        "limit_bytes": 4096,
    });
    let output = tool.call(args.to_string()).await.unwrap();

    assert!(
        output.contains("1000000"),
        "offset_bytes not reflected in output: {output}"
    );

    let args = serde_json::json!({
        "path": path.display().to_string(),
        "offset_bytes": 999999999999u64,
        "limit_bytes": 4096,
    });
    let output = tool.call(args.to_string()).await.unwrap();
    assert!(
        output.contains("999999999999") && output.contains(&togi::t!("read-empty-file")),
        "offset past EOF should be handled without underflow: {output}"
    );

    crate::support::remove_file(&path);
}
