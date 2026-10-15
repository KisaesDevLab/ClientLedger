//! Where the live database may be kept (Q-42).
//!
//! A running database must sit on a disk inside this computer. Network shares
//! and sync folders (`OneDrive`, Dropbox, iCloud Drive, Google Drive) can copy a
//! half-written file or drop a lock, which corrupts the database. Backups are
//! different and may go anywhere.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// A sync service that was recognised from a folder name or marker file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudService {
    /// Microsoft `OneDrive`.
    OneDrive,
    /// Dropbox.
    Dropbox,
    /// Apple iCloud Drive.
    ICloud,
    /// Google Drive.
    GoogleDrive,
    /// Some other cloud folder (for example a macOS File Provider folder).
    Other,
}

impl CloudService {
    fn label(self) -> &'static str {
        match self {
            CloudService::OneDrive => "OneDrive",
            CloudService::Dropbox => "Dropbox",
            CloudService::ICloud => "iCloud Drive",
            CloudService::GoogleDrive => "Google Drive",
            CloudService::Other => "cloud storage",
        }
    }
}

/// Why a folder cannot hold the live database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDirIssue {
    /// The folder is on another computer, reached over the network.
    NetworkShare {
        /// The folder that was refused.
        path: PathBuf,
        /// How we recognised it, such as `nfs` or `Windows network path`.
        detail: String,
    },
    /// The folder is inside a sync service.
    CloudSync {
        /// The folder that was refused.
        path: PathBuf,
        /// Which service.
        service: CloudService,
    },
    /// The folder name has a character that is unsafe to hand to the database
    /// program (it passes folder names through a command line).
    UnsafeName {
        /// The folder that was refused.
        path: PathBuf,
        /// The character.
        ch: char,
    },
}

impl fmt::Display for DataDirIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataDirIssue::UnsafeName { path, ch } => write!(
                f,
                "{} has the character {ch:?} in its name, which the database program cannot                  handle safely. Choose a folder whose full path does not contain it.",
                path.display()
            ),
            DataDirIssue::NetworkShare { path, .. } => write!(
                f,
                "{} is on a network drive. A company database must be kept on a drive inside \
                 this computer, because network drives can corrupt it. Choose a folder on a \
                 local drive. Backups can go anywhere, including network drives.",
                path.display()
            ),
            DataDirIssue::CloudSync { path, service } => write!(
                f,
                "{} is inside a {} folder. Sync programs can copy a database while it is being \
                 written and corrupt it. Choose a folder outside {}. Backups can be saved there.",
                path.display(),
                service.label(),
                service.label()
            ),
        }
    }
}

/// Check that `path` can hold a live database.
///
/// The folder does not need to exist yet: the closest existing parent is
/// resolved (symbolic links and mapped drives included) and checked.
///
/// # Errors
/// Returns the reason when the folder is on a network share or in a sync folder.
pub fn check_data_dir(path: &Path) -> Result<(), DataDirIssue> {
    let given = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(ch) = unsafe_char(&given.to_string_lossy()) {
        return Err(DataDirIssue::UnsafeName { path: given, ch });
    }
    let resolved = resolve(&given);
    let roots = onedrive_roots_from_env();
    let mounts = system_mounts();
    for p in [&given, &resolved] {
        if let Some(issue) = classify(p, &roots, &mounts) {
            return Err(match issue {
                Found::Network(detail) => DataDirIssue::NetworkShare {
                    path: given.clone(),
                    detail,
                },
                Found::Cloud(service) => DataDirIssue::CloudSync {
                    path: given.clone(),
                    service,
                },
            });
        }
    }
    Ok(())
}

/// A character that could break out of the quoted command-line text the
/// database program is started with: double quote, backtick, dollar sign,
/// backslash, and control characters on Unix (the shell would interpret
/// them); percent sign on Windows (`cmd` expands it).
fn unsafe_char(path: &str) -> Option<char> {
    path.chars().find(|&c| {
        c.is_control()
            || if cfg!(windows) {
                c == '%' || c == '"'
            } else {
                matches!(c, '"' | '`' | '$' | '\\')
            }
    })
}

enum Found {
    Network(String),
    Cloud(CloudService),
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
fn resolve(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(name) = cur.file_name().map(std::ffi::OsStr::to_os_string) else {
            return path.to_path_buf();
        };
        rest.push(name);
        if !cur.pop() {
            return path.to_path_buf();
        }
    }
}

fn classify(path: &Path, onedrive_roots: &[PathBuf], mounts: &[Mount]) -> Option<Found> {
    let text = path.to_string_lossy();
    if cfg!(windows) && is_unc_path(&text) {
        return Some(Found::Network("Windows network path".into()));
    }
    if let Some(fs) = network_mount_for(path, mounts) {
        return Some(Found::Network(fs));
    }
    if let Some(s) = cloud_by_components(path) {
        return Some(Found::Cloud(s));
    }
    if let Some(s) = cloud_by_roots(path, onedrive_roots) {
        return Some(Found::Cloud(s));
    }
    cloud_by_markers(path).map(Found::Cloud)
}

/// `\\server\share`, `//server/share`, or `\\?\UNC\server\share` (but not
/// `\\?\C:\` or `\\.\` device paths).
#[must_use]
pub fn is_unc_path(text: &str) -> bool {
    let t = text.replace('\\', "/");
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("//?/unc/") {
        return true;
    }
    t.starts_with("//") && !t.starts_with("//?/") && !t.starts_with("//./")
}

/// Folder-name rules for the sync services.
pub(crate) fn cloud_by_components(path: &Path) -> Option<CloudService> {
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    for (i, n) in names.iter().enumerate() {
        let n = n.as_str();
        if n == "onedrive" || n.starts_with("onedrive - ") || n.starts_with("onedrive-") {
            return Some(CloudService::OneDrive);
        }
        if n == "dropbox" || n.starts_with("dropbox (") || n.starts_with("dropbox-") {
            return Some(CloudService::Dropbox);
        }
        if n == "icloud drive"
            || n == "iclouddrive"
            || n == "com~apple~clouddocs"
            || (n == "mobile documents" && i > 0 && names[i - 1] == "library")
        {
            return Some(CloudService::ICloud);
        }
        if n == "google drive"
            || n == "googledrive"
            || n.starts_with("googledrive-")
            || n == "my drive"
        {
            return Some(CloudService::GoogleDrive);
        }
    }
    // macOS File Provider folders hold every cloud service; name-specific rules
    // above win so the message can say which one.
    names
        .windows(2)
        .any(|w| w[0] == "library" && w[1] == "cloudstorage")
        .then_some(CloudService::Other)
}

/// `OneDrive` root folders announced by the environment (Windows).
fn onedrive_roots_from_env() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(std::env::var_os)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub(crate) fn cloud_by_roots(path: &Path, roots: &[PathBuf]) -> Option<CloudService> {
    let lower = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    let me = lower(path);
    roots
        .iter()
        .map(|r| lower(r).trim_end_matches('/').to_string())
        .filter(|r| !r.is_empty())
        .any(|r| me == r || me.starts_with(&format!("{r}/")))
        .then_some(CloudService::OneDrive)
}

/// Marker files the sync clients leave in the root of their folder.
pub(crate) fn cloud_by_markers(path: &Path) -> Option<CloudService> {
    for dir in path.ancestors() {
        if dir.join(".dropbox").exists() || dir.join(".dropbox.cache").exists() {
            return Some(CloudService::Dropbox);
        }
        if dir.join(".shortcut-targets-by-id").exists()
            || dir.join(".tmp.driveupload").exists()
            || dir.join(".tmp.drivedownload").exists()
        {
            return Some(CloudService::GoogleDrive);
        }
    }
    None
}

/// One mounted file system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub point: PathBuf,
    pub fs_type: String,
}

/// File system types that live on another computer.
pub(crate) fn is_network_fs(fs_type: &str) -> bool {
    let t = fs_type.to_ascii_lowercase();
    matches!(
        t.as_str(),
        "nfs"
            | "nfs3"
            | "nfs4"
            | "cifs"
            | "smb"
            | "smb2"
            | "smb3"
            | "smbfs"
            | "ncpfs"
            | "afs"
            | "afpfs"
            | "webdav"
            | "davfs"
            | "ceph"
            | "glusterfs"
            | "lustre"
    ) || t.starts_with("fuse.sshfs")
        || t.starts_with("fuse.rclone")
        || t.starts_with("fuse.s3fs")
        || t.starts_with("fuse.gcsfuse")
        || t.starts_with("fuse.davfs")
        || t.starts_with("fuse.glusterfs")
        || t.starts_with("fuse.cephfs")
}

/// Parse Linux `/proc/mounts` text.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_proc_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let _device = it.next()?;
            let point = unescape_octal(it.next()?);
            let fs_type = it.next()?.to_string();
            Some(Mount {
                point: PathBuf::from(point),
                fs_type,
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
/// `/proc/mounts` escapes space, tab, newline, and backslash as `\040` etc.
fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..=i + 3].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (u32::from(b[i + 1] - b'0') << 6)
                | (u32::from(b[i + 2] - b'0') << 3)
                | u32::from(b[i + 3] - b'0');
            out.push(u8::try_from(v & 0xff).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse macOS `mount` output: `//u@host/share on /Volumes/x (smbfs, nodev)`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_mount_output(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|l| {
            let (_, rest) = l.split_once(" on ")?;
            let open = rest.rfind(" (")?;
            let point = &rest[..open];
            let opts = rest[open + 2..].trim_end_matches(')');
            let fs_type = opts.split(',').next()?.trim().to_string();
            Some(Mount {
                point: PathBuf::from(point),
                fs_type,
            })
        })
        .collect()
}

/// The file system type of the longest mount point that contains `path`, when
/// that file system is a network one.
pub(crate) fn network_mount_for(path: &Path, mounts: &[Mount]) -> Option<String> {
    let best = mounts
        .iter()
        .filter(|m| path.starts_with(&m.point))
        .max_by_key(|m| m.point.components().count())?;
    is_network_fs(&best.fs_type).then(|| best.fs_type.clone())
}

#[cfg(target_os = "linux")]
fn system_mounts() -> Vec<Mount> {
    std::fs::read_to_string("/proc/mounts")
        .map(|t| parse_proc_mounts(&t))
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn system_mounts() -> Vec<Mount> {
    match crate::proc::command(Path::new("/sbin/mount")).output() {
        Ok(o) if o.status.success() => parse_mount_output(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_mounts() -> Vec<Mount> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_that_break_command_lines_are_refused() {
        let bad: &[&str] = if cfg!(windows) {
            &["C:/data/100%/pgdata", "C:/data/%PATH%/pgdata"]
        } else {
            &[
                "/data/a\"b",
                "/data/a`b",
                "/data/$HOME",
                "/data/a\\b",
                "/data/a\nb",
            ]
        };
        for p in bad {
            assert!(
                matches!(
                    check_data_dir(Path::new(p)),
                    Err(DataDirIssue::UnsafeName { .. })
                ),
                "{p}"
            );
        }
        assert_eq!(unsafe_char("/home/some one/it's here/pgdata"), None);
        assert_eq!(unsafe_char("C:/Users/Some One/AppData/pgdata"), None);
    }
    use std::fs;

    fn comp(p: &str) -> Option<CloudService> {
        cloud_by_components(Path::new(p))
    }

    #[test]
    fn unc_paths_are_recognised() {
        assert!(is_unc_path(r"\\server\share\books"));
        assert!(is_unc_path("//server/share/books"));
        assert!(is_unc_path(r"\\?\UNC\server\share\books"));
        assert!(!is_unc_path(r"\\?\C:\Users\me"));
        assert!(!is_unc_path(r"\\.\PhysicalDrive0"));
        assert!(!is_unc_path(r"C:\Users\me"));
        assert!(!is_unc_path("/home/me"));
    }

    #[test]
    fn sync_folders_are_recognised_by_name() {
        assert_eq!(
            comp("C:/Users/me/OneDrive/Books"),
            Some(CloudService::OneDrive)
        );
        assert_eq!(
            comp("C:/Users/me/OneDrive - Contoso Ltd/x"),
            Some(CloudService::OneDrive)
        );
        assert_eq!(comp("/home/me/Dropbox/x"), Some(CloudService::Dropbox));
        assert_eq!(
            comp("/home/me/Dropbox (Personal)/x"),
            Some(CloudService::Dropbox)
        );
        assert_eq!(
            comp("C:/Users/me/iCloudDrive/x"),
            Some(CloudService::ICloud)
        );
        assert_eq!(
            comp("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/x"),
            Some(CloudService::ICloud)
        );
        assert_eq!(comp("G:/My Drive/x"), Some(CloudService::GoogleDrive));
        assert_eq!(
            comp("/Users/me/Google Drive/x"),
            Some(CloudService::GoogleDrive)
        );
        assert_eq!(
            comp("/Users/me/Library/CloudStorage/GoogleDrive-me@x.com/My Drive"),
            Some(CloudService::GoogleDrive)
        );
        assert_eq!(
            comp("/Users/me/Library/CloudStorage/Box-Box/x"),
            Some(CloudService::Other)
        );
    }

    #[test]
    fn ordinary_folders_are_not_flagged() {
        for p in [
            "C:/Users/me/AppData/Local/ClientLedger/pgdata",
            "/home/me/.local/share/clientledger/pgdata",
            "/Users/me/Library/Application Support/ClientLedger/pgdata",
            "/home/me/OneDriveNotes/x",
            "/home/me/dropboxer/x",
            "/home/me/Library/x",
        ] {
            assert_eq!(comp(p), None, "{p}");
        }
    }

    #[test]
    fn environment_roots_match_whole_folders_only() {
        let roots = vec![PathBuf::from(r"C:\Users\me\OneDrive - Acme")];
        assert!(cloud_by_roots(Path::new(r"C:\Users\ME\OneDrive - Acme\Docs\x"), &roots).is_some());
        assert!(cloud_by_roots(Path::new(r"C:\Users\me\OneDrive - Acme"), &roots).is_some());
        assert!(cloud_by_roots(Path::new(r"C:\Users\me\OneDrive - Acme2\x"), &roots).is_none());
        assert!(cloud_by_roots(Path::new(r"C:\Users\me\Other"), &roots).is_none());
        assert!(cloud_by_roots(Path::new(r"C:\x"), &[PathBuf::new()]).is_none());
    }

    #[test]
    fn marker_files_are_found_in_any_parent() {
        let root = std::env::temp_dir().join(format!("cl-pg-mark-{}", std::process::id()));
        let deep = root.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(cloud_by_markers(&deep.join("pgdata")), None);
        fs::write(root.join(".dropbox"), b"").unwrap();
        assert_eq!(
            cloud_by_markers(&deep.join("pgdata")),
            Some(CloudService::Dropbox)
        );
        fs::remove_file(root.join(".dropbox")).unwrap();
        fs::create_dir_all(root.join("a").join(".shortcut-targets-by-id")).unwrap();
        assert_eq!(
            cloud_by_markers(&deep.join("pgdata")),
            Some(CloudService::GoogleDrive)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn proc_mounts_are_parsed_and_matched_by_longest_prefix() {
        let text = "\
/dev/sda1 / ext4 rw 0 0
server:/export /mnt/shared nfs4 rw 0 0
//nas/books /mnt/my\\040books cifs rw 0 0
/dev/sdb1 /mnt/shared/local ext4 rw 0 0
";
        let mounts = parse_proc_mounts(text);
        assert_eq!(mounts.len(), 4);
        assert_eq!(mounts[2].point, PathBuf::from("/mnt/my books"));
        assert_eq!(
            network_mount_for(Path::new("/mnt/shared/data/pg"), &mounts),
            Some("nfs4".into())
        );
        assert_eq!(
            network_mount_for(Path::new("/mnt/my books/pg"), &mounts),
            Some("cifs".into())
        );
        // A local disk mounted inside a network share wins.
        assert_eq!(
            network_mount_for(Path::new("/mnt/shared/local/pg"), &mounts),
            None
        );
        assert_eq!(network_mount_for(Path::new("/home/me/pg"), &mounts), None);
    }

    #[test]
    fn macos_mount_output_is_parsed() {
        let text = "\
/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)
//me@nas._smb._tcp.local/books on /Volumes/books (smbfs, nodev, nosuid, mounted by me)
server:/vol on /Volumes/My Share (nfs, nodev)
";
        let mounts = parse_mount_output(text);
        assert_eq!(mounts.len(), 3);
        assert_eq!(
            network_mount_for(Path::new("/Volumes/books/pg"), &mounts),
            Some("smbfs".into())
        );
        assert_eq!(
            network_mount_for(Path::new("/Volumes/My Share/pg"), &mounts),
            Some("nfs".into())
        );
        assert_eq!(network_mount_for(Path::new("/Users/me/pg"), &mounts), None);
    }

    #[test]
    fn fuse_network_file_systems_count_as_network() {
        assert!(is_network_fs("fuse.sshfs"));
        assert!(is_network_fs("CIFS"));
        assert!(!is_network_fs("ext4"));
        assert!(!is_network_fs("fuse.gvfsd-fuse"));
        assert!(!is_network_fs("9p"));
    }

    #[test]
    fn the_message_names_the_problem_in_plain_words() {
        let msg = DataDirIssue::CloudSync {
            path: PathBuf::from("/x/Dropbox/pg"),
            service: CloudService::Dropbox,
        }
        .to_string();
        assert!(
            msg.contains("Dropbox") && msg.contains("Choose a folder"),
            "{msg}"
        );
        let msg = DataDirIssue::NetworkShare {
            path: PathBuf::from("/mnt/nas/pg"),
            detail: "nfs".into(),
        }
        .to_string();
        assert!(msg.contains("network drive") && msg.contains("Backups can go anywhere"));
    }

    #[test]
    fn a_normal_temp_folder_passes() {
        // Not inside OneDrive on any CI machine we use; a developer whose temp
        // folder is redirected into a sync folder will see this fail, correctly.
        let p = std::env::temp_dir().join("cl-pg-guard-pass").join("pgdata");
        assert_eq!(check_data_dir(&p), Ok(()));
    }
}
