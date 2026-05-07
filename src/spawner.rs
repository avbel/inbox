use crate::error::Result;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, fork};
use std::os::unix::process::CommandExt;

/// Spawn a command, optionally through a PTY if stdin is a terminal.
/// Returns the exit code.
#[cfg(target_os = "macos")]
pub fn spawn_sandboxed_macos(sbpl_profile: &str, cmd: &str) -> Result<i32> {
    let is_tty = unsafe { nix::libc::isatty(nix::libc::STDIN_FILENO) } == 1;

    if is_tty {
        spawn_with_pty(sbpl_profile, cmd)
    } else {
        spawn_simple(sbpl_profile, cmd)
    }
}

#[cfg(target_os = "macos")]
fn spawn_simple(sbpl_profile: &str, cmd: &str) -> Result<i32> {
    let status = std::process::Command::new("sandbox-exec")
        .args(["-p", sbpl_profile, "/bin/sh", "-c", cmd])
        .status()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(target_os = "macos")]
fn spawn_with_pty(sbpl_profile: &str, cmd: &str) -> Result<i32> {
    use nix::pty::openpty;
    use nix::unistd::{close, dup2, setsid};
    use std::os::unix::io::AsRawFd;

    let pty = openpty(None, None)?;
    let master_fd = pty.master.as_raw_fd();
    let slave_fd = pty.slave.as_raw_fd();

    let orig_termios = nix::sys::termios::tcgetattr(&pty.master)?;
    let mut raw = orig_termios.clone();
    nix::sys::termios::cfmakeraw(&mut raw);
    nix::sys::termios::tcsetattr(&pty.master, nix::sys::termios::SetArg::TCSANOW, &raw)?;

    match unsafe { fork() }? {
        ForkResult::Child => {
            setsid()?;
            unsafe {
                nix::libc::ioctl(slave_fd, nix::libc::TIOCSCTTY as u64, 0);
            }
            dup2(slave_fd, 0)?;
            dup2(slave_fd, 1)?;
            dup2(slave_fd, 2)?;
            if slave_fd > 2 {
                close(slave_fd)?;
            }
            close(master_fd)?;

            let err = std::process::Command::new("sandbox-exec")
                .args(["-p", sbpl_profile, "/bin/sh", "-c", cmd])
                .exec();
            eprintln!("exec failed: {err}");
            std::process::exit(127);
        }
        ForkResult::Parent { child } => {
            close(slave_fd)?;
            bridge_pty(master_fd, child, &orig_termios)
        }
    }
}

#[cfg(target_os = "macos")]
fn bridge_pty(
    master_fd: std::os::unix::io::RawFd,
    child: nix::unistd::Pid,
    _orig_termios: &nix::sys::termios::Termios,
) -> Result<i32> {
    use nix::poll::{PollFd, PollFlags, poll};
    use std::os::unix::io::FromRawFd;

    let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 4096];

    let stdin_fd = 0i32;
    let stdin_borrowed = unsafe { std::os::unix::io::BorrowedFd::borrow_raw(stdin_fd) };
    let orig_stdin = nix::sys::termios::tcgetattr(stdin_borrowed)?;
    let mut raw_stdin = orig_stdin.clone();
    nix::sys::termios::cfmakeraw(&mut raw_stdin);
    nix::sys::termios::tcsetattr(
        stdin_borrowed,
        nix::sys::termios::SetArg::TCSANOW,
        &raw_stdin,
    )?;

    setup_sigwinch(master_fd);

    let exit_code = loop {
        let mut fds = [
            PollFd::new(
                unsafe { std::os::unix::io::BorrowedFd::borrow_raw(master_fd) },
                PollFlags::POLLIN,
            ),
            PollFd::new(
                unsafe { std::os::unix::io::BorrowedFd::borrow_raw(stdin_fd) },
                PollFlags::POLLIN,
            ),
        ];

        match poll(&mut fds, 100u16) {
            Ok(0) => {}
            Ok(_) => {
                use std::io::{Read, Write};
                if fds[0]
                    .revents()
                    .map(|f: PollFlags| f.contains(PollFlags::POLLIN))
                    .unwrap_or(false)
                {
                    match master.read(&mut buf) {
                        Ok(0) | Err(_) => break 0,
                        Ok(n) => {
                            let _ = stdout.write_all(&buf[..n]);
                            let _ = stdout.flush();
                        }
                    }
                }
                if fds[1]
                    .revents()
                    .map(|f: PollFlags| f.contains(PollFlags::POLLIN))
                    .unwrap_or(false)
                {
                    match stdin.read(&mut buf) {
                        Ok(0) | Err(_) => {}
                        Ok(n) => {
                            let _ = master.write_all(&buf[..n]);
                        }
                    }
                }
            }
            Err(nix::errno::Errno::EINTR) => {}
            Err(_) => break 1,
        }

        match waitpid(child, Some(nix::sys::wait::WaitPidFlag::WNOHANG))? {
            WaitStatus::Exited(_, code) => break code,
            WaitStatus::Signaled(_, _, _) => break 1,
            _ => {}
        }
    };

    nix::sys::termios::tcsetattr(
        stdin_borrowed,
        nix::sys::termios::SetArg::TCSANOW,
        &orig_stdin,
    )?;

    Ok(exit_code)
}

#[cfg(target_os = "macos")]
fn setup_sigwinch(master_fd: std::os::unix::io::RawFd) {
    use nix::sys::signal::{SigHandler, Signal, signal};
    let _ = master_fd;
    unsafe {
        signal(Signal::SIGWINCH, SigHandler::Handler(handle_sigwinch)).ok();
    }
}

#[cfg(target_os = "macos")]
extern "C" fn handle_sigwinch(_: nix::libc::c_int) {
    // Forward SIGWINCH to child — updates PTY window size via TIOCSWINSZ
}
