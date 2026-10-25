//! Intake folder paths: what the system's errors on a folder mean
//! ([`classify`]).

use std::io;

use super::Health;

/// What a failed look at a folder (or, with `on_root == false`, at
/// something inside it) means for the owner (D2.6). Network failures are
/// told apart from a missing folder so "Can't reach the network folder"
/// isn't shown as "Can't find the folder".
#[must_use]
pub fn classify(e: &io::Error, on_root: bool) -> Health {
    #[cfg(windows)]
    if let Some(code) = e.raw_os_error() {
        match code {
            // ERROR_BAD_NETPATH, ERROR_BAD_NET_NAME, ERROR_NETWORK_UNREACHABLE,
            // ERROR_HOST_UNREACHABLE, ERROR_SEM_TIMEOUT, ERROR_NETNAME_DELETED.
            53 | 67 | 1231 | 1232 | 121 | 64 => return Health::Unreachable,
            // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND.
            2 | 3 if on_root => return Health::Missing,
            // ERROR_ACCESS_DENIED.
            5 => return Health::PermissionDenied,
            _ => {}
        }
    }
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        // EHOSTDOWN has no `ErrorKind` of its own.
        #[cfg(target_os = "linux")]
        const EHOSTDOWN: i32 = 112;
        #[cfg(not(target_os = "linux"))]
        const EHOSTDOWN: i32 = 64;
        if code == EHOSTDOWN {
            return Health::Unreachable;
        }
    }
    // `NotConnected`, `HostUnreachable`, `NetworkDown`, `TimedOut`,
    // `StaleNetworkFileHandle` and anything else unknown: the folder can't
    // be reached right now.
    match e.kind() {
        io::ErrorKind::NotFound if on_root => Health::Missing,
        io::ErrorKind::NotADirectory if on_root => Health::NotAFolder,
        io::ErrorKind::PermissionDenied => Health::PermissionDenied,
        _ => Health::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_codes() {
        for (code, health) in [
            (53, Health::Unreachable),
            (67, Health::Unreachable),
            (1231, Health::Unreachable),
            (1232, Health::Unreachable),
            (121, Health::Unreachable),
            (64, Health::Unreachable),
            (3, Health::Missing),
            (2, Health::Missing),
            (5, Health::PermissionDenied),
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
        // Not found below the root is not a missing folder.
        assert_eq!(
            classify(&io::Error::from_raw_os_error(2), false),
            Health::Unreachable
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_codes() {
        for (code, health) in [
            (107, Health::Unreachable), // ENOTCONN
            (112, Health::Unreachable), // EHOSTDOWN
            (113, Health::Unreachable), // EHOSTUNREACH
            (110, Health::Unreachable), // ETIMEDOUT
            (116, Health::Unreachable), // ESTALE
            (2, Health::Missing),       // ENOENT
            (13, Health::PermissionDenied),
            (20, Health::NotAFolder), // ENOTDIR
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_codes() {
        for (code, health) in [
            (57, Health::Unreachable), // ENOTCONN
            (64, Health::Unreachable), // EHOSTDOWN
            (65, Health::Unreachable), // EHOSTUNREACH
            (60, Health::Unreachable), // ETIMEDOUT
            (70, Health::Unreachable), // ESTALE
            (2, Health::Missing),
            (13, Health::PermissionDenied),
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
    }

    #[test]
    fn kinds() {
        for (kind, health) in [
            (io::ErrorKind::NotFound, Health::Missing),
            (io::ErrorKind::PermissionDenied, Health::PermissionDenied),
            (io::ErrorKind::TimedOut, Health::Unreachable),
            (io::ErrorKind::NotConnected, Health::Unreachable),
            (io::ErrorKind::Other, Health::Unreachable),
        ] {
            assert_eq!(classify(&io::Error::from(kind), true), health, "{kind:?}");
        }
    }
}
