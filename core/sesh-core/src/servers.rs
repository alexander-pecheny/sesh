//! The mosh-servers Sesh starts. Each carries the device's and the app run's tag, so a later
//! run stops what an earlier one left behind as soon as it reaches the Host; one whose device
//! never returns ends on its own a day after its client last spoke, as a pipe's server does.

use std::sync::OnceLock;

const TAG: &str = "SESH_TAG";
/// A terminal's server waits this long for its client to come back, as long as a pipe's does.
const IDLE_SECONDS: u32 = 24 * 3600;

static DEVICE: OnceLock<String> = OnceLock::new();

/// Names this device for the tags; the app calls it once, before any Host is reached.
pub fn set_device(id: &str) {
    let id: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    let _ = DEVICE.set(id);
}

/// This run of the app, the same length every time so no tag is a prefix of another.
fn run() -> &'static str {
    static RUN: OnceLock<String> = OnceLock::new();
    RUN.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        format!("{:016x}", hasher.finish())
    })
}

fn tag(device: &str) -> String {
    format!("sesh-{device}-{}", run())
}

/// What a pipe's server is told: its tag.
pub fn pipe_vars() -> Vec<(String, String)> {
    DEVICE.get().map(|device| vec![(TAG.to_string(), tag(device))]).unwrap_or_default()
}

/// What a terminal's server is told: its tag and when to give up on a client.
pub fn terminal_vars() -> Vec<(String, String)> {
    let mut vars = pipe_vars();
    vars.push(("MOSH_SERVER_NETWORK_TMOUT".into(), IDLE_SECONDS.to_string()));
    vars
}

/// Stops this device's servers from earlier runs; nothing before the device is named.
pub fn reap() -> Option<String> {
    let device = DEVICE.get()?;
    Some(format!(
        "ps -ww -u \"$(id -u)\" -o pid=,args= | while read -r pid args; do case \"$args\" in \
         *mosh-server*'{TAG}=sesh-{device}-'*) case \"$args\" in *'{TAG}={current}'*) ;; *) kill \"$pid\" ;; esac ;; esac; done",
        current = tag(device)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_run_stops_only_this_devices_earlier_servers() {
        set_device("Mac-1!");
        let current = tag("Mac1");
        let earlier = format!("sesh-Mac1-{}", "0".repeat(16));
        let script = format!(
            "ps() {{ printf '%s\\n' '11 mosh-server pipe -l {TAG}={earlier} -- sh -c x' '12 mosh-server new -l {TAG}={current} -- sh' \
             '13 mosh-server pipe -l {TAG}=sesh-Phone-{earlier} --' '14 vim notes'; }}; kill() {{ echo \"killed $1\"; }}; {}",
            reap().unwrap()
        );
        let output = std::process::Command::new("sh").args(["-c", &script]).output().unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "killed 11\n");
    }
}
