//! Network adapters to capture on, from Windows' own adapter list (`GetAdaptersAddresses`).
//!
//! Windows' list needs no access to the capture driver, which with Npcap restricted to
//! administrators means no admin prompt; only opening an adapter for capture shows one. Npcap
//! names each adapter `\Device\NPF_{<adapter GUID>}`, which Windows reports as the adapter name.

use std::ffi::{CStr, c_char, c_void};
use std::net::Ipv4Addr;

#[derive(Debug, Clone, PartialEq)]
pub struct Adapter {
    /// Npcap device path, e.g. `\Device\NPF_{...}`.
    pub device: String,
    pub name: String,
}

/// Adapters worth capturing on: connected, not loopback, with an IPv4 address that is not
/// link-local.
pub fn usable_adapters() -> Result<Vec<Adapter>, String> {
    Ok(list()?
        .into_iter()
        .filter(|a| a.usable())
        .map(|a| a.adapter)
        .collect())
}

/// What Windows reports about one adapter.
struct Listed {
    adapter: Adapter,
    up: bool,
    loopback: bool,
    ipv4: Vec<Ipv4Addr>,
}

impl Listed {
    fn usable(&self) -> bool {
        self.up
            && !self.loopback
            && self
                .ipv4
                .iter()
                .any(|ip| !ip.is_link_local() && !ip.is_loopback() && !ip.is_unspecified())
    }
}

#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetAdaptersAddresses(
        family: u32,
        flags: u32,
        reserved: *mut c_void,
        addresses: *mut u8,
        size: *mut u32,
    ) -> u32;
}

const AF_INET: u32 = 2;
const GAA_FLAG_SKIP_ANYCAST: u32 = 0x2;
const GAA_FLAG_SKIP_MULTICAST: u32 = 0x4;
const GAA_FLAG_SKIP_DNS_SERVER: u32 = 0x8;
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_OPER_STATUS_UP: i32 = 1;

/// The leading fields of IP_ADAPTER_ADDRESSES (64-bit layout). Entries are reached through
/// `next`, so the fields after `oper_status` never need declaring.
#[repr(C)]
struct AdapterAddresses {
    length_and_index: u64,
    next: *const AdapterAddresses,
    adapter_name: *const c_char,
    first_unicast: *const UnicastAddress,
    first_anycast: *const c_void,
    first_multicast: *const c_void,
    first_dns_server: *const c_void,
    dns_suffix: *const u16,
    description: *const u16,
    friendly_name: *const u16,
    physical_address: [u8; 8],
    physical_address_length: u32,
    flags: u32,
    mtu: u32,
    if_type: u32,
    oper_status: i32,
}

/// The leading fields of IP_ADAPTER_UNICAST_ADDRESS.
#[repr(C)]
struct UnicastAddress {
    length_and_flags: u64,
    next: *const UnicastAddress,
    sockaddr: *const u8,
    sockaddr_length: i32,
}

fn list() -> Result<Vec<Listed>, String> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // u64 elements keep the buffer 8-byte aligned for the structures Windows writes into it.
    let mut buffer: Vec<u64> = vec![0; 2048];
    for _ in 0..3 {
        let mut size = (buffer.len() * 8) as u32;
        // SAFETY: `buffer` holds `size` writable bytes; Windows fills it with a linked list of
        // IP_ADAPTER_ADDRESSES entries that stay valid while `buffer` lives.
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET,
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match result {
            0 => return Ok(unsafe { read_list(buffer.as_ptr().cast()) }),
            ERROR_BUFFER_OVERFLOW => buffer = vec![0; (size as usize).div_ceil(8)],
            code => return Err(format!("cannot list network adapters (error {code})")),
        }
    }
    Err("cannot list network adapters (list keeps growing)".into())
}

/// # Safety
/// `first` must point to a list written by GetAdaptersAddresses that is still alive.
unsafe fn read_list(first: *const AdapterAddresses) -> Vec<Listed> {
    let mut out = Vec::new();
    let mut entry = first;
    // SAFETY (whole loop): every pointer followed comes from the list Windows wrote.
    unsafe {
        while let Some(a) = entry.as_ref() {
            let guid = CStr::from_ptr(a.adapter_name)
                .to_string_lossy()
                .into_owned();
            let mut ipv4 = Vec::new();
            let mut unicast = a.first_unicast;
            while let Some(u) = unicast.as_ref() {
                // sockaddr_in: family (u16), port (u16), address (4 bytes).
                if u.sockaddr_length >= 8
                    && u16::from_ne_bytes([*u.sockaddr, *u.sockaddr.add(1)]) == AF_INET as u16
                {
                    let ip = std::slice::from_raw_parts(u.sockaddr.add(4), 4);
                    ipv4.push(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]));
                }
                unicast = u.next;
            }
            out.push(Listed {
                adapter: Adapter {
                    device: format!(r"\Device\NPF_{guid}"),
                    name: wide_str(a.friendly_name),
                },
                up: a.oper_status == IF_OPER_STATUS_UP,
                loopback: a.if_type == IF_TYPE_SOFTWARE_LOOPBACK,
                ipv4,
            });
            entry = a.next;
        }
    }
    out
}

/// # Safety
/// `p` must be null or a NUL-terminated UTF-16 string.
unsafe fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: NUL-terminated per the caller's contract.
    unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(up: bool, loopback: bool, ips: &[[u8; 4]]) -> Listed {
        Listed {
            adapter: Adapter {
                device: String::new(),
                name: String::new(),
            },
            up,
            loopback,
            ipv4: ips.iter().map(|&ip| Ipv4Addr::from(ip)).collect(),
        }
    }

    #[test]
    fn usable_rules() {
        assert!(listed(true, false, &[[10, 0, 1, 116]]).usable());
        assert!(
            !listed(false, false, &[[10, 0, 1, 116]]).usable(),
            "disconnected"
        );
        assert!(!listed(true, true, &[[127, 0, 0, 1]]).usable(), "loopback");
        assert!(
            !listed(true, false, &[[169, 254, 213, 149]]).usable(),
            "link-local only"
        );
        assert!(!listed(true, false, &[]).usable(), "no IPv4");
    }

    #[test]
    fn layouts_match_windows() {
        // Offsets from the Windows SDK, 64-bit.
        assert_eq!(std::mem::offset_of!(AdapterAddresses, adapter_name), 16);
        assert_eq!(std::mem::offset_of!(AdapterAddresses, friendly_name), 72);
        assert_eq!(std::mem::offset_of!(AdapterAddresses, if_type), 100);
        assert_eq!(std::mem::offset_of!(AdapterAddresses, oper_status), 104);
        assert_eq!(std::mem::offset_of!(UnicastAddress, sockaddr), 16);
        assert_eq!(std::mem::offset_of!(UnicastAddress, sockaddr_length), 24);
    }

    #[test]
    fn lists_this_machines_adapters() {
        // Windows always reports at least the loopback adapter.
        let all = list().unwrap();
        assert!(!all.is_empty());
        assert!(
            all.iter()
                .all(|a| a.adapter.device.starts_with(r"\Device\NPF_{"))
        );
    }
}
