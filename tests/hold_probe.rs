//! Integration probes for motion keys through a real pty:
//!
//! * `press_without_release_is_a_finite_step` — the pty answers the start-up
//!   capability query with only primary device attributes (a legacy terminal),
//!   so the app only receives Press events. A press must be one finite step,
//!   never a hold that drifts on after the terminal goes quiet.
//! * `probed_key_up_first_press_is_continuous` — the pty answers with kitty
//!   flags (a key-up terminal); the very first Press of the session must hold
//!   continuously without waiting for a Release or an auto-repeat event.
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Answer the start-up keyboard-capability query. `kitty` sends the flags
/// response plus primary device attributes (a key-up terminal); otherwise only
/// primary device attributes (a legacy terminal), so the probe does not wait
/// for its two-second timeout.
fn answer_keyboard_query(master: &mut std::fs::File, buf: &mut [u8], kitty: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    loop {
        assert!(
            Instant::now() < deadline,
            "app never queried keyboard support"
        );
        match master.read(buf) {
            Ok(0) => {}
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\x1b[?u") {
                    let reply: &[u8] = if kitty {
                        b"\x1b[?15u\x1b[?1;2c"
                    } else {
                        b"\x1b[?1;2c"
                    };
                    master.write_all(reply).expect("write capability reply");
                    return;
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return,
        }
    }
}

/// Drain `master` until `window` passes without a single byte (the app is
/// quiet) or `deadline` expires. Returns the bytes read and whether the app
/// went quiet. A WouldBlock read continues the window instead of ending it,
/// so the gap between two frames is never mistaken for idleness.
fn drain_until_quiet(
    master: &mut std::fs::File,
    buf: &mut [u8],
    deadline: Instant,
    window: Duration,
) -> (usize, bool) {
    let mut total = 0;
    let mut quiet_since: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return (total, false);
        }
        let got_byte = match master.read(buf) {
            Ok(0) => false,
            Ok(n) => {
                total += n;
                true
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => false,
            // The pty is gone: nothing else can arrive.
            Err(_) => return (total, true),
        };
        if got_byte {
            quiet_since = None;
        } else {
            if now.duration_since(*quiet_since.get_or_insert(now)) >= window {
                return (total, true);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Spawn the binary on a fresh 30x100 pty; returns the child and the master
/// side. The child owns the pts for stdin/stdout/stderr.
fn spawn_on_pty(bin: &str) -> (std::process::Child, std::fs::File) {
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ptmx")
        .expect("open ptmx");
    unsafe {
        assert_eq!(libc::unlockpt(slave.as_raw_fd()), 0, "unlockpt");
    }
    let ptsname = unsafe {
        let p = libc::ptsname(slave.as_raw_fd());
        assert!(!p.is_null());
        std::ffi::CStr::from_ptr(p).to_str().unwrap().to_string()
    };
    let master = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&ptsname)
        .expect("open pts");
    let ws = libc::winsize {
        ws_row: 30,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        assert_eq!(libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &ws), 0);
    }
    let slave_fd = slave.as_raw_fd();
    let child = unsafe {
        Command::new(bin)
            .arg("wrfm_files/cube.wrfm")
            .pre_exec(move || {
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                libc::setsid();
                Ok(())
            })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn")
    };
    drop(slave);
    (child, master)
}

fn set_nonblock(fd: i32, on: bool) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        let flags = if on {
            flags | libc::O_NONBLOCK
        } else {
            flags & !libc::O_NONBLOCK
        };
        libc::fcntl(fd, libc::F_SETFL, flags);
    }
}

#[test]
fn press_without_release_is_a_finite_step() {
    let bin = env!("CARGO_BIN_EXE_wireforge");
    let (mut child, mut master) = spawn_on_pty(bin);
    set_nonblock(master.as_raw_fd(), true);

    let mut buf = [0u8; 8192];
    answer_keyboard_query(&mut master, &mut buf, false);
    // The initial frame must finish before the key press.
    let (_, quiet) = drain_until_quiet(
        &mut master,
        &mut buf,
        Instant::now() + Duration::from_secs(5),
        Duration::from_millis(400),
    );
    assert!(quiet, "app never went quiet before the key press");

    // One press, no key-up: the pty can never report a Release.
    master.write_all(b"h").expect("write key press");

    // A press without key-up reporting is one finite step, so the output
    // settles again before the deadline. A hold that leaked out of a legacy
    // press would keep repainting, and `quiet` would never turn true.
    let (motion, quiet) = drain_until_quiet(
        &mut master,
        &mut buf,
        Instant::now() + Duration::from_secs(5),
        Duration::from_millis(400),
    );
    set_nonblock(master.as_raw_fd(), false);
    let _ = child.kill();
    let _ = child.wait();
    eprintln!("legacy press probe: repaint bytes after the press = {motion}, quiet = {quiet}");
    assert!(
        motion > 0,
        "the press must move the model at least once (input handling broken)"
    );
    assert!(
        quiet,
        "a press with no Release must be one finite step (stuck motion / 100% CPU)"
    );
}

#[test]
fn probed_key_up_first_press_is_continuous() {
    // The pty answers the start-up probe with key-up support, then the very
    // first Press of the session arrives without any Repeat. It must hold
    // continuously until Release; this is the cold-start regression probe.
    let bin = env!("CARGO_BIN_EXE_wireforge");
    let (mut child, mut master) = spawn_on_pty(bin);
    set_nonblock(master.as_raw_fd(), true);

    let mut buf = [0u8; 8192];
    answer_keyboard_query(&mut master, &mut buf, true);
    let (_, quiet) = drain_until_quiet(
        &mut master,
        &mut buf,
        Instant::now() + Duration::from_secs(5),
        Duration::from_millis(400),
    );
    assert!(quiet, "app never went quiet before the key press");

    // Kitty Press, no Release, no auto-repeat.
    master.write_all(b"\x1b[104;1u").expect("write kitty press");
    let (motion, quiet) = drain_until_quiet(
        &mut master,
        &mut buf,
        Instant::now() + Duration::from_secs(2),
        Duration::from_millis(400),
    );
    assert!(
        !quiet,
        "a probed key-up terminal must hold continuously without a Release"
    );
    assert!(motion > 0, "the first press must repaint the model");

    master
        .write_all(b"\x1b[104;1:3u")
        .expect("write kitty release");
    let (_, quiet) = drain_until_quiet(
        &mut master,
        &mut buf,
        Instant::now() + Duration::from_secs(2),
        Duration::from_millis(400),
    );
    set_nonblock(master.as_raw_fd(), false);
    let _ = child.kill();
    let _ = child.wait();
    eprintln!("kitty first-press probe: motion bytes = {motion}, stopped = {quiet}");
    assert!(quiet, "the Release must stop the first-press hold");
}
