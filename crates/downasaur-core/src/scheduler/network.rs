//! Metered-connection detection.
//!
//! Linux asks NetworkManager (`nmcli`); other platforms report `Unknown` until
//! their native probes land (Windows `NetworkCostType`, macOS
//! `NWPath.isExpensive`/`isConstrained`). `Unknown` never pauses downloads.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Metered {
    Yes,
    No,
    Unknown,
}

impl Metered {
    pub const fn should_pause(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// Parse `nmcli -t -f GENERAL.METERED dev show` output (`yes`, `yes (guessed)`, ...).
pub fn parse_nmcli(output: &str) -> Metered {
    let values: Vec<&str> = output.lines().filter_map(|l| l.split_once(':').map(|(_, v)| v.trim())).collect();
    if values.iter().any(|v| v.starts_with("yes")) {
        Metered::Yes
    } else if values.iter().any(|v| v.starts_with("no")) {
        Metered::No
    } else {
        Metered::Unknown
    }
}

pub async fn probe() -> Metered {
    if cfg!(target_os = "linux") {
        let out =
            tokio::process::Command::new("nmcli").args(["-t", "-f", "GENERAL.METERED", "dev", "show"]).output().await;
        match out {
            Ok(o) if o.status.success() => parse_nmcli(&String::from_utf8_lossy(&o.stdout)),
            _ => Metered::Unknown,
        }
    } else {
        Metered::Unknown
    }
}

/// Poll the network state and publish changes on a watch channel.
pub fn spawn_guard(interval: Duration, cancel: CancellationToken) -> watch::Receiver<Metered> {
    let (tx, rx) = watch::channel(Metered::Unknown);
    tokio::spawn(async move {
        loop {
            let state = probe().await;
            tx.send_if_modified(|cur| {
                let changed = *cur != state;
                *cur = state;
                changed
            });
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(interval) => {}
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nmcli() {
        assert_eq!(parse_nmcli("GENERAL.METERED:yes (guessed)\nGENERAL.METERED:unknown\n"), Metered::Yes);
        assert_eq!(parse_nmcli("GENERAL.METERED:no\n"), Metered::No);
        assert_eq!(parse_nmcli(""), Metered::Unknown);
    }
}
