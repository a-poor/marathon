//! Exercise the actual CLI, including its output and exit-status contracts.
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn exec(doc: &str, args: &[&str], answers: &str) -> Output {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("book.md");
    std::fs::write(&path, doc).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("exec")
        .arg(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answers.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn unattended_stdout_preserves_arbitrary_bytes() {
    let output = exec("```sh\nprintf 'x\\r\\ny\\377\\000'\n```", &["--yes"], "");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"x\r\ny\xff\0");
}

#[test]
fn nonzero_exit_stops_later_cells_and_preserves_status() {
    let output = exec(
        "```sh\nprintf first\nexit 7\n```\n\n```sh\nprintf second\n```",
        &["--yes"],
        "",
    );
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"first");
}

#[test]
fn spawn_error_only_goes_to_stderr() {
    let output = exec(
        "---\ninterpreters:\n  sh:\n    path: /no/such/marathon-interpreter\n---\n```sh\necho hi\n```",
        &["--yes"],
        "",
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("starting /no/such/marathon-interpreter"));
    assert!(!error.contains("signal"));
}

#[test]
fn interactive_exec_requires_confirmation_for_every_cell() {
    let doc = "```sh\nprintf first\n```\n\n```sh\nprintf second\n```";
    let output = exec(doc, &[], "yes\nno\n");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"first");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("Run this cell?")
            .count(),
        2
    );
    let output = exec(doc, &[], "yes\nyes\n");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"firstsecond");
}

#[test]
fn eof_does_not_authorize_execution() {
    let output = exec("```sh\nprintf should-not-run\n```", &[], "");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input ended"));
}

const INPUTS: &str = r#"
```json mrthn=input
{"type":"input","prompt":"Label?","target":"MRTHN_TEST_LABEL","default":"release"}
```
```json mrthn=input
{"type":"select","prompt":"Region?","target":"MRTHN_TEST_REGION","options":["east","west"],"default":"east"}
```
```json mrthn=input
{"type":"confirm","prompt":"Proceed?","target":"MRTHN_TEST_PROCEED","default":false}
```
```sh
printf '%s/%s/%s' "$MRTHN_TEST_LABEL" "$MRTHN_TEST_REGION" "$MRTHN_TEST_PROCEED"
```
"#;

#[test]
fn unattended_inputs_use_explicit_defaults_and_env_overrides() {
    let output = exec(INPUTS, &["--yes", "-e", "MRTHN_TEST_LABEL=custom"], "");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"custom/east/no");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Run this cell?"));
}

#[test]
fn interactive_inputs_validate_choices_and_feed_later_cells() {
    let output = exec(INPUTS, &[], "hello\n99\n2\nmaybe\nyes\nyes\n");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"hello/west/yes");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("option number from 1 to 2"));
    assert!(error.contains("expected yes or no"));
}

#[test]
fn missing_or_invalid_unattended_answers_stop_before_downstream_code() {
    let doc = "```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"Name?\",\"target\":\"MRTHN_TEST_MISSING\"}\n```\n```sh\necho should-not-run\n```";
    let output = exec(doc, &["--yes"], "");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("needs a value"));
    let output = exec(INPUTS, &["--yes", "-e", "MRTHN_TEST_REGION=invalid"], "");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn select_reads_options_generated_by_a_previous_cell() {
    let doc = r#"
```sh
printf 'east\nwest\n' > "$TMP_DIR/options"
```
```json mrthn=input
{"type":"select","prompt":"Region?","target":"MRTHN_TEST_REGION","option_file":"$TMP_DIR/options","default":"west"}
```
```sh
printf '%s' "$MRTHN_TEST_REGION"
```
"#;
    let output = exec(doc, &["--yes"], "");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"west");
}

#[test]
fn unlabeled_and_non_shell_blocks_are_display_only_without_frontmatter() {
    let output = exec(
        "```\necho wrong\n```\n\n```python\nprint('wrong')\n```\n\n```sh skip=true\necho wrong\n```\n\n```sh\nprintf right\n```",
        &["--yes"],
        "",
    );
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"right");
}

#[test]
fn temporary_directory_is_cleaned_on_success_and_failure() {
    for code in [0, 9] {
        let output = exec(
            &format!("```sh\nprintf '%s' \"$TMP_DIR\"\nexit {code}\n```"),
            &["--yes"],
            "",
        );
        let path = String::from_utf8(output.stdout).unwrap();
        assert!(!path.is_empty());
        assert!(!std::path::Path::new(&path).exists());
        assert_eq!(output.status.code(), Some(code));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sigint_cleans_up_the_command_and_temp_directory() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("book.md");
    let marker = dir.path().join("survived");
    let release = dir.path().join("release");
    std::fs::write(&path, "```sh\nprintf '%s\\n' \"$TMP_DIR\"\n(while [ ! -e \"$RELEASE\" ]; do sleep 0.05; done; touch \"$MARKER\") &\nwait\n```").unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("exec")
        .arg(path)
        .arg("--yes")
        .env("MARKER", &marker)
        .env("RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let tmp = tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGINT);
    }
    let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(130));
    assert!(!std::path::Path::new(&tmp).exists());
    std::fs::write(release, "go").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!marker.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_while_waiting_for_a_prompt_exits_without_waiting_for_stdin() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("book.md");
    std::fs::write(&path, "```sh\necho should-not-run\n```").unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("exec")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // Child::wait closes its stdin handle; retain it so this remains a real
    // blocked prompt rather than racing EOF against SIGINT.
    let _stdin = child.stdin.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut prompt = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !String::from_utf8_lossy(&prompt).contains("Run this cell?") {
            prompt.push(stderr.read_u8().await.unwrap());
        }
    })
    .await
    .unwrap();
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGINT);
    }
    let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(130));
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .await
        .unwrap();
    assert!(stdout.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn broken_stdout_pipe_cancels_the_child_and_cleans_scratch() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("book.md");
    let scratch_file = dir.path().join("scratch-path");
    std::fs::write(
        &path,
        "```sh\nprintf '%s' \"$TMP_DIR\" > \"$SCRATCH_FILE\"\nprintf ready\nsleep 10\n```",
    )
    .unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("exec")
        .arg(path)
        .arg("--yes")
        .env("SCRATCH_FILE", &scratch_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success());
    let scratch = std::fs::read_to_string(scratch_file).unwrap();
    assert!(!std::path::Path::new(&scratch).exists());
}

#[test]
fn shipped_local_samples_run_unattended() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // demo.md deliberately fails; shell-override and tmpdir need optional zsh/bc.
    for sample in ["hello.md", "interactive.md"] {
        let doc = std::fs::read_to_string(root.join("samples").join(sample)).unwrap();
        let output = exec(&doc, &["--yes"], "");
        assert!(output.status.success(), "{sample}: {:?}", output);
    }
}

#[test]
fn missing_option_file_is_reported_before_downstream_execution() {
    let output = exec(
        "```json mrthn=input\n{\"type\":\"select\",\"prompt\":\"Pick\",\"target\":\"MRTHN_TEST_REGION\",\"option_file\":\"$TMP_DIR/missing\",\"default\":\"west\"}\n```\n```sh\necho wrong\n```",
        &["--yes"],
        "",
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reading option file"));
}

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_with_a_full_stdout_pipe_still_cleans_up() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("book.md");
    let scratch_file = dir.path().join("scratch-path");
    std::fs::write(&path, "```sh\nprintf '%s' \"$TMP_DIR\" > \"$SCRATCH_FILE\"\nhead -c 10000000 /dev/zero\nsleep 10\n```").unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("exec")
        .arg(path)
        .arg("--yes")
        .env("SCRATCH_FILE", &scratch_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // Observe the first byte, then keep the read end open without draining it.
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        child.stdout.as_mut().unwrap().read_u8(),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGINT);
    }
    let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(130));
    let scratch = std::fs::read_to_string(scratch_file).unwrap();
    assert!(!std::path::Path::new(&scratch).exists());
}
