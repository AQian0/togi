use togi::tools::shell::Shell;

fn tool() -> Box<dyn rig::tool::ToolDyn> {
    crate::support::inject_cwd(std::env::temp_dir(), Shell)
}

#[tokio::test]
async fn shell_runs_command_and_captures_stdout() {
    let output = tool()
        .call(r#"{"command":"echo hello"}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("exit code: 0"));
    assert!(output.contains("hello"));
    assert!(output.contains("cwd:"), "should display cwd");
}

#[tokio::test]
async fn shell_reports_nonzero_exit_code() {
    let output = tool()
        .call(r#"{"command":"exit 3"}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("exit code: 3"));
}

#[tokio::test]
async fn shell_captures_stderr() {
    let output = tool()
        .call(r#"{"command":"echo oops 1>&2"}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("--- stderr ---"));
    assert!(output.contains("oops"));
}

#[tokio::test]
async fn shell_rejects_empty_command() {
    let result = tool().call(r#"{"command":"   "}"#.to_string()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn shell_times_out_long_commands() {
    let result = tool()
        .call(r#"{"command":"sleep 5","timeout_secs":1}"#.to_string())
        .await;
    assert!(result.is_err());
    let message = result.unwrap_err().to_string();
    assert!(message.contains("did not finish"));
}

#[tokio::test]
async fn shell_schema_hides_injected_params() {
    let definition = tool().definition(String::new()).await;
    let properties = definition.parameters["properties"].as_object().unwrap();
    assert!(properties.contains_key("command"));
    assert!(properties.contains_key("timeout_secs"));
    assert!(properties.contains_key("interleave"));
    assert!(!properties.contains_key("cwd"));
    assert!(!properties.contains_key("env"));
}

#[tokio::test]
async fn shell_injects_env_variables() {
    let output = tool()
        .call(
            r#"{"command":"echo $TOGI_TEST_VAR","env":{"TOGI_TEST_VAR":"injected_value"}}"#
                .to_string(),
        )
        .await
        .unwrap();
    assert!(output.contains("injected_value"));
}

#[tokio::test]
async fn shell_interleaved_mode() {
    let output = tool()
        .call(r#"{"command":"echo out1; echo err1 1>&2; echo out2","interleave":true}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("out1"));
    assert!(output.contains("err1"));
    assert!(output.contains("out2"));
    assert!(output.contains("--- stdout ---"));
    assert!(output.contains("--- stderr ---"));
}

#[tokio::test]
async fn shell_interleaved_no_output() {
    let output = tool()
        .call(r#"{"command":"true","interleave":true}"#.to_string())
        .await
        .unwrap();
    assert!(output.contains("(no output)"));
}

#[tokio::test]
async fn shell_signal_info_on_kill() {
    let result = tool()
        .call(r#"{"command":"sleep 3","timeout_secs":1}"#.to_string())
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("did not finish"));
}

#[tokio::test]
async fn shell_truncates_large_interleaved_output() {
    let output = tool()
        .call(
            r#"{"command":"yes 'test line' | head -c 300000","interleave":true,"timeout_secs":10}"#
                .to_string(),
        )
        .await
        .unwrap();
    assert!(
        output.contains("truncated"),
        "large interleaved output should be truncated, got: {output}"
    );
}

#[tokio::test]
async fn shell_truncates_large_separated_output() {
    let output = tool()
        .call(r#"{"command":"yes 'test line' | head -c 300000","timeout_secs":10}"#.to_string())
        .await
        .unwrap();
    assert!(
        output.contains("truncated"),
        "large separated output should be truncated, got: {output}"
    );
    assert!(output.len() < 270_000, "output was not bounded");
}
