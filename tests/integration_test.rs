use std::process::Command;

fn inbox_bin() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("inbox");
    path.to_string_lossy().to_string()
}

#[test]
fn whoami_same_user() {
    // inbox "whoami" should run as the same user as the shell
    let expected = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| {
            String::from_utf8(Command::new("whoami").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        });

    let output = Command::new(inbox_bin())
        .arg("--")
        .arg("whoami")
        .output()
        .expect("failed to run inbox");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "inbox whoami exited non-zero: {}",
        output.status
    );
    assert!(
        stdout.trim() == expected.trim(),
        "expected user '{}', got '{}'",
        expected.trim(),
        stdout.trim()
    );
}

#[test]
#[cfg(target_os = "macos")]
fn ephemeral_restores_file() {
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("test.txt");
    fs::write(&file, "original").unwrap();

    let status = Command::new(inbox_bin())
        .arg("--ephemeral")
        .arg(dir.path())
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg(format!("echo modified > {}", file.display()))
        .status()
        .expect("failed to run inbox");

    assert!(status.success(), "inbox exited non-zero: {}", status);
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(
        content.trim(),
        "original",
        "file was not restored after ephemeral run"
    );
}
