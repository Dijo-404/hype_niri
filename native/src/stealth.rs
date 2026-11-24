use crate::{
    Result, bus, err,
    network::{self, Manager, ObjectPath, Settings},
    runtime,
};
use gtk::{glib, prelude::*};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const WIFI: &str = "802-11-wireless";
const CHECK_URL: &str = "https://check.torproject.org/api/ip";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Link {
    Wifi,
    Ethernet,
}
impl Link {
    fn name(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Ethernet => "ethernet",
        }
    }
}

fn valid_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() < libc::IFNAMSIZ
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn mac_bytes(value: &str) -> Option<[u8; 6]> {
    if value.len() != 17 {
        return None;
    }
    let mut bytes = [0; 6];
    for (i, part) in value.split(':').enumerate() {
        if i >= 6 || part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        bytes[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(bytes)
}
fn mac_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn random_uuid() -> Result<String> {
    let mut b = random_bytes::<16>()?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let hex = b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
fn random_mac(original: &str) -> Result<String> {
    for _ in 0..8 {
        let mut b = random_bytes::<6>()?;
        b[0] = (b[0] | 2) & 254;
        if Some(b) != mac_bytes(original) {
            return Ok(mac_text(&b));
        }
    }
    Err(err("Could not generate a different MAC address"))
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    interface: String,
    link: Link,
    original_mac: String,
    tor_preexisting: bool,
    connection: String,
    clone_uuid: String,
    clone_name: String,
    spoof_mac: String,
}
impl State {
    fn encode(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.interface,
            self.link.name(),
            self.original_mac,
            u8::from(self.tor_preexisting),
            self.connection,
            self.clone_uuid,
            self.clone_name,
            self.spoof_mac
        )
    }
    fn decode(text: &str) -> Result<Self> {
        let f: Vec<_> = text.split_terminator('\n').collect();
        if f.len() != 8 {
            return Err(err("Saved state is incomplete"));
        }
        if !valid_interface(f[0]) {
            return Err(err("Saved network interface is invalid"));
        }
        let link = match f[1] {
            "wifi" => Link::Wifi,
            "ethernet" => Link::Ethernet,
            _ => return Err(err("Saved connection type is invalid")),
        };
        if mac_bytes(f[2]).is_none() {
            return Err(err("Saved MAC address is invalid"));
        }
        if !matches!(f[3], "0" | "1") {
            return Err(err("Saved Tor state is invalid"));
        }
        if !valid_uuid(f[4]) {
            return Err(err("Saved NetworkManager connection is invalid"));
        }
        if link == Link::Wifi {
            if (!f[5].is_empty() && !valid_uuid(f[5]))
                || !f[6].strip_prefix("stealth-").is_some_and(valid_uuid)
                || mac_bytes(f[7]).is_none()
            {
                return Err(err("Saved temporary Wi-Fi connection is invalid"));
            }
        } else if f[5..].iter().any(|s| !s.is_empty()) {
            return Err(err("Ethernet state contains a temporary Wi-Fi profile"));
        }
        Ok(Self {
            interface: f[0].into(),
            link,
            original_mac: f[2].into(),
            tor_preexisting: f[3] == "1",
            connection: f[4].into(),
            clone_uuid: f[5].into(),
            clone_name: f[6].into(),
            spoof_mac: f[7].into(),
        })
    }
}

#[derive(Clone)]
struct Paths {
    directory: PathBuf,
    log: PathBuf,
    rules: PathBuf,
}
impl Paths {
    fn system() -> Self {
        Self {
            directory: "/run/stealth".into(),
            log: "/var/log/stealth.log".into(),
            rules: "/etc/stealth.nft".into(),
        }
    }
    fn state(&self) -> PathBuf {
        self.directory.join("state")
    }
    fn active(&self) -> PathBuf {
        self.directory.join("active")
    }
    fn policy(&self) -> PathBuf {
        self.directory.join("mac-policy")
    }
    fn prepare(&self) -> Result<()> {
        match fs::DirBuilder::new().mode(0o755).create(&self.directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let metadata = fs::symlink_metadata(&self.directory)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(err("Unsafe Stealth state directory"));
        }
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }
    fn read_state(&self) -> Result<State> {
        State::decode(&read_private(&self.state())?)
    }
    fn write_state(&self, state: &State) -> Result<()> {
        runtime::atomic_write(&self.state(), state.encode().as_bytes())
    }
    fn clear(&self) -> Result<()> {
        remove_if_exists(&self.state())?;
        let _ = fs::remove_dir(&self.directory);
        Ok(())
    }
    fn log(&self, message: &str) {
        let stamp = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%Y-%m-%d %H:%M:%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let line = format!("[{stamp}] {message}\n");
        print!("{line}");
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.log)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
fn read_private(path: &Path) -> Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(err("Unsafe Stealth recovery file"));
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(16385)
        .read_to_string(&mut text)?;
    if text.len() > 16384 {
        return Err(err("Stealth recovery file is too large"));
    }
    Ok(text)
}
fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

type NftNew = unsafe extern "C" fn(u32) -> *mut c_void;
type NftFree = unsafe extern "C" fn(*mut c_void);
type NftBuffer = unsafe extern "C" fn(*mut c_void) -> i32;
type NftOutput = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type NftFlags = unsafe extern "C" fn(*mut c_void, u32);
type NftRun = unsafe extern "C" fn(*mut c_void, *const c_char) -> i32;
struct Nft {
    _library: libloading::Library,
    new: NftNew,
    free: NftFree,
    buffer_output: NftBuffer,
    buffer_error: NftBuffer,
    output: NftOutput,
    error: NftOutput,
    flags: NftFlags,
    run: NftRun,
}
struct NftContext {
    pointer: *mut c_void,
    free: NftFree,
}
impl Drop for NftContext {
    fn drop(&mut self) {
        unsafe { (self.free)(self.pointer) }
    }
}
impl Nft {
    fn new() -> Result<Self> {
        // Signatures are from the installed nftables/libnftables.h C API.
        unsafe {
            let library = libloading::Library::new("libnftables.so.1")?;
            let new = *library.get(b"nft_ctx_new\0")?;
            let free = *library.get(b"nft_ctx_free\0")?;
            let buffer_output = *library.get(b"nft_ctx_buffer_output\0")?;
            let buffer_error = *library.get(b"nft_ctx_buffer_error\0")?;
            let output = *library.get(b"nft_ctx_get_output_buffer\0")?;
            let error = *library.get(b"nft_ctx_get_error_buffer\0")?;
            let flags = *library.get(b"nft_ctx_output_set_flags\0")?;
            let run = *library.get(b"nft_run_cmd_from_buffer\0")?;
            Ok(Self {
                _library: library,
                new,
                free,
                buffer_output,
                buffer_error,
                output,
                error,
                flags,
                run,
            })
        }
    }
    fn run(&self, command: &str) -> Result<String> {
        let command = CString::new(command)?;
        unsafe {
            let pointer = (self.new)(0);
            if pointer.is_null() {
                return Err(err("Could not allocate nftables context"));
            }
            let context = NftContext {
                pointer,
                free: self.free,
            };
            if (self.buffer_output)(context.pointer) != 0
                || (self.buffer_error)(context.pointer) != 0
            {
                return Err(err("Could not buffer nftables output"));
            }
            (self.flags)(context.pointer, 1 << 4);
            let status = (self.run)(context.pointer, command.as_ptr());
            let output = if status == 0 {
                (self.output)(context.pointer)
            } else {
                (self.error)(context.pointer)
            };
            let text = if output.is_null() {
                String::new()
            } else {
                CStr::from_ptr(output).to_string_lossy().into_owned()
            };
            if status == 0 {
                Ok(text)
            } else {
                Err(err(format!("nftables: {}", text.trim())))
            }
        }
    }
    fn present(&self) -> Result<bool> {
        let result: serde_json::Value = serde_json::from_str(&self.run("list tables inet\n")?)?;
        let objects = result
            .get("nftables")
            .and_then(|v| v.as_array())
            .ok_or_else(|| err("Invalid nftables table listing"))?;
        Ok(objects.iter().any(|object| {
            object
                .get("table")
                .is_some_and(|table| table["family"] == "inet" && table["name"] == "stealth")
        }))
    }
}
pub fn firewall_present() -> Result<bool> {
    Nft::new()?.present()
}

fn interface_for_route_reply(data: &[u8]) -> Result<u32> {
    let mut offset = 0;
    while offset + 16 <= data.len() {
        let length = u32::from_ne_bytes(data[offset..offset + 4].try_into()?) as usize;
        if length < 16 || offset + length > data.len() {
            return Err(err("Truncated route response"));
        }
        let kind = u16::from_ne_bytes(data[offset + 4..offset + 6].try_into()?);
        if kind == libc::NLMSG_ERROR as u16 {
            if length < 20 {
                return Err(err("Invalid route error"));
            }
            let code = i32::from_ne_bytes(data[offset + 16..offset + 20].try_into()?);
            if code != 0 {
                return Err(std::io::Error::from_raw_os_error(-code).into());
            }
        } else if kind == libc::RTM_NEWROUTE {
            if length < 28 || data[offset + 16] != libc::AF_INET as u8 {
                return Err(err("Invalid IPv4 route response"));
            }
            let mut attr = offset + 28;
            while attr + 4 <= offset + length {
                let size = u16::from_ne_bytes(data[attr..attr + 2].try_into()?) as usize;
                let kind = u16::from_ne_bytes(data[attr + 2..attr + 4].try_into()?);
                if size < 4 || attr + size > offset + length {
                    return Err(err("Invalid route attribute"));
                }
                if kind == libc::RTA_OIF && size == 8 {
                    return Ok(u32::from_ne_bytes(data[attr + 4..attr + 8].try_into()?));
                }
                attr += (size + 3) & !3;
            }
        }
        offset += (length + 3) & !3;
    }
    Err(err("No default IPv4 network interface was found"))
}
fn default_interface() -> Result<String> {
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let timeout = libc::timeval {
        tv_sec: 3,
        tv_usec: 0,
    };
    if unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            std::mem::size_of_val(&timeout) as u32,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut message = [0_u8; 36];
    message[..4].copy_from_slice(&36_u32.to_ne_bytes());
    message[4..6].copy_from_slice(&libc::RTM_GETROUTE.to_ne_bytes());
    message[6..8].copy_from_slice(&(libc::NLM_F_REQUEST as u16).to_ne_bytes());
    message[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    message[16] = libc::AF_INET as u8;
    message[17] = 32;
    message[28..30].copy_from_slice(&8_u16.to_ne_bytes());
    message[30..32].copy_from_slice(&libc::RTA_DST.to_ne_bytes());
    message[32..36].copy_from_slice(&[1, 1, 1, 1]);
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    if unsafe {
        libc::sendto(
            fd.as_raw_fd(),
            message.as_ptr().cast(),
            message.len(),
            0,
            (&address as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&address) as u32,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut response = [0_u8; 8192];
    let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    let mut sender_length = std::mem::size_of_val(&sender) as libc::socklen_t;
    let size = unsafe {
        libc::recvfrom(
            fd.as_raw_fd(),
            response.as_mut_ptr().cast(),
            response.len(),
            0,
            (&mut sender as *mut libc::sockaddr_nl).cast(),
            &mut sender_length,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if sender_length != std::mem::size_of_val(&sender) as libc::socklen_t
        || sender.nl_family != libc::AF_NETLINK as u16
        || sender.nl_pid != 0
        || size < 16
        || response[8..12] != 1_u32.to_ne_bytes()
    {
        return Err(err("Route response did not come from the kernel lookup"));
    }
    let index = interface_for_route_reply(&response[..size as usize])?;
    let mut name = [0_i8; libc::IFNAMSIZ];
    if unsafe { libc::if_indextoname(index, name.as_mut_ptr()) }.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let name = unsafe { CStr::from_ptr(name.as_ptr()) }
        .to_str()?
        .to_owned();
    if !valid_interface(&name) {
        return Err(err("Invalid route interface name"));
    }
    Ok(name)
}
fn current_mac(interface: &str) -> Result<String> {
    if !valid_interface(interface) {
        return Err(err("Invalid interface name"));
    }
    let value = fs::read_to_string(Path::new("/sys/class/net").join(interface).join("address"))?;
    Ok(mac_text(
        &mac_bytes(value.trim()).ok_or_else(|| err("Invalid interface MAC address"))?,
    ))
}
fn restore_link_mac(interface: &str, mac: &str) -> Result<()> {
    if !valid_interface(interface) {
        return Err(err("Invalid interface name"));
    }
    let mac = mac_bytes(mac).ok_or_else(|| err("Invalid saved MAC"))?;
    let descriptor =
        unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (slot, b) in request.ifr_name.iter_mut().zip(interface.bytes()) {
        *slot = b as c_char;
    }
    if unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::SIOCGIFFLAGS, &mut request) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let original_flags = unsafe { request.ifr_ifru.ifru_flags };
    request.ifr_ifru.ifru_flags = original_flags & !(libc::IFF_UP as i16);
    if unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::SIOCSIFFLAGS, &request) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut address: libc::sockaddr = unsafe { std::mem::zeroed() };
    address.sa_family = libc::ARPHRD_ETHER;
    for (slot, b) in address.sa_data.iter_mut().zip(mac) {
        *slot = b as c_char;
    }
    request.ifr_ifru.ifru_hwaddr = address;
    let result = unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::SIOCSIFHWADDR, &request) };
    let error = std::io::Error::last_os_error();
    request.ifr_ifru.ifru_flags = original_flags | libc::IFF_UP as i16;
    let up = unsafe { libc::ioctl(descriptor.as_raw_fd(), libc::SIOCSIFFLAGS, &request) };
    if result < 0 {
        return Err(error.into());
    }
    if up < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn socket_present(text: &str, port: u16, tcp: bool) -> bool {
    let expected = format!("0100007F:{port:04X}");
    text.lines().skip(1).any(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.get(1) == Some(&expected.as_str())
            && fields.get(3) == Some(&if tcp { "0A" } else { "07" })
    })
}
fn tor_listeners() -> Result<bool> {
    Ok(
        socket_present(&fs::read_to_string("/proc/net/tcp")?, 9040, true)
            && socket_present(&fs::read_to_string("/proc/net/udp")?, 5353, false),
    )
}
fn verified_ip(text: &str) -> Result<String> {
    let response: serde_json::Value = serde_json::from_str(text)?;
    if response["IsTor"] != true {
        return Err(err("The regular TCP request did not use Tor"));
    }
    let ip = response["IP"]
        .as_str()
        .ok_or_else(|| err("Tor check has no IP address"))?;
    if !matches!(ip.parse::<IpAddr>(), Ok(IpAddr::V4(_))) {
        return Err(err("Invalid IPv4 Tor exit address"));
    }
    Ok(ip.into())
}
fn mac_policy(settings: &Settings) -> Result<String> {
    let Some(wifi) = settings.get(WIFI) else {
        return Err(err("Saved profile is not Wi-Fi"));
    };
    if let Some(value) = wifi.get("assigned-mac-address") {
        return value
            .str()
            .map(str::to_owned)
            .ok_or_else(|| err("Invalid assigned MAC policy"));
    }
    if let Some(value) = wifi.get("cloned-mac-address") {
        let bytes = value
            .get::<Vec<u8>>()
            .ok_or_else(|| err("Invalid legacy MAC policy"))?;
        if bytes.is_empty() {
            return Ok(String::new());
        }
        if bytes.len() != 6 {
            return Err(err("Invalid legacy MAC address"));
        }
        return Ok(mac_text(&bytes));
    }
    Ok(String::new())
}
fn set_mac_policy(settings: &mut Settings, policy: &str) {
    let wifi = settings.entry(WIFI.into()).or_default();
    wifi.remove("cloned-mac-address");
    wifi.remove("assigned-mac-address");
    if !policy.is_empty() {
        wifi.insert("assigned-mac-address".into(), policy.to_variant());
    }
}

#[derive(Clone, Copy, Debug)]
struct Restored {
    network: bool,
    unmanaged: bool,
}
trait Platform {
    fn interrupted(&self) -> bool {
        false
    }
    async fn firewall(&mut self) -> Result<bool>;
    async fn install_firewall(&mut self, interface: &str) -> Result<()>;
    async fn remove_firewall(&mut self) -> Result<()>;
    async fn interface(&mut self) -> Result<String>;
    async fn connection(&mut self, interface: &str) -> Result<(Link, String)>;
    async fn mac(&mut self, interface: &str) -> Result<String>;
    async fn clone_wifi(&mut self, state: &State) -> Result<()>;
    async fn disconnect(&mut self, interface: &str) -> Result<()>;
    async fn activate(&mut self, interface: &str, uuid: &str) -> Result<()>;
    async fn wait_network(&mut self, interface: &str, uuid: &str) -> Result<()>;
    async fn restore(&mut self, state: &State, require_network: bool) -> Result<Restored>;
    async fn delete_clone(&mut self, state: &State) -> Result<()>;
    async fn unit_state(&mut self, unit: &str) -> Result<String>;
    async fn unit_change(&mut self, unit: &str, start: bool) -> Result<()>;
    async fn listeners(&mut self) -> Result<bool>;
    async fn verify(&mut self, attempts: u32) -> Result<String>;
}

struct Native {
    manager: Manager,
    nft: Nft,
    paths: Paths,
    interrupted: Rc<Cell<bool>>,
}
impl Native {
    async fn profile(&self, uuid: &str) -> Result<ObjectPath> {
        self.manager
            .by_uuid(uuid)
            .await?
            .ok_or_else(|| err("Saved NetworkManager connection is unavailable"))
    }
    async fn full_settings(&self, profile: &str) -> Result<Settings> {
        let mut settings = self.manager.settings(profile).await?;
        for section in ["802-11-wireless-security", "802-1x"] {
            if !settings.contains_key(section) {
                continue;
            }
            let secrets = self
                .manager
                .call(
                    profile,
                    CONNECTION,
                    "GetSecrets",
                    Some(&(section,).to_variant()),
                )
                .await?
                .get::<(Settings,)>()
                .ok_or_else(|| err("Invalid saved Wi-Fi credentials"))?
                .0;
            for (section, properties) in secrets {
                settings.entry(section).or_default().extend(properties);
            }
        }
        Ok(settings)
    }
    async fn activate_saved(manager: &Manager, paths: &Paths, state: &State) -> Result<bool> {
        let profile = manager
            .by_uuid(&state.connection)
            .await?
            .ok_or_else(|| err("Saved NetworkManager connection is unavailable"))?;
        // Updating this existing profile preserves its secrets in NetworkManager/its agent.
        let mut settings = manager.settings(profile.as_str()).await?;
        let policy = if paths.policy().exists() {
            read_private(&paths.policy())?
                .trim_end_matches('\n')
                .to_owned()
        } else {
            let policy = mac_policy(&settings)?;
            runtime::atomic_write(&paths.policy(), format!("{policy}\n").as_bytes())?;
            policy
        };
        set_mac_policy(&mut settings, &state.original_mac);
        let activation = async {
            manager.update(profile.as_str(), &settings, true).await?;
            let device = manager.device(&state.interface).await?;
            let active = manager.activate(profile.as_str(), device.as_str()).await?;
            manager
                .wait_active(active.as_str(), Duration::from_secs(30))
                .await
        }
        .await;
        // Always restore the original in-memory policy, including failed/interrupted activation.
        set_mac_policy(&mut settings, &policy);
        manager.update(profile.as_str(), &settings, true).await?;
        remove_if_exists(&paths.policy())?;
        match activation {
            Ok(()) => Ok(true),
            Err(error) => {
                eprintln!("Original Wi-Fi profile could not reconnect: {error}");
                Ok(false)
            }
        }
    }
}
impl Platform for Native {
    fn interrupted(&self) -> bool {
        self.interrupted.get()
    }
    async fn firewall(&mut self) -> Result<bool> {
        self.nft.present()
    }
    async fn install_firewall(&mut self, interface: &str) -> Result<()> {
        if !valid_interface(interface) {
            return Err(err("Invalid firewall uplink"));
        }
        self.nft.run(&format!(
            "define uplink = \"{interface}\"\n{}",
            fs::read_to_string(&self.paths.rules)?
        ))?;
        Ok(())
    }
    async fn remove_firewall(&mut self) -> Result<()> {
        self.nft.run("delete table inet stealth\n")?;
        Ok(())
    }
    async fn interface(&mut self) -> Result<String> {
        default_interface()
    }
    async fn connection(&mut self, interface: &str) -> Result<(Link, String)> {
        let device = self.manager.device(interface).await?;
        let kind = self
            .manager
            .property::<u32>(device.as_str(), DEVICE, "DeviceType")
            .await?;
        let link = match kind {
            1 => Link::Ethernet,
            2 if Path::new("/sys/class/net")
                .join(interface)
                .join("wireless")
                .is_dir() =>
            {
                Link::Wifi
            }
            _ => {
                return Err(err(
                    "The default IPv4 route must use NetworkManager Wi-Fi or Ethernet",
                ));
            }
        };
        let active: ObjectPath = self
            .manager
            .property(device.as_str(), DEVICE, "ActiveConnection")
            .await?;
        let uuid: String = self
            .manager
            .property(active.as_str(), ACTIVE, "Uuid")
            .await?;
        if !valid_uuid(&uuid) {
            return Err(err("Invalid active NetworkManager connection UUID"));
        }
        Ok((link, uuid))
    }
    async fn mac(&mut self, interface: &str) -> Result<String> {
        current_mac(interface)
    }
    async fn clone_wifi(&mut self, state: &State) -> Result<()> {
        let original = self.profile(&state.connection).await?;
        let mut settings = self.full_settings(original.as_str()).await?;
        let connection = settings.entry("connection".into()).or_default();
        connection.insert("id".into(), state.clone_name.to_variant());
        connection.insert("uuid".into(), state.clone_uuid.to_variant());
        connection.insert("autoconnect".into(), false.to_variant());
        set_mac_policy(&mut settings, &state.spoof_mac);
        self.manager.add(&settings, true).await?;
        Ok(())
    }
    async fn disconnect(&mut self, interface: &str) -> Result<()> {
        let device = self.manager.device(interface).await?;
        if let Err(error) = self
            .manager
            .call(device.as_str(), DEVICE, "Disconnect", None)
            .await
            && !matches!(
                self.manager
                    .property::<u32>(device.as_str(), DEVICE, "State")
                    .await,
                Ok(20 | 30)
            )
        {
            return Err(error);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while !matches!(
            self.manager
                .property::<u32>(device.as_str(), DEVICE, "State")
                .await?,
            20 | 30
        ) {
            if Instant::now() >= deadline {
                return Err(err("Wi-Fi disconnect timed out"));
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }
        Ok(())
    }
    async fn activate(&mut self, interface: &str, uuid: &str) -> Result<()> {
        let profile = self.profile(uuid).await?;
        let device = self.manager.device(interface).await?;
        let active = self
            .manager
            .activate(profile.as_str(), device.as_str())
            .await?;
        self.manager
            .wait_active(active.as_str(), Duration::from_secs(30))
            .await
    }
    async fn wait_network(&mut self, interface: &str, uuid: &str) -> Result<()> {
        let device = self.manager.device(interface).await?;
        for _ in 0..15 {
            let check = async {
                if self
                    .manager
                    .property::<u32>(device.as_str(), DEVICE, "State")
                    .await?
                    != 100
                {
                    return Ok(false);
                }
                let active: ObjectPath = self
                    .manager
                    .property(device.as_str(), DEVICE, "ActiveConnection")
                    .await?;
                if self
                    .manager
                    .property::<String>(active.as_str(), ACTIVE, "Uuid")
                    .await?
                    != uuid
                {
                    return Ok(false);
                }
                let ip: ObjectPath = self
                    .manager
                    .property(device.as_str(), DEVICE, "Ip4Config")
                    .await?;
                let addresses: Vec<HashMap<String, glib::Variant>> = self
                    .manager
                    .property(
                        ip.as_str(),
                        "org.freedesktop.NetworkManager.IP4Config",
                        "AddressData",
                    )
                    .await?;
                let route = fs::read_to_string("/proc/net/route")?
                    .lines()
                    .skip(1)
                    .any(|line| {
                        let f: Vec<_> = line.split_whitespace().collect();
                        f.len() > 3
                            && f[0] == interface
                            && f[1] == "00000000"
                            && u32::from_str_radix(f[3], 16).is_ok_and(|flags| flags & 1 != 0)
                    });
                Ok::<_, Box<dyn std::error::Error>>(!addresses.is_empty() && route)
            }
            .await;
            if matches!(check, Ok(true)) {
                return Ok(());
            }
            if self.interrupted.get() {
                return Err(err("Stealth change interrupted"));
            }
            glib::timeout_future(Duration::from_secs(1)).await;
        }
        Err(err("Wi-Fi did not obtain a fresh IPv4 activation"))
    }
    async fn restore(&mut self, state: &State, require_network: bool) -> Result<Restored> {
        self.disconnect(&state.interface).await?;
        restore_link_mac(&state.interface, &state.original_mac)?;
        let activated = Self::activate_saved(&self.manager, &self.paths, state).await?;
        let mut restored = Restored {
            network: activated
                && self
                    .wait_network(&state.interface, &state.connection)
                    .await
                    .is_ok(),
            unmanaged: false,
        };
        if mac_bytes(&current_mac(&state.interface)?) != mac_bytes(&state.original_mac)
            && !require_network
        {
            let _ = self.disconnect(&state.interface).await;
            let device = self.manager.device(&state.interface).await?;
            bus::set_property(
                &self.manager.connection,
                network::NM,
                device.as_str(),
                DEVICE,
                "Managed",
                &false.to_variant(),
            )
            .await?;
            restore_link_mac(&state.interface, &state.original_mac)?;
            restored.network = false;
            restored.unmanaged = true;
        }
        if mac_bytes(&current_mac(&state.interface)?) != mac_bytes(&state.original_mac) {
            return Err(err("Original MAC address was not restored"));
        }
        if require_network && !restored.network {
            return Err(err("Saved Wi-Fi profile could not reconnect"));
        }
        Ok(restored)
    }
    async fn delete_clone(&mut self, state: &State) -> Result<()> {
        if !state.clone_uuid.is_empty()
            && let Some(profile) = self.manager.by_uuid(&state.clone_uuid).await?
        {
            self.manager.delete(profile.as_str()).await?;
            return Ok(());
        }
        for profile in self.manager.profiles().await? {
            if network::text(
                &self.manager.settings(profile.as_str()).await?,
                "connection",
                "id",
            ) == Some(&state.clone_name)
            {
                self.manager.delete(profile.as_str()).await?;
            }
        }
        Ok(())
    }
    async fn unit_state(&mut self, unit: &str) -> Result<String> {
        let reply = bus::call(
            &self.manager.connection,
            SYSTEMD,
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            "LoadUnit",
            Some(&(unit,).to_variant()),
            5000,
        )
        .await?;
        let (path,) = reply
            .get::<(ObjectPath,)>()
            .ok_or_else(|| err("Invalid systemd unit path"))?;
        bus::property(
            &self.manager.connection,
            SYSTEMD,
            path.as_str(),
            "org.freedesktop.systemd1.Unit",
            "ActiveState",
        )
        .await
    }
    async fn unit_change(&mut self, unit: &str, start: bool) -> Result<()> {
        bus::call(
            &self.manager.connection,
            SYSTEMD,
            SYSTEMD_PATH,
            SYSTEMD_MANAGER,
            if start { "StartUnit" } else { "StopUnit" },
            Some(&(unit, "replace").to_variant()),
            5000,
        )
        .await?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let state = self.unit_state(unit).await?;
            if (start && state == "active")
                || (!start && matches!(state.as_str(), "inactive" | "failed"))
            {
                return Ok(());
            }
            if start && state == "failed" {
                return Err(err("Tor service failed to start"));
            }
            if Instant::now() >= deadline {
                return Err(err("Tor service change timed out"));
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }
    }
    async fn listeners(&mut self) -> Result<bool> {
        tor_listeners()
    }
    async fn verify(&mut self, attempts: u32) -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        for attempt in 0..attempts {
            if self.interrupted.get() {
                return Err(err("Stealth change interrupted"));
            }
            let result = (|| {
                let response = client.get(CHECK_URL).send()?.error_for_status()?;
                let mut text = String::new();
                response.take(65537).read_to_string(&mut text)?;
                if text.len() > 65536 {
                    return Err(err("Tor verification response is too large"));
                }
                verified_ip(&text)
            })();
            match result {
                Ok(ip) => return Ok(ip),
                Err(error) => self
                    .paths
                    .log(&format!("Transparent Tor check failed: {error}")),
            }
            if attempt + 1 < attempts {
                glib::timeout_future(Duration::from_secs(2)).await;
            }
        }
        Err(err("A regular TCP request was not verified through Tor"))
    }
}

struct Engine<P> {
    platform: P,
    paths: Paths,
}
impl<P: Platform> Engine<P> {
    fn checkpoint(&self) -> Result<()> {
        if self.platform.interrupted() {
            Err(err("Stealth change interrupted"))
        } else {
            Ok(())
        }
    }
    async fn start(&mut self) -> Result<()> {
        if !self.paths.rules.is_file() {
            return Err(err("Missing firewall rules: /etc/stealth.nft"));
        }
        if self.paths.state().exists() {
            return Err(err("Saved state already exists. Run stealth-stop first"));
        }
        if self.platform.firewall().await? {
            return Err(err(
                "Stealth firewall already exists. Run stealth-stop first",
            ));
        }
        let interface = self.platform.interface().await?;
        if !valid_interface(&interface) {
            return Err(err("No valid default IPv4 interface"));
        }
        let (link, connection) = self.platform.connection(&interface).await?;
        let original_mac = self.platform.mac(&interface).await?;
        if mac_bytes(&original_mac).is_none() || !valid_uuid(&connection) {
            return Err(err("Invalid active network state"));
        }
        let state = State {
            interface,
            link,
            original_mac: original_mac.clone(),
            tor_preexisting: self.platform.unit_state("tor.service").await? == "active",
            connection,
            clone_uuid: if link == Link::Wifi {
                random_uuid()?
            } else {
                String::new()
            },
            clone_name: if link == Link::Wifi {
                format!("stealth-{}", random_uuid()?)
            } else {
                String::new()
            },
            spoof_mac: if link == Link::Wifi {
                random_mac(&original_mac)?
            } else {
                String::new()
            },
        };
        self.paths.prepare()?;
        self.paths.write_state(&state)?;
        let mut mac_attempted = false;
        let mut tor_started = false;
        let result=async{
            self.paths.log(&format!("Installing fail-closed firewall before changing {}.",state.interface));
            self.platform.install_firewall(&state.interface).await?;self.checkpoint()?;
            if state.link==Link::Wifi{
                self.platform.clone_wifi(&state).await?;self.checkpoint()?;
                mac_attempted=true;self.platform.disconnect(&state.interface).await?;self.checkpoint()?;
                self.platform.activate(&state.interface,&state.clone_uuid).await?;self.checkpoint()?;
                self.platform.wait_network(&state.interface,&state.clone_uuid).await?;
            }else{self.platform.wait_network(&state.interface,&state.connection).await?;}
            self.checkpoint()?;
            let mac=self.platform.mac(&state.interface).await?;let expected=if state.link==Link::Wifi{&state.spoof_mac}else{&state.original_mac};
            if mac_bytes(&mac)!=mac_bytes(expected){return Err(err("NetworkManager did not apply the expected MAC address"))}
            if self.platform.interface().await?!=state.interface{return Err(err("The default IPv4 route changed during startup"))}
            if !state.tor_preexisting{tor_started=true;self.platform.unit_change("tor.service",true).await?;}
            self.checkpoint()?;
            if self.platform.unit_state("tor.service").await?!="active"{return Err(err("Tor service is inactive"))}
            if !self.platform.listeners().await?{return Err(err("Tor transparent TCP and DNS listeners are missing"))}
            let ip=self.platform.verify(3).await?;self.checkpoint()?;
            self.paths.log(&format!("Stealth active: {} {} MAC {mac}; Tor exit {ip}.",state.link.name(),state.interface));
            self.paths.log("Host IPv4 TCP and DNS use Tor; IPv6, other UDP, ICMP, and forwarded traffic are blocked.");
            runtime::atomic_write(&self.paths.active(),b"")?;fs::set_permissions(self.paths.active(),fs::Permissions::from_mode(0o644))?;
            Ok::<_,Box<dyn std::error::Error>>(())
        }.await;
        if result.is_err() {
            let _ = remove_if_exists(&self.paths.active());
            self.paths
                .log("Start failed; attempting to restore the network and Tor state.");
            if mac_attempted && let Err(error) = self.platform.restore(&state, true).await {
                self.paths.log(&format!(
                    "MAC restoration failed; manual recovery is required: {error}"
                ));
            }
            if tor_started && let Err(error) = self.platform.unit_change("tor.service", false).await
            {
                self.paths
                    .log(&format!("Tor could not be stopped: {error}"));
            }
            match self.platform.firewall().await {
                Ok(false) => {
                    if state.link == Link::Wifi
                        && let Err(error) = self.platform.delete_clone(&state).await
                    {
                        self.paths.log(&format!(
                            "Temporary Wi-Fi profile could not be removed: {error}"
                        ));
                        return result;
                    }
                    self.paths.clear()?;
                }
                _ => self.paths.log(
                    "The firewall remains active to block leaks. Run stealth-stop to recover.",
                ),
            }
        }
        result
    }
    async fn stop(&mut self) -> Result<()> {
        let firewall = self.platform.firewall().await?;
        if !self.paths.state().exists() && !firewall {
            remove_if_exists(&self.paths.active())?;
            let _ = fs::remove_dir(&self.paths.directory);
            self.paths.log("Stealth is already stopped.");
            return Ok(());
        }
        remove_if_exists(&self.paths.active())?;
        let state = self.paths.read_state().map_err(|e| {
            err(format!(
                "{e}; keeping the firewall active to avoid an unsafe restore"
            ))
        })?;
        let restored = if state.link == Link::Wifi {
            self.paths.log(&format!(
                "Restoring {} to its saved MAC {}.",
                state.interface, state.original_mac
            ));
            let restored = self.platform.restore(&state, false).await?;
            self.checkpoint()?;
            self.platform.delete_clone(&state).await?;
            restored
        } else {
            self.paths.log(&format!(
                "Ethernet connection {} and its MAC were not changed.",
                state.interface
            ));
            Restored {
                network: true,
                unmanaged: false,
            }
        };
        self.checkpoint()?;
        if !state.tor_preexisting && self.platform.unit_state("tor.service").await? == "active" {
            self.platform.unit_change("tor.service", false).await?;
        }
        self.checkpoint()?;
        if firewall {
            self.platform.remove_firewall().await?;
        }
        self.paths.clear()?;
        if restored.network {
            self.paths
                .log("Stealth stopped; ordinary routing restored.");
        } else {
            self.paths.log("Stealth stopped; original MAC restored, but Wi-Fi is unavailable. Reconnect the original profile when the network returns.");
            if restored.unmanaged {
                self.paths.log(&format!("NetworkManager control was disabled for MAC recovery on {}. Re-enable the device in Network Connections when its network returns.",state.interface));
            }
        }
        Ok(())
    }
    async fn status(&mut self) -> Result<()> {
        let service = self.platform.unit_state("stealth.service").await?;
        let tor = self.platform.unit_state("tor.service").await?;
        let default = self.platform.interface().await.unwrap_or_default();
        let saved = if self.paths.state().exists() {
            Some(self.paths.read_state()?)
        } else {
            None
        };
        let (interface, link) = if let Some(state) = &saved {
            (state.interface.clone(), Some(state.link))
        } else {
            (
                default.clone(),
                self.platform.connection(&default).await.ok().map(|v| v.0),
            )
        };
        let mac = self
            .platform
            .mac(&interface)
            .await
            .unwrap_or_else(|_| "unavailable".into());
        println!(
            "Stealth service: {service}\nTor service: {tor}\nInterface: {}\nConnection type: {}\nCurrent MAC: {mac}\nDefault IPv4 interface: {}",
            if interface.is_empty() {
                "unavailable"
            } else {
                &interface
            },
            link.map(Link::name).unwrap_or("unavailable"),
            if default.is_empty() {
                "unavailable"
            } else {
                &default
            }
        );
        if service != "active" && link.is_none() {
            println!("Start requirement: NetworkManager Wi-Fi or Ethernet default IPv4 route.");
        }
        let firewall = self.platform.firewall().await?;
        let mut healthy = true;
        if let Some(state) = &saved {
            println!("Saved pre-start MAC: {}", state.original_mac);
            let expected = if state.link == Link::Wifi {
                println!("Expected spoofed MAC: {}", state.spoof_mac);
                &state.spoof_mac
            } else {
                println!("MAC spoofing: off (Ethernet)");
                &state.original_mac
            };
            if firewall && mac_bytes(&mac) != mac_bytes(expected) {
                println!("Warning: the expected saved MAC is not active.");
                healthy = false;
            }
        } else if firewall || service == "active" {
            println!("Warning: saved MAC state is missing; safe stop is unavailable.");
            healthy = false;
        }
        if firewall {
            println!("Fail-closed firewall: active");
            if !self.paths.active().is_file() {
                println!("Warning: Stealth routing is not confirmed active.");
                healthy = false;
            }
            if tor == "active" && self.platform.listeners().await? {
                match self.platform.verify(1).await {
                    Ok(ip) => println!("Transparent Tor check: verified ({ip})"),
                    Err(_) => {
                        println!("Transparent Tor check: FAILED; traffic remains blocked");
                        healthy = false;
                    }
                }
            } else {
                println!("Transparent Tor check: unavailable; traffic remains blocked");
                healthy = false;
            }
        } else {
            println!("Fail-closed firewall: inactive");
            if saved.is_some() {
                println!("Warning: saved state exists without its stealth firewall.");
                healthy = false;
            }
            if service == "active" {
                println!("Warning: service is active without its stealth firewall.");
                healthy = false;
            }
        }
        println!(
            "Scope: host IPv4 TCP and DNS through Tor while active; IPv6, other UDP, ICMP, and forwarded traffic blocked."
        );
        if healthy {
            Ok(())
        } else {
            Err(err("Stealth health check failed"))
        }
    }
}

struct RootLock(File);
impl RootLock {
    fn acquire() -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/run/stealth.lock")?;
        if !file.metadata()?.is_file() || file.metadata()?.uid() != 0 {
            return Err(err("Unsafe Stealth lock file"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(err("Another Stealth change is in progress"));
        }
        Ok(Self(file))
    }
}
impl Drop for RootLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub async fn run(args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("");
    if action == "version" {
        println!("stealth-transparent-v2");
        return Ok(());
    }
    if !matches!(action, "start" | "stop" | "status") {
        return Err(err("Usage: stealth {start|stop|status}"));
    }
    if unsafe { libc::geteuid() } != 0 {
        return Err(err("Run this command with elevated privileges"));
    }
    let paths = Paths::system();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&paths.log)?;
    if !log.metadata()?.is_file() || log.metadata()?.uid() != 0 {
        return Err(err("Unsafe Stealth log file"));
    }
    log.set_permissions(fs::Permissions::from_mode(0o600))?;
    let _lock = if action != "status" {
        Some(RootLock::acquire()?)
    } else {
        None
    };
    let interrupted = Rc::new(Cell::new(false));
    let mut signals = Vec::new();
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let interrupted = interrupted.clone();
        signals.push(glib_unix::unix_signal_add_local(signal, move || {
            interrupted.set(true);
            glib::ControlFlow::Continue
        }));
    }
    let result = async {
        let platform = Native {
            manager: Manager::new().await?,
            nft: Nft::new()?,
            paths: paths.clone(),
            interrupted,
        };
        let mut engine = Engine { platform, paths };
        match action {
            "start" => engine.start().await,
            "stop" => engine.stop().await,
            _ => engine.status().await,
        }
    }
    .await;
    for signal in signals {
        signal.remove();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    const ORIGINAL: &str = "98:bd:80:57:0e:ae";
    const UUID: &str = "64595948-f950-4cf0-8a80-d234a3cb4448";
    fn state(link: Link) -> State {
        State {
            interface: if link == Link::Wifi {
                "wlan0"
            } else {
                "enp0s13f0u3"
            }
            .into(),
            link,
            original_mac: ORIGINAL.into(),
            tor_preexisting: false,
            connection: UUID.into(),
            clone_uuid: if link == Link::Wifi {
                UUID.into()
            } else {
                String::new()
            },
            clone_name: if link == Link::Wifi {
                format!("stealth-{UUID}")
            } else {
                String::new()
            },
            spoof_mac: if link == Link::Wifi {
                "02:11:22:33:44:55".into()
            } else {
                String::new()
            },
        }
    }
    #[test]
    fn state_is_exactly_eight_lines_and_reads_legacy_startup_state() {
        for link in [Link::Wifi, Link::Ethernet] {
            let s = state(link);
            assert_eq!(s.encode().split_terminator('\n').count(), 8);
            assert_eq!(State::decode(&s.encode()).unwrap(), s);
        }
        let mut s = state(Link::Wifi);
        s.clone_uuid.clear();
        assert_eq!(State::decode(&s.encode()).unwrap(), s);
    }
    #[test]
    fn state_rejects_invalid_recovery_data() {
        let s = state(Link::Wifi);
        for text in [
            s.encode().replace("wlan0", "../../etc"),
            s.encode().replace(ORIGINAL, "not-a-mac"),
            s.encode().replace("\nwifi\n", "\nbridge\n"),
            s.encode() + "extra\n",
            s.encode().replace("stealth-", "evil-"),
        ] {
            assert!(State::decode(&text).is_err(), "{text}");
        }
        let mut wired = state(Link::Ethernet);
        wired.clone_uuid = UUID.into();
        assert!(State::decode(&wired.encode()).is_err());
    }
    #[test]
    fn random_addresses_are_local_unicast_and_uuid_v4() {
        for _ in 0..16 {
            let mac = random_mac(ORIGINAL).unwrap();
            assert_eq!(mac_bytes(&mac).unwrap()[0] & 3, 2);
            assert_ne!(mac, ORIGINAL);
            let uuid = random_uuid().unwrap();
            assert!(valid_uuid(&uuid));
            assert_eq!(&uuid[14..15], "4");
        }
    }
    #[test]
    fn mac_policy_uses_native_assigned_string_and_preserves_empty_legacy() {
        let mut settings = Settings::new();
        settings.insert(WIFI.into(), HashMap::new());
        assert_eq!(mac_policy(&settings).unwrap(), "");
        settings
            .get_mut(WIFI)
            .unwrap()
            .insert("cloned-mac-address".into(), Vec::<u8>::new().to_variant());
        assert_eq!(mac_policy(&settings).unwrap(), "");
        settings.get_mut(WIFI).unwrap().insert(
            "cloned-mac-address".into(),
            vec![0x98_u8, 0xbd, 0x80, 0x57, 0x0e, 0xae].to_variant(),
        );
        assert_eq!(mac_policy(&settings).unwrap(), ORIGINAL);
        set_mac_policy(&mut settings, "random");
        assert_eq!(mac_policy(&settings).unwrap(), "random");
        assert!(!settings[WIFI].contains_key("cloned-mac-address"));
        set_mac_policy(&mut settings, "");
        assert_eq!(mac_policy(&settings).unwrap(), "");
    }
    #[test]
    fn saved_profile_restoration_needs_no_secret_agent_and_resets_policy_after_failure() {
        use gtk::gio;
        use std::{cell::RefCell, os::unix::net::UnixStream};
        run(async {
            let (server_socket, client_socket) = UnixStream::pair().unwrap();
            let server_stream = gio::Socket::from_fd(server_socket.into())
                .unwrap()
                .connection_factory_create_connection();
            let client_stream = gio::Socket::from_fd(client_socket.into())
                .unwrap()
                .connection_factory_create_connection();
            let guid = gio::dbus_generate_guid();
            let server = glib::spawn_future_local(gio::DBusConnection::new_future(
                &server_stream,
                Some(&guid),
                gio::DBusConnectionFlags::AUTHENTICATION_SERVER
                    | gio::DBusConnectionFlags::DELAY_MESSAGE_PROCESSING,
                None,
            ));
            let client = gio::DBusConnection::new_future(
                &client_stream,
                None,
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT,
                None,
            )
            .await
            .unwrap();
            let server = server.await.unwrap().unwrap();
            server.set_exit_on_close(false);
            client.set_exit_on_close(false);
            let xml = gio::DBusNodeInfo::for_xml(r#"<node>
              <interface name="org.freedesktop.NetworkManager.Settings">
                <method name="ListConnections"><arg type="ao" direction="out"/></method>
              </interface>
              <interface name="org.freedesktop.NetworkManager.Settings.Connection">
                <method name="GetSettings"><arg type="a{sa{sv}}" direction="out"/></method>
                <method name="UpdateUnsaved"><arg type="a{sa{sv}}" direction="in"/></method>
                <method name="GetSecrets"><arg type="s" direction="in"/><arg type="a{sa{sv}}" direction="out"/></method>
              </interface>
              <interface name="org.freedesktop.NetworkManager">
                <method name="GetDeviceByIpIface"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
                <method name="ActivateConnection"><arg type="o" direction="in"/><arg type="o" direction="in"/><arg type="o" direction="in"/><arg type="o" direction="out"/></method>
              </interface>
              <interface name="org.freedesktop.NetworkManager.Connection.Active">
                <property name="State" type="u" access="read"/>
              </interface>
            </node>"#).unwrap();
            let profile_path = "/org/freedesktop/NetworkManager/Settings/1";
            let active_path = "/org/freedesktop/NetworkManager/ActiveConnection/1";
            let mut settings = Settings::new();
            settings.insert(
                "connection".into(),
                HashMap::from([("uuid".into(), UUID.to_variant())]),
            );
            settings.insert(
                WIFI.into(),
                HashMap::from([("assigned-mac-address".into(), "random".to_variant())]),
            );
            settings.insert(
                "802-11-wireless-security".into(),
                HashMap::from([
                    ("key-mgmt".into(), "wpa-psk".to_variant()),
                    ("psk-flags".into(), 1_u32.to_variant()),
                ]),
            );
            let current = Rc::new(RefCell::new(settings));
            let updates = Rc::new(RefCell::new(Vec::<Settings>::new()));
            let secret_requests = Rc::new(Cell::new(0));
            let activation_fails = Rc::new(Cell::new(false));
            let mut registrations = Vec::new();
            registrations.push(
                server
                    .register_object(
                        network::SETTINGS,
                        &xml.lookup_interface("org.freedesktop.NetworkManager.Settings")
                            .unwrap(),
                    )
                    .method_call(move |_, _, _, _, method, _, invocation| {
                        assert_eq!(method, "ListConnections");
                        invocation.return_value(Some(
                            &(vec![network::path(profile_path).unwrap()],).to_variant(),
                        ));
                    })
                    .build()
                    .unwrap(),
            );
            let fixture_settings = current.clone();
            let changed = updates.clone();
            let secrets = secret_requests.clone();
            registrations.push(
                server
                    .register_object(profile_path, &xml.lookup_interface(CONNECTION).unwrap())
                    .method_call(move |_, _, _, _, method, args, invocation| match method {
                        "GetSettings" => invocation
                            .return_value(Some(&(fixture_settings.borrow().clone(),).to_variant())),
                        "UpdateUnsaved" => {
                            let (settings,) = args.get::<(Settings,)>().unwrap();
                            changed.borrow_mut().push(settings.clone());
                            *fixture_settings.borrow_mut() = settings;
                            invocation.return_value(Some(&().to_variant()));
                        }
                        "GetSecrets" => {
                            secrets.set(secrets.get() + 1);
                            invocation.return_dbus_error(
                                "org.freedesktop.NetworkManager.Error.NoSecrets",
                                "Secret agent is unavailable",
                            );
                        }
                        _ => panic!("Unexpected profile method {method}"),
                    })
                    .build()
                    .unwrap(),
            );
            let fails = activation_fails.clone();
            registrations.push(
                server
                    .register_object(network::ROOT, &xml.lookup_interface(network::NM).unwrap())
                    .method_call(move |_, _, _, _, method, _, invocation| match method {
                        "GetDeviceByIpIface" => invocation.return_value(Some(
                            &(network::path("/org/freedesktop/NetworkManager/Devices/1").unwrap(),)
                                .to_variant(),
                        )),
                        "ActivateConnection" if fails.get() => invocation.return_dbus_error(
                            "org.freedesktop.NetworkManager.Error.ConnectionNotAvailable",
                            "Wi-Fi unavailable",
                        ),
                        "ActivateConnection" => invocation.return_value(Some(
                            &(network::path(active_path).unwrap(),).to_variant(),
                        )),
                        _ => panic!("Unexpected manager method {method}"),
                    })
                    .build()
                    .unwrap(),
            );
            registrations.push(
                server
                    .register_object(active_path, &xml.lookup_interface(ACTIVE).unwrap())
                    .property(|_, _, _, _, property| {
                        assert_eq!(property, "State");
                        2_u32.to_variant()
                    })
                    .build()
                    .unwrap(),
            );
            server.start_message_processing();
            let manager = Manager {
                connection: client.clone(),
            };
            let (_root, engine) = fixture(Link::Wifi);
            engine.paths.prepare().unwrap();
            let saved = state(Link::Wifi);
            for fail in [false, true] {
                activation_fails.set(fail);
                assert_eq!(
                    Native::activate_saved(&manager, &engine.paths, &saved)
                        .await
                        .unwrap(),
                    !fail
                );
                assert_eq!(mac_policy(&current.borrow()).unwrap(), "random");
                assert!(!engine.paths.policy().exists());
            }
            assert_eq!(secret_requests.get(), 0);
            {
                let updates = updates.borrow();
                assert_eq!(updates.len(), 4);
                for pair in updates.as_chunks::<2>().0 {
                    assert_eq!(mac_policy(&pair[0]).unwrap(), ORIGINAL);
                    assert_eq!(mac_policy(&pair[1]).unwrap(), "random");
                    for settings in pair {
                        let security = &settings["802-11-wireless-security"];
                        assert_eq!(security["psk-flags"].get::<u32>(), Some(1));
                        assert!(!security.contains_key("psk"));
                    }
                }
            }
            for registration in registrations {
                server.unregister_object(registration).unwrap();
            }
            client.close_future().await.unwrap();
            let _ = server.close_future().await;
        });
    }
    #[test]
    fn listeners_require_ipv4_loopback_and_correct_socket_state() {
        let tcp = "header\n0: 0100007F:2350 00000000:0000 0A\n";
        assert!(socket_present(tcp, 9040, true));
        assert!(!socket_present(
            &tcp.replace("0100007F", "00000000"),
            9040,
            true
        ));
        assert!(!socket_present(&tcp.replace("0A", "01"), 9040, true));
        assert!(socket_present(
            "header\n0: 0100007F:14E9 00000000:0000 07\n",
            5353,
            false
        ));
    }
    #[test]
    fn verification_rejects_non_tor_and_malformed_exit_ip() {
        assert_eq!(
            verified_ip(r#"{"IsTor":true,"IP":"203.0.113.10"}"#).unwrap(),
            "203.0.113.10"
        );
        for value in [
            r#"{"IsTor":false,"IP":"203.0.113.10"}"#,
            r#"{"IsTor":true,"IP":"invalid"}"#,
            r#"{"IsTor":true,"IP":"::1"}"#,
            "garbage",
        ] {
            assert!(verified_ip(value).is_err());
        }
    }
    #[test]
    fn route_parser_checks_lengths_and_extracts_native_oif() {
        let mut bytes = vec![0_u8; 36];
        bytes[..4].copy_from_slice(&36_u32.to_ne_bytes());
        bytes[4..6].copy_from_slice(&libc::RTM_NEWROUTE.to_ne_bytes());
        bytes[16] = libc::AF_INET as u8;
        bytes[28..30].copy_from_slice(&8_u16.to_ne_bytes());
        bytes[30..32].copy_from_slice(&libc::RTA_OIF.to_ne_bytes());
        bytes[32..36].copy_from_slice(&7_u32.to_ne_bytes());
        assert_eq!(interface_for_route_reply(&bytes).unwrap(), 7);
        assert!(interface_for_route_reply(&bytes[..32]).is_err());
        bytes[28..30].copy_from_slice(&2_u16.to_ne_bytes());
        assert!(interface_for_route_reply(&bytes).is_err());
    }

    struct Fake {
        link: Link,
        firewall: bool,
        tor: bool,
        clone: bool,
        mac: String,
        pending_mac: String,
        events: Vec<String>,
        fail: Option<&'static str>,
        offline: bool,
        interrupted: bool,
        interrupt_after_install: bool,
    }
    impl Fake {
        fn new(link: Link) -> Self {
            Self {
                link,
                firewall: false,
                tor: false,
                clone: false,
                mac: ORIGINAL.into(),
                pending_mac: String::new(),
                events: Vec::new(),
                fail: None,
                offline: false,
                interrupted: false,
                interrupt_after_install: false,
            }
        }
        fn event(&mut self, name: &str) -> Result<()> {
            self.events.push(name.into());
            if self.fail == Some(name) {
                Err(err(format!("Injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }
    impl Platform for Fake {
        fn interrupted(&self) -> bool {
            self.interrupted
        }
        async fn firewall(&mut self) -> Result<bool> {
            Ok(self.firewall)
        }
        async fn install_firewall(&mut self, _: &str) -> Result<()> {
            self.event("firewall-install")?;
            self.firewall = true;
            if self.interrupt_after_install {
                self.interrupted = true;
            }
            Ok(())
        }
        async fn remove_firewall(&mut self) -> Result<()> {
            self.event("firewall-remove")?;
            self.firewall = false;
            Ok(())
        }
        async fn interface(&mut self) -> Result<String> {
            Ok(if self.link == Link::Wifi {
                "wlan0"
            } else {
                "enp0s13f0u3"
            }
            .into())
        }
        async fn connection(&mut self, _: &str) -> Result<(Link, String)> {
            Ok((self.link, UUID.into()))
        }
        async fn mac(&mut self, _: &str) -> Result<String> {
            Ok(self.mac.clone())
        }
        async fn clone_wifi(&mut self, s: &State) -> Result<()> {
            self.event("clone")?;
            self.clone = true;
            self.pending_mac = s.spoof_mac.clone();
            Ok(())
        }
        async fn disconnect(&mut self, _: &str) -> Result<()> {
            self.event("disconnect")
        }
        async fn activate(&mut self, _: &str, _: &str) -> Result<()> {
            self.event("activate")?;
            self.mac = self.pending_mac.clone();
            Ok(())
        }
        async fn wait_network(&mut self, _: &str, _: &str) -> Result<()> {
            self.event("network")
        }
        async fn restore(&mut self, _: &State, required: bool) -> Result<Restored> {
            self.event("restore")?;
            self.mac = ORIGINAL.into();
            if required && self.offline {
                return Err(err("Offline"));
            }
            Ok(Restored {
                network: !self.offline,
                unmanaged: self.offline,
            })
        }
        async fn delete_clone(&mut self, _: &State) -> Result<()> {
            self.event("delete-clone")?;
            self.clone = false;
            Ok(())
        }
        async fn unit_state(&mut self, unit: &str) -> Result<String> {
            Ok(if unit == "tor.service" && self.tor {
                "active"
            } else {
                "inactive"
            }
            .into())
        }
        async fn unit_change(&mut self, _: &str, start: bool) -> Result<()> {
            self.event(if start { "tor-start" } else { "tor-stop" })?;
            self.tor = start;
            Ok(())
        }
        async fn listeners(&mut self) -> Result<bool> {
            Ok(self.tor)
        }
        async fn verify(&mut self, _: u32) -> Result<String> {
            self.event("verify")?;
            Ok("203.0.113.10".into())
        }
    }
    fn fixture(link: Link) -> (tempfile::TempDir, Engine<Fake>) {
        let root = tempfile::tempdir().unwrap();
        let rules = root.path().join("rules");
        fs::write(&rules, "add table inet stealth\n").unwrap();
        let paths = Paths {
            directory: root.path().join("run"),
            log: root.path().join("log"),
            rules,
        };
        (
            root,
            Engine {
                platform: Fake::new(link),
                paths,
            },
        )
    }
    fn run(future: impl std::future::Future<Output = ()>) {
        glib::MainContext::new().block_on(future)
    }
    #[test]
    fn wifi_start_installs_firewall_before_network_and_stop_removes_it_last() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.start().await.unwrap();
            assert!(engine.paths.active().is_file());
            assert_eq!(engine.paths.read_state().unwrap().link, Link::Wifi);
            assert_eq!(
                &engine.platform.events[..4],
                ["firewall-install", "clone", "disconnect", "activate"]
            );
            engine.stop().await.unwrap();
            assert!(!engine.paths.state().exists());
            assert!(!engine.platform.firewall);
            assert!(!engine.platform.clone);
            assert_eq!(engine.platform.mac, ORIGINAL);
            assert_eq!(
                &engine.platform.events[engine.platform.events.len() - 4..],
                ["restore", "delete-clone", "tor-stop", "firewall-remove"]
            );
        });
    }
    #[test]
    fn failed_verification_retains_firewall_state_and_clone_for_recovery() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.platform.fail = Some("verify");
            assert!(engine.start().await.is_err());
            assert!(engine.platform.firewall);
            assert!(engine.platform.clone);
            assert!(engine.paths.state().is_file());
            assert!(!engine.paths.active().exists());
            assert_eq!(engine.platform.mac, ORIGINAL);
            assert!(!engine.platform.tor);
            engine.platform.fail = None;
            engine.stop().await.unwrap();
            assert!(!engine.platform.firewall);
        });
    }
    #[test]
    fn preexisting_tor_is_never_stopped_and_ethernet_never_changes_link() {
        run(async {
            let (_root, mut engine) = fixture(Link::Ethernet);
            engine.platform.tor = true;
            engine.start().await.unwrap();
            engine.stop().await.unwrap();
            assert!(engine.platform.tor);
            for operation in [
                "clone",
                "disconnect",
                "activate",
                "restore",
                "delete-clone",
                "tor-start",
                "tor-stop",
            ] {
                assert!(
                    !engine
                        .platform
                        .events
                        .iter()
                        .any(|event| event == operation),
                    "{operation}"
                );
            }
            assert_eq!(engine.platform.mac, ORIGINAL);
        });
    }
    #[test]
    fn missing_or_invalid_state_keeps_existing_firewall() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.platform.firewall = true;
            assert!(engine.stop().await.is_err());
            assert!(engine.platform.firewall);
            engine.paths.prepare().unwrap();
            runtime::atomic_write(&engine.paths.state(), b"broken\n").unwrap();
            assert!(engine.stop().await.is_err());
            assert!(engine.platform.firewall);
            assert!(engine.platform.events.is_empty());
        });
    }
    #[test]
    fn failed_restore_or_clone_deletion_never_removes_firewall() {
        run(async {
            for failure in ["restore", "delete-clone", "tor-stop"] {
                let (_root, mut engine) = fixture(Link::Wifi);
                engine.start().await.unwrap();
                engine.platform.fail = Some(failure);
                assert!(engine.stop().await.is_err());
                assert!(engine.platform.firewall);
                assert!(engine.paths.state().is_file());
                assert!(!engine.paths.active().exists());
                assert!(!engine.platform.events.contains(&"firewall-remove".into()));
            }
        });
    }
    #[test]
    fn clone_failure_and_offline_stop_are_recoverable() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.platform.fail = Some("clone");
            assert!(engine.start().await.is_err());
            assert!(engine.platform.firewall);
            assert!(engine.paths.state().is_file());
            engine.platform.fail = None;
            engine.platform.offline = true;
            engine.stop().await.unwrap();
            assert!(!engine.platform.firewall);
            assert_eq!(engine.platform.mac, ORIGINAL);
            assert!(!engine.paths.state().exists());
        });
    }
    #[test]
    fn interrupted_stop_preserves_recovery_state_and_firewall() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.start().await.unwrap();
            engine.platform.interrupted = true;
            assert!(engine.stop().await.is_err());
            assert!(engine.platform.firewall);
            assert!(engine.paths.state().is_file());
            assert_eq!(engine.platform.mac, ORIGINAL);
        });
    }
    #[test]
    fn interruption_after_firewall_install_retains_state_without_changing_wifi() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.platform.interrupt_after_install = true;
            assert!(engine.start().await.is_err());
            assert!(engine.platform.firewall);
            assert!(engine.paths.state().is_file());
            assert!(!engine.paths.active().exists());
            assert_eq!(engine.platform.events, ["firewall-install"]);
            assert_eq!(engine.platform.mac, ORIGINAL);
            assert!(!engine.platform.tor);
        });
    }
    #[test]
    fn rejected_firewall_batch_discards_unused_state_without_network_changes() {
        run(async {
            let (_root, mut engine) = fixture(Link::Wifi);
            engine.platform.fail = Some("firewall-install");
            assert!(engine.start().await.is_err());
            assert!(!engine.platform.firewall);
            assert!(!engine.paths.state().exists());
            assert!(!engine.paths.active().exists());
            assert_eq!(engine.platform.mac, ORIGINAL);
            assert!(
                !engine
                    .platform
                    .events
                    .iter()
                    .any(|event| event == "disconnect")
            );
        });
    }
    #[test]
    fn failed_start_preserves_preexisting_tor_for_wifi_and_ethernet() {
        run(async {
            for link in [Link::Wifi, Link::Ethernet] {
                let (_root, mut engine) = fixture(link);
                engine.platform.tor = true;
                engine.platform.fail = Some("verify");
                assert!(engine.start().await.is_err());
                assert!(engine.platform.tor);
                assert!(engine.platform.firewall);
                assert!(engine.paths.state().is_file());
                assert!(
                    !engine
                        .platform
                        .events
                        .iter()
                        .any(|event| event == "tor-stop")
                );
                engine.platform.fail = None;
                engine.stop().await.unwrap();
                assert!(engine.platform.tor);
            }
        });
    }
}
