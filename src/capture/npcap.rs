//! Live capture straight from Npcap, the Windows packet capture driver.
//!
//! Npcap's `wpcap.dll` is loaded when capture starts, so building needs no Npcap SDK and the
//! exe runs on any PC that has Npcap installed. Capture only receives; nothing is sent.
//! With Npcap restricted to administrators, opening the first adapter shows one admin prompt.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::time::Duration;

use crate::capture::Packet;

type PcapT = c_void;

#[repr(C)]
#[allow(dead_code)] // mirrors the C struct; libpcap reads and writes it
struct BpfProgram {
    len: u32,
    insns: *mut c_void,
}

/// `struct pcap_pkthdr` on Windows, where `struct timeval` holds two 32-bit `long`s.
#[repr(C)]
#[allow(dead_code)] // mirrors the C struct; `len` (original length) is not needed
struct PktHeader {
    tv_sec: i32,
    tv_usec: i32,
    caplen: u32,
    len: u32,
}

// libpcap's C signatures for the functions the tool uses.
type OpenLiveFn =
    unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> *mut PcapT;
type DatalinkFn = unsafe extern "C" fn(*mut PcapT) -> c_int;
type CompileFn =
    unsafe extern "C" fn(*mut PcapT, *mut BpfProgram, *const c_char, c_int, u32) -> c_int;
type SetFilterFn = unsafe extern "C" fn(*mut PcapT, *mut BpfProgram) -> c_int;
type FreeCodeFn = unsafe extern "C" fn(*mut BpfProgram);
type NextExFn = unsafe extern "C" fn(*mut PcapT, *mut *mut PktHeader, *mut *const u8) -> c_int;
type GetErrFn = unsafe extern "C" fn(*mut PcapT) -> *const c_char;
type CloseFn = unsafe extern "C" fn(*mut PcapT);

/// The `wpcap.dll` functions the tool uses.
struct Api {
    open_live: OpenLiveFn,
    datalink: DatalinkFn,
    compile: CompileFn,
    setfilter: SetFilterFn,
    freecode: FreeCodeFn,
    next_ex: NextExFn,
    geterr: GetErrFn,
    close: CloseFn,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

/// Resolve the DLL's own dependencies (Npcap's `Packet.dll`) from its folder.
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
const PCAP_ERRBUF_SIZE: usize = 256;
const DLT_EN10MB: c_int = 1;
const SNAPLEN: c_int = 65_535;
/// How long a read waits before returning with nothing, so readers notice when to stop.
const READ_TIMEOUT_MS: c_int = 500;

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// Npcap's API, loaded once. Fails with an explanation when Npcap is not installed.
fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    let mut dir = [0u16; 260];
    // SAFETY: `dir` has room for 260 UTF-16 units, the size passed.
    let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let system = String::from_utf16_lossy(&dir[..n.min(dir.len())]);
    // Npcap's own folder first; WinPcap-compatible installs also put a copy in System32.
    let candidates = [
        format!(r"{system}\Npcap\wpcap.dll"),
        format!(r"{system}\wpcap.dll"),
    ];
    let module = candidates
        .iter()
        .map(|path| {
            let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `wide` is a NUL-terminated path; the module stays loaded for the process.
            unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) }
        })
        .find(|m| !m.is_null())
        .ok_or("Npcap is not installed. Install it from https://npcap.com and restart LastWarHQ Scanner.")?;

    let symbol = |name: &str| -> Result<*mut c_void, String> {
        let c_name = CString::new(name).expect("symbol names have no NUL");
        // SAFETY: `module` is a loaded DLL; `c_name` is NUL-terminated.
        let p = unsafe { GetProcAddress(module, c_name.as_ptr()) };
        if p.is_null() {
            Err(format!("Npcap's wpcap.dll has no {name}; reinstall Npcap"))
        } else {
            Ok(p)
        }
    };
    // SAFETY: each pointer is the named libpcap function, whose C signature matches the field
    // it is stored in.
    unsafe {
        Ok(Api {
            open_live: std::mem::transmute::<*mut c_void, OpenLiveFn>(symbol("pcap_open_live")?),
            datalink: std::mem::transmute::<*mut c_void, DatalinkFn>(symbol("pcap_datalink")?),
            compile: std::mem::transmute::<*mut c_void, CompileFn>(symbol("pcap_compile")?),
            setfilter: std::mem::transmute::<*mut c_void, SetFilterFn>(symbol("pcap_setfilter")?),
            freecode: std::mem::transmute::<*mut c_void, FreeCodeFn>(symbol("pcap_freecode")?),
            next_ex: std::mem::transmute::<*mut c_void, NextExFn>(symbol("pcap_next_ex")?),
            geterr: std::mem::transmute::<*mut c_void, GetErrFn>(symbol("pcap_geterr")?),
            close: std::mem::transmute::<*mut c_void, CloseFn>(symbol("pcap_close")?),
        })
    }
}

/// Checks that Npcap is installed, without opening any adapter (so no admin prompt).
pub fn available() -> Result<(), String> {
    api().map(|_| ())
}

/// An open adapter. Closed on drop.
pub struct Capture {
    handle: *mut PcapT,
    api: &'static Api,
}

// SAFETY: a pcap handle may be moved to another thread; each Capture is used by one thread.
unsafe impl Send for Capture {}

impl Capture {
    /// Opens `device` (an Npcap name such as `\Device\NPF_{...}`) with a BPF `filter`.
    pub fn open(device: &str, filter: &str) -> Result<Self, String> {
        let api = api()?;
        let c_device = CString::new(device).map_err(|_| "bad device name".to_string())?;
        let mut errbuf = [0 as c_char; PCAP_ERRBUF_SIZE];
        // SAFETY: NUL-terminated device name and an errbuf of PCAP_ERRBUF_SIZE, per libpcap.
        let handle = unsafe {
            (api.open_live)(
                c_device.as_ptr(),
                SNAPLEN,
                0,
                READ_TIMEOUT_MS,
                errbuf.as_mut_ptr(),
            )
        };
        if handle.is_null() {
            // SAFETY: libpcap wrote a NUL-terminated message into errbuf.
            let msg = unsafe { CStr::from_ptr(errbuf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            return Err(format!("cannot open {device}: {msg}"));
        }
        let capture = Capture { handle, api };
        // SAFETY: `handle` is open.
        if unsafe { (api.datalink)(handle) } != DLT_EN10MB {
            return Err(format!("{device} is not an Ethernet-style adapter"));
        }
        capture.set_filter(filter)?;
        Ok(capture)
    }

    fn set_filter(&self, filter: &str) -> Result<(), String> {
        let c_filter = CString::new(filter).map_err(|_| "bad filter".to_string())?;
        let mut program = BpfProgram {
            len: 0,
            insns: std::ptr::null_mut(),
        };
        // SAFETY: `handle` is open; `program` is freed after use. Netmask 0 is fine for a
        // filter with no broadcast tests.
        unsafe {
            if (self.api.compile)(self.handle, &mut program, c_filter.as_ptr(), 1, 0) != 0 {
                return Err(format!("filter: {}", self.error()));
            }
            let set = (self.api.setfilter)(self.handle, &mut program);
            (self.api.freecode)(&mut program);
            if set != 0 {
                return Err(format!("filter: {}", self.error()));
            }
        }
        Ok(())
    }

    fn error(&self) -> String {
        // SAFETY: pcap_geterr returns a NUL-terminated string owned by the handle.
        unsafe { CStr::from_ptr((self.api.geterr)(self.handle)) }
            .to_string_lossy()
            .into_owned()
    }

    /// The next packet, or `None` when the read timeout passed with nothing to read.
    pub fn read_packet(&mut self) -> Result<Option<Packet>, String> {
        let mut header: *mut PktHeader = std::ptr::null_mut();
        let mut data: *const u8 = std::ptr::null();
        // SAFETY: `handle` is open; on 1, libpcap points `header` and `data` at a packet that
        // stays valid until the next call, and it is copied out before then.
        unsafe {
            match (self.api.next_ex)(self.handle, &mut header, &mut data) {
                1 => {
                    let h = &*header;
                    let bytes = std::slice::from_raw_parts(data, h.caplen as usize).to_vec();
                    let time = Duration::from_secs(u64::from(h.tv_sec as u32))
                        + Duration::from_micros(u64::from(h.tv_usec as u32));
                    Ok(Some(Packet { time, data: bytes }))
                }
                0 => Ok(None),
                _ => Err(self.error()),
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: the handle is open and closed exactly once.
        unsafe { (self.api.close)(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_windows() {
        assert_eq!(size_of::<PktHeader>(), 16);
        assert_eq!(size_of::<BpfProgram>(), 16);
    }

    #[test]
    fn loading_npcap_works_or_explains_how_to_install_it() {
        // Loading the library opens no adapter, so it needs no admin rights. On a machine
        // without Npcap (such as a build server) the error must point to the installer.
        match available() {
            Ok(()) => {}
            Err(err) => assert!(err.contains("https://npcap.com"), "{err}"),
        }
    }
}
