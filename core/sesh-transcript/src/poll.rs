//! poll(2), which std does not offer, for waiting on several descriptors at once.

use std::os::fd::RawFd;
use std::time::Duration;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(target_os = "linux")]
type Nfds = std::ffi::c_ulong;
#[cfg(not(target_os = "linux"))]
type Nfds = std::ffi::c_uint;

extern "C" {
    fn poll(fds: *mut PollFd, count: Nfds, timeout: i32) -> i32;
}

const POLLIN: i16 = 0x1;
const POLLOUT: i16 = 0x4;
const POLLERR: i16 = 0x8;
const POLLHUP: i16 = 0x10;

fn wait(descriptors: &mut [PollFd], timeout: Duration) -> bool {
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe { poll(descriptors.as_mut_ptr(), descriptors.len() as Nfds, millis) > 0 }
}

/// Waits up to `timeout` for any of `reads` to have something to read or to hang up, or any
/// of `writes` to take more.
pub fn ready(reads: &[RawFd], writes: &[RawFd], timeout: Duration) -> bool {
    let mut descriptors: Vec<PollFd> = reads.iter().map(|&fd| PollFd { fd, events: POLLIN, revents: 0 })
        .chain(writes.iter().map(|&fd| PollFd { fd, events: POLLOUT, revents: 0 }))
        .collect();
    wait(&mut descriptors, timeout)
}

/// Whether whoever reads `fd` has hung up.
pub fn hung_up(fd: RawFd) -> bool {
    let mut descriptor = [PollFd { fd, events: POLLOUT, revents: 0 }];
    wait(&mut descriptor, Duration::ZERO) && descriptor[0].revents & (POLLERR | POLLHUP) != 0
}
