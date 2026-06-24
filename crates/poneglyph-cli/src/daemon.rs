//! OS-level service management for `poneglyph mcp` — launchd on macOS,
//! systemd --user on Linux. `enable` registers the service to start at login
//! and starts it now; `disable` reverses that (stop + unregister). `start`/
//! `stop` toggle an already-registered service without touching login
//! startup.
//!
//! ponytail: launchd + systemd only, no Windows/SysV init — add when someone
//! actually needs it on those platforms. Shells out to `launchctl`/
//! `systemctl` rather than a service-manager crate: both ship with the OS,
//! so there's nothing to add and nothing to get out of sync with the
//! platform's own behavior.

use anyhow::{bail, Context, Result};
use poneglyph_core::config::Config;
use std::process::Command;
use std::time::Duration;

const LABEL: &str = "com.poneglyph.daemon";
#[cfg(target_os = "linux")]
const SYSTEMD_UNIT: &str = "poneglyph.service";

fn current_exe() -> Result<String> {
    std::env::current_exe()
        .context("failed to resolve current executable path")
        .map(|p| p.display().to_string())
}

fn run(program: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{program} {}`", args.join(" ")))
}

/// TCP-reachability check — same approach as `detect::detect_local_llm`.
/// Good enough for "is something listening on this port", which is all a
/// liveness probe needs; avoids pulling in an HTTP client crate.
pub(crate) fn port_open(port: u16) -> bool {
    format!("127.0.0.1:{port}")
        .parse()
        .ok()
        .is_some_and(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

pub fn status(config: &Config) -> Result<()> {
    let registered = platform::is_registered()?;
    println!(
        "Service:  {}",
        if registered { "registered" } else { "not registered" }
    );
    let listening = port_open(config.agents.mcp_server_port);
    println!(
        "Liveness: {} (127.0.0.1:{})",
        if listening { "up" } else { "down" },
        config.agents.mcp_server_port
    );
    Ok(())
}

pub fn enable(config: &Config) -> Result<()> {
    let exe = current_exe()?;
    platform::enable(&exe)?;
    println!("Daemon enabled — `poneglyph mcp` starts at login and is running now.");
    if !port_open(config.agents.mcp_server_port) {
        println!("note: not yet listening on 127.0.0.1:{} — give it a moment, then check `poneglyph daemon status`.", config.agents.mcp_server_port);
    }
    Ok(())
}

pub fn disable() -> Result<()> {
    platform::disable()?;
    println!("Daemon disabled — stopped and removed from login startup.");
    Ok(())
}

pub fn start() -> Result<()> {
    platform::start()?;
    println!("Daemon started.");
    Ok(())
}

pub fn stop() -> Result<()> {
    platform::stop()?;
    println!("Daemon stopped.");
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn home() -> Result<std::path::PathBuf> {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .context("HOME not set")
    }

    fn plist_path() -> Result<std::path::PathBuf> {
        Ok(home()?.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    fn uid() -> Result<String> {
        let out = run("id", &["-u"])?;
        if !out.status.success() {
            bail!("`id -u` failed");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn domain_target() -> Result<String> {
        Ok(format!("gui/{}/{LABEL}", uid()?))
    }

    pub fn plist_contents(exe: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>mcp</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
"#
        )
    }

    pub fn is_registered() -> Result<bool> {
        Ok(plist_path()?.exists())
    }

    pub fn enable(exe: &str) -> Result<()> {
        let plist = plist_path()?;
        if let Some(dir) = plist.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        std::fs::write(&plist, plist_contents(exe))
            .with_context(|| format!("failed to write {}", plist.display()))?;

        // bootout first (ignore failure — fine if not currently loaded), then
        // bootstrap to (re)load with the fresh plist.
        let _ = run("launchctl", &["bootout", &domain_target()?]);
        let out = run("launchctl", &["bootstrap", &format!("gui/{}", uid()?), plist.to_str().unwrap()])?;
        if !out.status.success() {
            bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let _ = run("launchctl", &["bootout", &domain_target()?]);
        let plist = plist_path()?;
        if plist.exists() {
            std::fs::remove_file(&plist)
                .with_context(|| format!("failed to remove {}", plist.display()))?;
        }
        Ok(())
    }

    // `KeepAlive=true` means launchd relaunches the job the instant it exits
    // for *any* reason — `launchctl kill` only sends a signal to the running
    // instance and leaves the job loaded, so it bounces straight back. Both
    // start and stop instead load/unload the job itself (`bootstrap`/
    // `bootout`), which is what actually keeps KeepAlive from refighting us.
    pub fn start() -> Result<()> {
        if !is_registered()? {
            bail!("daemon not registered — run `poneglyph daemon enable` first");
        }
        let _ = run("launchctl", &["bootout", &domain_target()?]);
        let out = run("launchctl", &["bootstrap", &format!("gui/{}", uid()?), plist_path()?.to_str().unwrap()])?;
        if !out.status.success() {
            bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub fn stop() -> Result<()> {
        let out = run("launchctl", &["bootout", &domain_target()?])?;
        if !out.status.success() {
            bail!(
                "launchctl bootout failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    fn unit_path() -> Result<std::path::PathBuf> {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .context("HOME not set")?;
        Ok(home.join(".config/systemd/user").join(SYSTEMD_UNIT))
    }

    pub fn unit_contents(exe: &str) -> String {
        format!(
            "[Unit]\nDescription=Poneglyph MCP daemon\n\n[Service]\nExecStart={exe} mcp\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
        )
    }

    pub fn is_registered() -> Result<bool> {
        Ok(unit_path()?.exists())
    }

    pub fn enable(exe: &str) -> Result<()> {
        let unit = unit_path()?;
        if let Some(dir) = unit.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        std::fs::write(&unit, unit_contents(exe))
            .with_context(|| format!("failed to write {}", unit.display()))?;

        run("systemctl", &["--user", "daemon-reload"])?;
        let out = run("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
        if !out.status.success() {
            bail!(
                "systemctl enable failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let _ = run("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT]);
        let unit = unit_path()?;
        if unit.exists() {
            std::fs::remove_file(&unit)
                .with_context(|| format!("failed to remove {}", unit.display()))?;
        }
        run("systemctl", &["--user", "daemon-reload"])?;
        Ok(())
    }

    pub fn start() -> Result<()> {
        if !is_registered()? {
            bail!("daemon not registered — run `poneglyph daemon enable` first");
        }
        let out = run("systemctl", &["--user", "start", SYSTEMD_UNIT])?;
        if !out.status.success() {
            bail!(
                "systemctl start failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub fn stop() -> Result<()> {
        let out = run("systemctl", &["--user", "stop", SYSTEMD_UNIT])?;
        if !out.status.success() {
            bail!(
                "systemctl stop failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::*;

    pub fn is_registered() -> Result<bool> {
        Ok(false)
    }
    pub fn enable(_exe: &str) -> Result<()> {
        bail!("`poneglyph daemon` is only supported on macOS (launchd) and Linux (systemd --user)")
    }
    pub fn disable() -> Result<()> {
        bail!("`poneglyph daemon` is only supported on macOS (launchd) and Linux (systemd --user)")
    }
    pub fn start() -> Result<()> {
        bail!("`poneglyph daemon` is only supported on macOS (launchd) and Linux (systemd --user)")
    }
    pub fn stop() -> Result<()> {
        bail!("`poneglyph daemon` is only supported on macOS (launchd) and Linux (systemd --user)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn plist_embeds_exe_and_mcp_arg() {
        let xml = platform::plist_contents("/usr/local/bin/poneglyph");
        assert!(xml.contains("<string>/usr/local/bin/poneglyph</string>"));
        assert!(xml.contains("<string>mcp</string>"));
        assert!(xml.contains(LABEL));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unit_embeds_exe_and_mcp_arg() {
        let unit = platform::unit_contents("/usr/local/bin/poneglyph");
        assert!(unit.contains("ExecStart=/usr/local/bin/poneglyph mcp"));
        assert!(unit.contains("[Install]"));
    }
}
