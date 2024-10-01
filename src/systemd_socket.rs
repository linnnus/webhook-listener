//! `systemd_socket` implements the daemon side of the socket activation. The interface is similar
//! to the one provided by the systemd/sd-daemon library, but adjusted for easier usage in rust. It
//! relies on `nix` for all low-level operations. All checks are ported from the systemd code.
//!
//! Enums required for socket type (`SockType`) and address family (`AddressFamily`) are reexported
//! from nix.
//!
//! The library is based on [rust-systemd](https://github.com/jmesmon/rust-systemd) by Cody P
//! Schafer, but it does not require any extra libraries and works on rust stable.

// I'm hoping to bring this module with me to other packages, so let's just allow all the functions
// which _are_ useful, just not for this project. That's why there are a lot of `allow(dead_code)`
// in this module.

use nix::fcntl;
use nix::libc;
use nix::sys::socket::{self, SockaddrLike};
use nix::sys::stat;
use nix::unistd::Pid;
use std::collections::HashMap;
use std::convert::From;
use std::env;
use std::error::Error as StdError;
use std::fmt;
use std::num::ParseIntError;
use std::os::unix::io::{OwnedFd, RawFd};
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::path;

pub use nix::sys::socket::SockType;
pub use nix::sys::socket::AddressFamily;

const VAR_FDS: &'static str = "LISTEN_FDS";
const VAR_NAMES: &'static str = "LISTEN_FDNAMES";
const VAR_PID: &'static str = "LISTEN_PID";

#[derive(Debug, PartialEq)]
pub enum Error {
    Var(env::VarError),
    Parse(ParseIntError),
    DifferentProcess,
    InvalidVariableValue,
    Nix(nix::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match self {
            &Error::InvalidVariableValue => "Environment variable could not be parsed",
            &Error::DifferentProcess =>
                "Environment variables are meant for a different process (pid mismatch)",
            &Error::Var(_) => "Required environment variable missing or unreadable",
            &Error::Parse(_) => "Could not parse number in 'LISTEN_FDS'",
            &Error::Nix(_) => "Calling system function on socket failed",
        }
    }

    fn cause(&self) -> Option<&dyn StdError> {
        match self {
            &Error::Var(ref e) => Some(e),
            &Error::Parse(ref e) => Some(e),
            &Error::Nix(ref e) => Some(e),
            _ => None,
        }
    }
}

impl From<env::VarError> for Error {
    fn from(e: env::VarError) -> Error {
        Error::Var(e)
    }
}

impl From<ParseIntError> for Error {
    fn from(e: ParseIntError) -> Error {
        Error::Parse(e)
    }
}

impl From<nix::Error> for Error {
    fn from(e: nix::Error) -> Error {
        Error::Nix(e)
    }
}

/// Encapsulates the possible failure modes of local functions.
pub type Result<T> = std::result::Result<T, Error>;

/// Number of the first passed file descriptor
const LISTEN_FDS_START: RawFd = 3;

fn unset_all_env() {
    env::remove_var(VAR_PID);
    env::remove_var(VAR_FDS);
    env::remove_var(VAR_NAMES);
}

/// Returns the file descriptors passed in by init process. Removes the `$LISTEN_FDS` and
/// `$LISTEN_PID` variables from the environment if `unset_environment` is `true`.
pub fn listen_fds(unset_environment: bool) -> Result<Vec<OwnedFd>> {
    let pid_str = env::var(VAR_PID)?;
    let pid_raw: libc::pid_t = pid_str.parse()?;
    let pid = Pid::from_raw(pid_raw);

    if pid != nix::unistd::getpid() {
        return Err(Error::DifferentProcess);
    }

    let fds_str = env::var(VAR_FDS)?;
    let fds: libc::c_int = fds_str.parse()?;

    if fds < 0 {
        return Err(Error::InvalidVariableValue);
    }

    for fd in LISTEN_FDS_START..(LISTEN_FDS_START+fds) {
        fcntl::fcntl(fd, fcntl::FcntlArg::F_SETFD(fcntl::FdFlag::FD_CLOEXEC))?;
    }

    if unset_environment {
        unset_all_env();
    }
    let fd_vec: Vec<_> = (LISTEN_FDS_START .. (LISTEN_FDS_START+fds))
        .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
        .collect();
    Ok(fd_vec)
}

/// Returns file descriptors with names. Removes the `$LISTEN_FDS` and `$LISTEN_PID` variables from
/// the environment if `unset_environment` is `true`.
#[allow(unused)]
pub fn listen_fds_with_names(unset_environment: bool) -> Result<HashMap<String, OwnedFd>> {
    let names_str = env::var(VAR_NAMES)?;
    let names: Vec<&str> = names_str.split(':').collect();

    let fds: Vec<OwnedFd> = listen_fds(unset_environment)?;
    if fds.len() != names.len() {
        return Err(Error::InvalidVariableValue);
    }

    let mut map = HashMap::new();
    for (name, fd) in names.into_iter().zip(fds) {
        map.insert(name.to_string(), fd);
    }
    Ok(map)
}

/// Identifies whether the passed file descriptor is a FIFO. If a path is
/// supplied, the file descriptor must also match the path.
#[allow(unused)]
pub fn is_fifo<T: AsRawFd>(fd: T, path: Option<&str>) -> Result<bool> {
    let fs = stat::fstat(fd.as_raw_fd())?;
    let mode = stat::SFlag::from_bits_truncate(fs.st_mode);
    if !mode.contains(stat::SFlag::S_IFIFO) {
        return Ok(false);
    }
    if let Some(path_str) = path {
        let path_stat = match stat::stat(path::Path::new(path_str)) {
            Ok(x) => x,
            Err(_) => {return Ok(false)},
        };
        return Ok(path_stat.st_dev == fs.st_dev && path_stat.st_ino == fs.st_ino);
    }
    Ok(true)
}

/// Identifies whether the passed file descriptor is a special character device.
/// If a path is supplied, the file descriptor must also match the path.
#[allow(unused)]
pub fn is_special<T: AsRawFd>(fd: T, path: Option<&str>) -> Result<bool> {
    let fs = stat::fstat(fd.as_raw_fd())?;
    let mode = stat::SFlag::from_bits_truncate(fs.st_mode);
    if !mode.contains(stat::SFlag::S_IFREG) && !mode.contains(stat::SFlag::S_IFCHR) {
        // path not comparable
        return Ok(true);
    }

    if let Some(path_str) = path {
        let path_stat = match stat::stat(path::Path::new(path_str)) {
            Ok(x) => x,
            Err(_) => {return Ok(false)},
        };

        let path_mode = stat::SFlag::from_bits_truncate(path_stat.st_mode);
        if (mode & path_mode).contains(stat::SFlag::S_IFREG) {
            return Ok(path_stat.st_dev == fs.st_dev && path_stat.st_ino == fs.st_ino);
        }

        if (mode & path_mode).contains(stat::SFlag::S_IFCHR) {
            return Ok(path_stat.st_rdev == fs.st_rdev);
        }

        return Ok(false);
    }

    Ok(true)
}

/// Do checks common to all socket verification functions. (type, listening state)
#[allow(unused)]
fn is_socket_internal<T: AsFd>(fd: &T, socktype: Option<SockType>,
                      listening: Option<bool>) -> Result<bool> {
    /*if fd < 0 {
        return Err(Error::InvalidFdValue);
    }*/

    let fs = stat::fstat(fd.as_fd().as_raw_fd())?;
    let mode = stat::SFlag::from_bits_truncate(fs.st_mode);
    if !mode.contains(stat::SFlag::S_IFSOCK) {
        return Ok(false);
    }
    if let Some(val) = socktype {
        let typ: SockType = socket::getsockopt(&fd, socket::sockopt::SockType)?;
        if typ != val {
            return Ok(false);
        }
    }

    if let Some(val) = listening {
        // This is broken on MacOS, as according to [getsockopt(2)] and [this stackoverflow
        // anser][so], `SO_ACCEPTCONN` is not
        // supported at the `SOL_SOCKET` level. I assume this also applies to other platforms using
        // the Darwin kernel, i.e. all Apple's platfroms.
        //
        // [getsockopt(2)]: https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getsockopt.2.html
        // [so]: https://stackoverflow.com/a/75943802
        if cfg!(target_vendor = "apple") {
            todo!("Getting listening state is not implemented on Apple's Darwin kernel");
        }

        let acc = socket::getsockopt(&fd, socket::sockopt::AcceptConn)?;
        if acc != val {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Identifies whether the passed file descriptor is a socket. If family,
/// type, and listening state are supplied, they must match as well.
#[allow(unused)]
pub fn is_socket<T: AsFd>(fd: &T, family: Option<AddressFamily>, socktype: Option<SockType>,
                 listening: Option<bool>) -> Result<bool> {
    if !is_socket_internal(fd, socktype, listening)? {
        return Ok(false);
    }

    if let Some(f) = family {
        let sock_addr: socket::SockaddrStorage = socket::getsockname(fd.as_fd().as_raw_fd())?;
        let sock_family = sock_addr.family().unwrap();
        if sock_family != f {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Identifies whether the passed file descriptor is an Internet socket. If family, type, listening
/// state, and/or port are supplied, they must match as well.
pub fn is_socket_inet<T: AsFd>(fd: &T, family: Option<AddressFamily>, socktype: Option<SockType>,
                      listening: Option<bool>, port: Option<u16>) -> Result<bool> {
    if !is_socket_internal(fd, socktype, listening)? {
        return Ok(false);
    }

    let sock_addr: socket::SockaddrStorage = socket::getsockname(fd.as_fd().as_raw_fd())?;
    let sock_family = sock_addr.family().unwrap();
    if sock_family != AddressFamily::Inet && sock_family != AddressFamily::Inet6 {
        return Ok(false);
    }

    if let Some(val) = family {
        if sock_family != val {
            return Ok(false);
        }
    }

    if let Some(expected_port) = port {
        let port = match sock_family {
            socket::AddressFamily::Inet => sock_addr.as_sockaddr_in().unwrap().port(),
            socket::AddressFamily::Inet6 => sock_addr.as_sockaddr_in6().unwrap().port(),
            _ => unreachable!(),
        };
        if port != expected_port {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Identifies whether the passed file descriptor is an AF_UNIX socket. If type are supplied, it
/// must match as well. Path checking is currently unsupported and will be ignored
#[allow(unused)]
pub fn is_socket_unix<T: AsFd>(fd: &T, socktype: Option<SockType>, listening: Option<bool>,
                      path: Option<&str>) -> Result<bool> {
    if !is_socket_internal(fd, socktype, listening)? {
        return Ok(false);
    }

    let sock_addr: socket::SockaddrStorage = socket::getsockname(fd.as_fd().as_raw_fd())?;
    let sock_family = sock_addr.family().unwrap();
    if sock_family != AddressFamily::Unix {
        return Ok(false);
    }

    if let Some(_val) = path {
        // TODO: unsupported
    }

    Ok(true)
}

// TODO
///// Identifies whether the passed file descriptor is a POSIX message queue. If a
///// path is supplied, it will also verify the name.
//pub fn is_mq(fd: RawFd, path: Option<&str>) -> Result<bool> {
//}

#[cfg(test)]
mod tests {
    use ::nix;
    use ::lazy_static::lazy_static;
    use ::std::env;
    use ::std::os::unix::io::OwnedFd;
    use ::std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use ::std::sync::{Mutex,MutexGuard};
    use ::std::mem;

    // Even with one -j1, cargo runs multiple tests at once.  That doesn't work with environment
    // variables, or specific socket ordering, so mutexes are required.
    lazy_static! {
        static ref LOCK: Mutex<()> = Mutex::new(());
    }

    fn lock_env<'a>() -> MutexGuard<'a, ()> {
        // SAFETY: We can ignore `PoisonError`s since the ressource we are locking is just `()`.
        // See: <https://stackoverflow.com/a/51694631>.
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_current_pid() {
        let pid = nix::unistd::getpid();
        env::set_var(super::VAR_PID, format!("{}", pid));
    }

    /// Create a new socket with the given `family` and `typ`e.
    ///
    /// This function is used by the `is_*` tests, so it returns an owned ressource (as opposed to
    /// [`create_socket_with_fd`](self::create_socket_with_fd)).
    fn create_socket(family: super::AddressFamily, typ: super::SockType) -> OwnedFd {
        nix::sys::socket::socket(family, typ, nix::sys::socket::SockFlag::empty(), None).unwrap()
    }

    /// Create a new socket with the given `family` and `typ`e, asserting that it gets assigned a
    /// specific fd.
    ///
    /// This function is used to simulate Systemd opening a socket for us, so the actual ressource
    /// is ["forgotten"](std::mem::forget).
    fn create_socket_with_fd(no: nix::libc::c_int, family: super::AddressFamily, typ: super::SockType) {
        debug_assert!(no > 0, "Valid file descriptors are always positive");

        // Allocate a socket. During normal operation, this would be done by Systemd before our
        // program was even started.
        let fd = create_socket(family, typ);
        assert_eq!(fd.as_raw_fd(), no, "Expected new socket to have fd {} but got {}", no, fd.as_raw_fd());

        // We don't want Rust to manage the ressource for us (YET), as this function is supposed to
        // mimic how Systemd would file descriptors to us.
        mem::forget(fd);
    }

    /// Returns a file descriptor for a regular file.
    fn open_file() -> OwnedFd {
        let path = ::std::path::Path::new("/etc/hosts");
        let fd = nix::fcntl::open(path, nix::fcntl::OFlag::O_RDONLY, nix::sys::stat::Mode::empty()).unwrap();
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    #[test]
    fn listen_fds_success() {
        let _l = lock_env();
        set_current_pid();
        let _fd = create_socket_with_fd(3, super::AddressFamily::Inet, super::SockType::Stream);
        env::set_var(super::VAR_FDS, "1");
        let fds = super::listen_fds(true).unwrap();
        assert_eq!(fds.len(), 1);
        assert_eq!(fds[0].as_raw_fd(), 3);
    }

    #[test]
    fn names() {
        let _l = lock_env();
        set_current_pid();
        env::set_var(super::VAR_FDS, "2");
        env::set_var(super::VAR_NAMES, "a:b");
        let _fd1 = create_socket_with_fd(3, super::AddressFamily::Inet, super::SockType::Stream);
        let _fd2 = create_socket_with_fd(4, super::AddressFamily::Inet, super::SockType::Stream);
        let fds = super::listen_fds_with_names(true).unwrap();
        assert_eq!(fds.len(), 2);
        assert_eq!(fds["a"].as_raw_fd(), 3);
        assert_eq!(fds["b"].as_raw_fd(), 4);
    }

    #[test]
    fn listen_fds_cleans() {
        let _l = lock_env();
        set_current_pid();
        env::set_var(super::VAR_FDS, "0");
        super::listen_fds(false).unwrap();
        assert_eq!(env::var(super::VAR_FDS), Ok("0".into()));
        super::listen_fds(true).unwrap();
        assert_eq!(env::var(super::VAR_FDS), Err(env::VarError::NotPresent));
        assert_eq!(env::var(super::VAR_PID), Err(env::VarError::NotPresent));
        assert_eq!(env::var(super::VAR_NAMES), Err(env::VarError::NotPresent));
    }

    #[test]
    fn is_socket() {
        let _l = lock_env();

        let fd = create_socket(super::AddressFamily::Inet, super::SockType::Stream);
        assert!(super::is_socket(&fd, None, None, None).unwrap());
        #[cfg(not(target_vendor = "apple"))]
        assert!(super::is_socket(&fd, Some(super::AddressFamily::Inet),
                                 Some(super::SockType::Stream), Some(false)).unwrap());
        #[cfg(target_vendor = "apple")]
        assert!(super::is_socket(&fd, Some(super::AddressFamily::Inet),
                                 Some(super::SockType::Stream), None).unwrap());

        let fd = open_file();
        assert!(!super::is_socket(&fd, None, None, None).unwrap());
    }

    #[test]
    fn is_socket_inet() {
        let _l = lock_env();
        let fd = create_socket(super::AddressFamily::Inet, super::SockType::Stream);
        assert!(super::is_socket_inet(&fd, None, None, None, None).unwrap());
        #[cfg(not(target_vendor = "apple"))]
        assert!(super::is_socket_inet(&fd, Some(super::AddressFamily::Inet),
                                      Some(super::SockType::Stream), Some(false), None).unwrap());
        #[cfg(target_vendor = "apple")]
        assert!(super::is_socket_inet(&fd, Some(super::AddressFamily::Inet),
                                      Some(super::SockType::Stream), None, None).unwrap());

        let fd = open_file();
        assert!(!super::is_socket_inet(&fd, None, None, None, None).unwrap());
    }

    #[test]
    fn is_socket_unix() {
        let _l = lock_env();
        let fd = create_socket(super::AddressFamily::Unix, super::SockType::Stream);
        assert!(super::is_socket_unix(&fd, None, None, None).unwrap());
        #[cfg(not(target_vendor = "apple"))]
        assert!(super::is_socket_unix(&fd, Some(super::SockType::Stream),
                                      Some(false), None).unwrap());
        #[cfg(target_vendor = "apple")]
        assert!(super::is_socket_unix(&fd, Some(super::SockType::Stream), None, None).unwrap());

        let fd = open_file();
        assert!(!super::is_socket_unix(&fd, None, None, None).unwrap());
    }
}
