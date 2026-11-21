//! Linux backend: a monitor interface driven through `iw` and a raw packet socket.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::Command;
use std::time::Duration;

use super::{Radio, Received};
use crate::wifi::frame::{BROADCAST, Mac};
use crate::wifi::radiotap;

const PACKET_OUTGOING: u8 = 4;
/// Rate for injected frames, 2 Mbit/s in radiotap's 500 kbit/s units.
const TX_RATE: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub monitor: bool,
    /// The card acknowledges unicast frames sent to a monitor interface.
    pub active_monitor: bool,
}

fn run(program: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!("{program}: {e}")))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(format!(
            "{program} {}: {msg}",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn phy_of(interface: &str) -> io::Result<u32> {
    let info = run("iw", &["dev", interface, "info"])?;
    info.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("wiphy ")
                .and_then(|n| n.trim().parse().ok())
        })
        .ok_or_else(|| io::Error::other(format!("{interface}: no wiphy in iw output")))
}

pub fn capabilities(interface: &str) -> io::Result<Capabilities> {
    let phy = format!("phy{}", phy_of(interface)?);
    let info = run("iw", &["phy", &phy, "info"])?;
    let mut in_modes = false;
    let mut monitor = false;
    for line in info.lines() {
        let t = line.trim();
        if t.starts_with("Supported interface modes") {
            in_modes = true;
        } else if in_modes && t.starts_with('*') {
            monitor |= t == "* monitor";
        } else {
            in_modes = false;
        }
    }
    Ok(Capabilities {
        monitor,
        active_monitor: info.contains("active monitor"),
    })
}

fn is_up(interface: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{interface}/flags"))
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .is_some_and(|f| f & libc::IFF_UP as u32 != 0)
}

fn nm_managed(interface: &str) -> bool {
    run("nmcli", &["-t", "-f", "DEVICE,STATE", "device"])
        .map(|out| {
            out.lines().any(|l| {
                l.split_once(':')
                    .is_some_and(|(d, s)| d == interface && s != "unmanaged")
            })
        })
        .unwrap_or(false)
}

fn tuner_name(monitor: &str) -> String {
    format!("{monitor}t")
}

fn read_mac(interface: &str) -> io::Result<Mac> {
    let s = std::fs::read_to_string(format!("/sys/class/net/{interface}/address"))?;
    let bytes: Vec<u8> = s
        .trim()
        .split(':')
        .filter_map(|b| u8::from_str_radix(b, 16).ok())
        .collect();
    bytes
        .try_into()
        .map_err(|_| io::Error::other(format!("{interface}: unreadable MAC address")))
}

/// Puts the original interface back the way it was found.
struct Restore {
    interface: String,
    monitor: String,
    was_up: bool,
    was_managed: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = run("iw", &["dev", &self.monitor, "del"]);
        let _ = run("iw", &["dev", &tuner_name(&self.monitor), "del"]);
        if self.was_up {
            let _ = run("ip", &["link", "set", &self.interface, "up"]);
        }
        if self.was_managed {
            let _ = run(
                "nmcli",
                &["device", "set", &self.interface, "managed", "yes"],
            );
        }
    }
}

pub struct LinuxRadio {
    fd: OwnedFd,
    monitor: String,
    mac: Mac,
    pub capabilities: Capabilities,
    _restore: Restore,
}

impl LinuxRadio {
    /// Takes over the card behind `interface` until dropped, refusing a connected one unless forced.
    pub fn open(interface: &str, force: bool) -> io::Result<Self> {
        if !force && !run("iw", &["dev", interface, "link"])?.starts_with("Not connected") {
            return Err(io::Error::other(format!(
                "{interface} is connected to a network; use another card or force it"
            )));
        }
        let caps = capabilities(interface)?;
        if !caps.monitor {
            return Err(io::Error::other(format!(
                "{interface} does not support monitor mode"
            )));
        }
        let phy = phy_of(interface)?;
        let monitor = format!("miidrop{phy}");
        let _ = run("iw", &["dev", &monitor, "del"]);
        let _ = run("iw", &["dev", &tuner_name(&monitor), "del"]);

        let was_up = is_up(interface);
        let was_managed = nm_managed(interface);
        let restore = Restore {
            interface: interface.into(),
            monitor: monitor.clone(),
            was_up,
            was_managed,
        };
        if was_managed {
            run("nmcli", &["device", "set", interface, "managed", "no"])?;
        }
        let phy_name = format!("phy{phy}");
        let mut args = vec![
            "phy",
            &phy_name,
            "interface",
            "add",
            &monitor,
            "type",
            "monitor",
        ];
        if caps.active_monitor {
            args.extend(["flags", "active"]);
        }
        run("iw", &args)?;
        run("ip", &["link", "set", &monitor, "down"])?;
        // Full-size stream frames exceed the default MTU once headers are added.
        run("ip", &["link", "set", &monitor, "mtu", "2000"])?;
        run("ip", &["link", "set", interface, "down"])?;
        // An active monitor gets no channel of its own, so a plain one must be up first to hold it.
        let tuner = tuner_name(&monitor);
        run(
            "iw",
            &[
                "phy",
                &phy_name,
                "interface",
                "add",
                &tuner,
                "type",
                "monitor",
            ],
        )?;
        run("ip", &["link", "set", &tuner, "up"])?;
        run("ip", &["link", "set", &monitor, "up"])?;

        let fd = open_socket(&monitor)?;
        let mac = read_mac(&monitor)?;
        Ok(LinuxRadio {
            fd,
            monitor,
            mac,
            capabilities: caps,
            _restore: restore,
        })
    }
}

fn open_socket(interface: &str) -> io::Result<OwnedFd> {
    let name = CString::new(interface).map_err(io::Error::other)?;
    let ifindex = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if ifindex == 0 {
        return Err(io::Error::last_os_error());
    }
    let proto = (libc::ETH_P_ALL as u16).to_be();
    let raw = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, proto as i32) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
    addr.sll_family = libc::AF_PACKET as u16;
    addr.sll_protocol = proto;
    addr.sll_ifindex = ifindex as i32;
    let rc = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const libc::sockaddr_ll as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_ll>() as u32,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

impl Radio for LinuxRadio {
    fn mac(&self) -> Mac {
        self.mac
    }

    fn set_channel(&mut self, channel: u8) -> io::Result<()> {
        run(
            "iw",
            &["dev", &self.monitor, "set", "channel", &channel.to_string()],
        )
        .map(|_| ())
    }

    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let group = frame.len() >= 10 && frame[4..10] == BROADCAST;
        let packet = radiotap::build(frame, TX_RATE, group);
        let n = unsafe { libc::send(self.fd.as_raw_fd(), packet.as_ptr().cast(), packet.len(), 0) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Received>> {
        let deadline = std::time::Instant::now() + timeout;
        let mut buf = vec![0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let mut pfd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = left.as_millis().min(i32::MAX as u128) as i32;
            let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
            if rc < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if rc == 0 {
                return Ok(None);
            }
            let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_ll>() as u32;
            let n = unsafe {
                libc::recvfrom(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                    &mut addr as *mut libc::sockaddr_ll as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if addr.sll_pkttype == PACKET_OUTGOING {
                continue;
            }
            if let Ok((info, frame)) = radiotap::parse(&buf[..n as usize]) {
                if !info.bad_fcs {
                    return Ok(Some(Received {
                        info,
                        frame: frame.to_vec(),
                    }));
                }
            }
        }
    }
}
