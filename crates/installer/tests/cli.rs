use std::process::{Command, Stdio};

#[test]
fn help_and_invalid_arguments_do_not_start_installation() {
    let binary = env!("CARGO_BIN_EXE_hype-install");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--source REPOSITORY"));
    let invalid = Command::new(binary).arg("--invalid").output().unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Unknown argument"));
}

#[test]
fn unattended_installation_fails_before_any_phase() {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hype-install"));
    // Detach from the test runner's controlling terminal without changing its session.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.stdin(Stdio::null()).output().unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Phase "));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("terminal")
            || String::from_utf8_lossy(&output.stderr).contains("regular user")
    );
}
