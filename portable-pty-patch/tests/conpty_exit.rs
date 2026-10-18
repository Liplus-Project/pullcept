//! How a child that ends on its own is seen through ConPTY (pullcept #130).
//!
//! The pseudo console keeps the write end of the output pipe for as long as it
//! is open, whether or not anything is still attached to it. A reader on the
//! master therefore does not reach the end of its output when the child exits:
//! it reaches it when the pseudo console is closed, which is the master being
//! dropped. The app watches for an end with the child's own wait instead, and
//! drops the master once that returns. These tests hold both halves.
#![cfg(windows)]

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

fn size() -> PtySize {
    PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Read the master to its end on a thread, and say when it got there.
fn read_to_end(mut reader: Box<dyn Read + Send>) -> mpsc::Receiver<()> {
    let (done, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = done.send(());
    });
    ended
}

/// A child that exits by itself is reaped with its code, and the reader does
/// not reach the end while the master is held: the premise of #130.
#[test]
fn reader_does_not_end_on_its_own_when_the_child_exits() {
    let pair = NativePtySystem::default().openpty(size()).unwrap();
    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.args(["/C", "exit", "3"]);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let ended = read_to_end(pair.master.try_clone_reader().unwrap());

    let status = child.wait().unwrap();
    assert_eq!(status.exit_code(), 3);
    assert!(
        ended.recv_timeout(Duration::from_secs(3)).is_err(),
        "the reader reached the end while the pseudo console was still open"
    );

    drop(pair.master);
    ended
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader did not reach the end after the master was dropped");
}

/// A kill path ends the child through a cloned killer while another thread is
/// blocked in the child's `wait`, and that wait returns: the app holds the
/// child on its waiter thread and only the killer in its map.
#[test]
fn a_cloned_killer_ends_a_child_another_thread_waits_on() {
    let pair = NativePtySystem::default().openpty(size()).unwrap();
    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.args(["/C", "ping", "-n", "30", "127.0.0.1"]);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let ended = read_to_end(pair.master.try_clone_reader().unwrap());
    let mut killer = child.clone_killer();

    let (done, waited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(child.wait().map(|status| status.exit_code()).ok());
    });
    assert!(
        waited.recv_timeout(Duration::from_millis(500)).is_err(),
        "the child ended before it was killed"
    );

    killer.kill().unwrap();
    drop(pair.master);
    waited
        .recv_timeout(Duration::from_secs(10))
        .expect("the wait did not return after the kill");
    ended
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader did not reach the end after the kill");
}
