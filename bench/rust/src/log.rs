//! Event-log writer thread: appends JSON lines, group-commits with fdatasync every 50 ms.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

const SYNC_PERIOD: Duration = Duration::from_millis(50);

pub fn run(file: File, rx: Receiver<Vec<u8>>) {
    let mut w = BufWriter::with_capacity(1 << 16, file);
    let mut dirty = false;
    let mut next_sync = Instant::now() + SYNC_PERIOD;
    loop {
        match rx.recv_deadline(next_sync) {
            Ok(line) => {
                w.write_all(&line).expect("log write");
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = w.flush();
                let _ = w.get_ref().sync_data();
                return;
            }
        }
        let now = Instant::now();
        if now >= next_sync {
            if dirty {
                w.flush().expect("log flush");
                w.get_ref().sync_data().expect("fdatasync");
                dirty = false;
            }
            next_sync = now + SYNC_PERIOD;
        }
    }
}
