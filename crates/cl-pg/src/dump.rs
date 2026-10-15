//! Whole-cluster backup and restore for disaster recovery.
//!
//! The pinned zonky binaries ship only `postgres`, `initdb`, and `pg_ctl`, so
//! there is no `pg_dump` (Q-P03a). A cluster backup is therefore a *cold copy*:
//! stop the server, archive the data directory (`tar.gz`), verify the archive
//! by reading it back, and restart the server if it was running. It is exact
//! (same bytes, same row counts) but only valid for the same `PostgreSQL` major
//! version and the same operating system and CPU. Company backups and
//! transfers use the engine's per-company export instead (Q-05).
//!
//! The archive holds the role's password *verifier*, never the password.

use crate::binaries::host_dir_name;
use crate::secret::hex;
use crate::{Cluster, PasswordStore, PgError, Result, Running, Secret, proc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Archive format version.
const FORMAT: u64 = 1;

/// Name of the manifest entry (always the last entry).
const MANIFEST: &str = "manifest.json";

/// Directory prefix of the data directory inside the archive.
const PREFIX: &str = "data";

/// Names at the top of the data directory that are never archived: runtime
/// state and logs.
const EXCLUDED: [&str; 3] = ["postmaster.pid", "postmaster.opts", "log"];

/// What a finished backup contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpInfo {
    /// The archive file.
    pub path: PathBuf,
    /// Regular files archived.
    pub files: u64,
    /// Uncompressed size of those files in bytes.
    pub bytes: u64,
    /// SHA-256 of the archive file, lowercase hex.
    pub sha256: String,
    /// Set when the server was running before the backup and was started again.
    /// Its port can differ from before.
    pub restarted: Option<Running>,
}

/// What a finished restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreInfo {
    /// The data directory that was replaced, kept so nothing is lost. Delete it
    /// once the restored database is confirmed good. `None` if there was none.
    pub previous: Option<PathBuf>,
    /// Regular files restored.
    pub files: u64,
}

/// The facts stored in `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    format: u64,
    pg_major: String,
    platform: String,
    files: u64,
    bytes: u64,
}

impl Manifest {
    fn to_json(&self) -> String {
        format!(
            "{{\n  \"format\": {},\n  \"pgMajor\": \"{}\",\n  \"platform\": \"{}\",\n  \"files\": {},\n  \"bytes\": {}\n}}\n",
            self.format, self.pg_major, self.platform, self.files, self.bytes
        )
    }

    fn parse(text: &str) -> Option<Manifest> {
        fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
            let at = text.find(&format!("\"{key}\""))?;
            let rest = &text[at + key.len() + 2..];
            let rest = rest.trim_start().strip_prefix(':')?.trim_start();
            let end = rest.find([',', '\n', '}'])?;
            Some(rest[..end].trim().trim_matches('"'))
        }
        Some(Manifest {
            format: field(text, "format")?.parse().ok()?,
            pg_major: field(text, "pgMajor")?.to_string(),
            platform: field(text, "platform")?.to_string(),
            files: field(text, "files")?.parse().ok()?,
            bytes: field(text, "bytes")?.parse().ok()?,
        })
    }
}

fn backup_err(msg: impl Into<String>) -> PgError {
    PgError::Backup(msg.into())
}

fn io_err(context: &str) -> impl Fn(std::io::Error) -> PgError + '_ {
    move |e| PgError::io(context, e)
}

impl Cluster {
    /// Back up the whole cluster to `dest` (a `.tar.gz` file).
    ///
    /// The server is stopped for the copy (a few seconds for a small business)
    /// and started again afterwards if it was running.
    ///
    /// # Errors
    /// Fails if `dest` is inside the data directory, the server cannot be
    /// stopped, a file cannot be read, or the written archive does not verify.
    pub fn dump(&self, dest: &Path) -> Result<DumpInfo> {
        let dest = std::path::absolute(dest).map_err(io_err("resolve backup path"))?;
        if dest.starts_with(&self.data_dir) {
            return Err(backup_err(
                "the backup file cannot be inside the data folder",
            ));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io_err("create backup folder"))?;
        }
        let was_running = self.running_port().is_some();
        self.stop()?;
        let result = self.write_archive(&dest);
        let restarted = if was_running {
            match self.start() {
                Ok(r) => Some(r),
                Err(e) => {
                    // The backup itself may be fine; report the restart problem
                    // only when the backup also failed or is the only error.
                    if result.is_ok() {
                        return Err(e);
                    }
                    None
                }
            }
        } else {
            None
        };
        let (files, bytes, sha256) = result?;
        tracing::info!(files, bytes, "cluster backup written and verified");
        Ok(DumpInfo {
            path: dest,
            files,
            bytes,
            sha256,
            restarted,
        })
    }

    fn write_archive(&self, dest: &Path) -> Result<(u64, u64, String)> {
        let part = dest.with_extension("part");
        let counted = self.write_archive_to(&part);
        let (files, bytes) = match counted {
            Ok(c) => c,
            Err(e) => {
                let _ = fs::remove_file(&part);
                return Err(e);
            }
        };
        if let Err(e) = verify_archive(&part, files, bytes) {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
        let sha = sha256_file(&part)?;
        if dest.exists() {
            fs::remove_file(dest).map_err(io_err("replace old backup"))?;
        }
        fs::rename(&part, dest).map_err(io_err("save backup"))?;
        Ok((files, bytes, sha))
    }

    fn write_archive_to(&self, part: &Path) -> Result<(u64, u64)> {
        let file = fs::File::create(part).map_err(io_err("create backup file"))?;
        let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::fast()));
        builder.follow_symlinks(false);
        let (mut files, mut bytes) = (0u64, 0u64);
        let mut stack: Vec<(PathBuf, String)> = vec![(self.data_dir.clone(), PREFIX.to_string())];
        while let Some((dir, name)) = stack.pop() {
            builder
                .append_dir(&name, &dir)
                .map_err(io_err("add folder to backup"))?;
            let mut entries: Vec<_> = fs::read_dir(&dir)
                .map_err(io_err("read data folder"))?
                .collect::<std::io::Result<_>>()
                .map_err(io_err("read data folder"))?;
            entries.sort_by_key(fs::DirEntry::file_name);
            for e in entries {
                let fname = e.file_name().to_string_lossy().into_owned();
                if dir == self.data_dir && EXCLUDED.contains(&fname.as_str()) {
                    continue;
                }
                let path = e.path();
                let entry_name = format!("{name}/{fname}");
                let ft = e.file_type().map_err(io_err("read data folder"))?;
                if ft.is_dir() {
                    stack.push((path, entry_name));
                } else if ft.is_file() {
                    let len = e.metadata().map_err(io_err("read data file"))?.len();
                    builder
                        .append_path_with_name(&path, &entry_name)
                        .map_err(io_err("add file to backup"))?;
                    files += 1;
                    bytes += len;
                } else {
                    return Err(backup_err(format!(
                        "{entry_name} is a link or special file; this backup cannot hold it"
                    )));
                }
            }
        }
        let manifest = Manifest {
            format: FORMAT,
            pg_major: self.data_major()?,
            platform: host_dir_name().unwrap_or("unknown").to_string(),
            files,
            bytes,
        }
        .to_json();
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        builder
            .append_data(&mut header, MANIFEST, manifest.as_bytes())
            .map_err(io_err("add manifest to backup"))?;
        let gz = builder.into_inner().map_err(io_err("finish backup"))?;
        let mut file = gz.finish().map_err(io_err("finish backup"))?;
        file.flush().map_err(io_err("finish backup"))?;
        file.sync_all().map_err(io_err("flush backup to disk"))?;
        Ok((files, bytes))
    }

    /// Major version of the data directory (`PG_VERSION`).
    fn data_major(&self) -> Result<String> {
        let v = fs::read_to_string(self.data_dir.join("PG_VERSION"))
            .map_err(io_err("read PG_VERSION"))?;
        Ok(v.trim().to_string())
    }

    /// Major version of the bundled server, from `postgres --version`.
    fn binary_major(&self) -> Result<String> {
        let mut cmd = proc::command(&self.bins.program("postgres"));
        cmd.arg("--version");
        let out = proc::run(cmd, "postgres", None)?;
        parse_version_major(&out)
            .ok_or_else(|| backup_err("could not read the bundled PostgreSQL version"))
    }

    /// Replace the cluster with the one in the backup at `src`.
    ///
    /// The archive is unpacked and checked first (format, platform, major
    /// version, file count and size); only then is the server stopped and the
    /// folders swapped. The data directory that was replaced is **kept** next
    /// to the new one and returned in [`RestoreInfo::previous`], so a restore
    /// never destroys the only copy.
    ///
    /// The restored database keeps the password it had when it was backed up.
    /// If that differs from the saved one, use [`Cluster::reset_password`].
    ///
    /// # Errors
    /// Fails with [`PgError::Backup`] when the archive is damaged or does not
    /// match this install; nothing is changed in that case.
    pub fn restore(&self, src: &Path) -> Result<RestoreInfo> {
        let staging = sibling(&self.data_dir, ".restoring");
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(io_err("create restore folder"))?;
        let staged = self.unpack_and_check(src, &staging);
        let files = match staged {
            Ok(f) => f,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };

        self.stop()?;
        let previous = if self.data_dir.exists() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let keep = sibling(&self.data_dir, &format!(".before-restore-{stamp}"));
            fs::rename(&self.data_dir, &keep).map_err(io_err("set the old database aside"))?;
            Some(keep)
        } else {
            None
        };
        if let Err(e) = fs::rename(staging.join(PREFIX), &self.data_dir) {
            if let Some(keep) = &previous {
                let _ = fs::rename(keep, &self.data_dir);
            }
            let _ = fs::remove_dir_all(&staging);
            return Err(PgError::io("put the restored database in place", e));
        }
        let _ = fs::remove_dir_all(&staging);
        self.write_static_config()?;
        tracing::info!(files, "cluster restored from backup");
        Ok(RestoreInfo { previous, files })
    }

    fn unpack_and_check(&self, src: &Path, staging: &Path) -> Result<u64> {
        let file = fs::File::open(src).map_err(io_err("open backup file"))?;
        let mut archive = tar::Archive::new(GzDecoder::new(file));
        let (mut files, mut bytes) = (0u64, 0u64);
        for entry in archive
            .entries()
            .map_err(|e| backup_err(format!("not a readable backup: {e}")))?
        {
            let mut entry = entry.map_err(|e| backup_err(format!("damaged backup: {e}")))?;
            let path = entry
                .path()
                .map_err(|e| backup_err(format!("bad entry name: {e}")))?
                .into_owned();
            let first = path
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            if first != PREFIX && path != Path::new(MANIFEST) {
                return Err(backup_err(format!("unexpected entry {}", path.display())));
            }
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                return Err(backup_err(format!("unsupported entry {}", path.display())));
            }
            if kind.is_file() && first == PREFIX {
                files += 1;
                bytes += entry.size();
            }
            let safe = entry
                .unpack_in(staging)
                .map_err(|e| backup_err(format!("could not unpack {}: {e}", path.display())))?;
            if !safe {
                return Err(backup_err(format!("unsafe entry {}", path.display())));
            }
        }
        let text = fs::read_to_string(staging.join(MANIFEST))
            .map_err(|_| backup_err("the backup has no manifest (incomplete file)"))?;
        let m = Manifest::parse(&text).ok_or_else(|| backup_err("the manifest is unreadable"))?;
        if m.format != FORMAT {
            return Err(backup_err(format!(
                "backup format {} is not supported",
                m.format
            )));
        }
        if m.files != files || m.bytes != bytes {
            return Err(backup_err(
                "the backup does not match its manifest (damaged)",
            ));
        }
        let here = host_dir_name().unwrap_or("unknown");
        if m.platform != here {
            return Err(backup_err(format!(
                "the backup was made on {} and cannot be restored on {here}",
                m.platform
            )));
        }
        let major = self.binary_major()?;
        let staged_major = fs::read_to_string(staging.join(PREFIX).join("PG_VERSION"))
            .map_err(|_| backup_err("the backup holds no database"))?;
        if m.pg_major != major || staged_major.trim() != major {
            return Err(backup_err(format!(
                "the backup is PostgreSQL {} but this version is {major}",
                m.pg_major
            )));
        }
        Ok(files)
    }

    /// Give the database a new random password and save it in `store`.
    ///
    /// Use after [`Cluster::restore`] when the restored database has a
    /// different password than the saved one. The server must be stopped; it
    /// runs `postgres --single`, which needs no login, and the password is
    /// sent over standard input only.
    ///
    /// Not available when the program runs elevated on Windows (`PostgreSQL`
    /// refuses to run with administrator rights).
    ///
    /// # Errors
    /// Fails if the server is running or the single-user run fails.
    pub fn reset_password(&mut self, store: &dyn PasswordStore) -> Result<()> {
        self.stop()?;
        let new = Secret::generate()?;
        let mut cmd = proc::command(&self.bins.program("postgres"));
        cmd.args(["--single", "-D"])
            .arg(&self.data_dir)
            .args(["-c", "log_min_error_statement=panic", "postgres"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| PgError::io("start postgres --single", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            let sql = format!("ALTER USER postgres PASSWORD '{}'\n", new.expose());
            stdin
                .write_all(sql.as_bytes())
                .map_err(|e| PgError::io("send the new password", e))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| PgError::io("wait for postgres --single", e))?;
        if !out.status.success() {
            let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stdout));
            return Err(PgError::Command {
                program: "postgres",
                status: out
                    .status
                    .code()
                    .map_or_else(|| "killed".into(), |c| format!("exit {c}")),
                detail: proc::tail(&new.scrub(&text), Some(&self.password)),
            });
        }
        store.set(&new)?;
        self.password = new;
        tracing::info!("database password reset");
        Ok(())
    }
}

/// `<data_dir><suffix>` next to the data directory.
fn sibling(data_dir: &Path, suffix: &str) -> PathBuf {
    let mut name = data_dir
        .file_name()
        .map_or_else(|| "pgdata".into(), std::ffi::OsStr::to_os_string);
    name.push(suffix);
    data_dir.with_file_name(name)
}

/// `16` from `postgres (PostgreSQL) 16.15`.
fn parse_version_major(output: &str) -> Option<String> {
    let version = output.split_whitespace().last()?;
    let major = version.split('.').next()?;
    major
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| major.to_string())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).map_err(io_err("read backup"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(io_err("read backup"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Read the archive back end to end and compare with what was written.
fn verify_archive(path: &Path, files: u64, bytes: u64) -> Result<()> {
    let file = fs::File::open(path).map_err(io_err("verify backup"))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let (mut seen_files, mut seen_bytes, mut manifest, mut has_version) =
        (0u64, 0u64, false, false);
    for entry in archive
        .entries()
        .map_err(|e| backup_err(format!("verification failed: {e}")))?
    {
        let mut entry = entry.map_err(|e| backup_err(format!("verification failed: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| backup_err(format!("verification failed: {e}")))?
            .to_string_lossy()
            .replace('\\', "/");
        if name == MANIFEST {
            manifest = true;
        }
        if name == format!("{PREFIX}/PG_VERSION") {
            has_version = true;
        }
        if entry.header().entry_type().is_file() {
            // Reading to the end checks the compressed stream's checksum.
            let n = std::io::copy(&mut entry, &mut std::io::sink())
                .map_err(|e| backup_err(format!("verification failed: {e}")))?;
            if name != MANIFEST {
                seen_files += 1;
                seen_bytes += n;
            }
        }
    }
    if !manifest || !has_version || seen_files != files || seen_bytes != bytes {
        return Err(backup_err(
            "the backup did not verify after writing (file count or size differs)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips() {
        let m = Manifest {
            format: 1,
            pg_major: "16".into(),
            platform: "windows-x64".into(),
            files: 1234,
            bytes: 56_789_012,
        };
        assert_eq!(Manifest::parse(&m.to_json()), Some(m));
    }

    #[test]
    fn manifest_rejects_garbage() {
        assert_eq!(Manifest::parse("{}"), None);
        assert_eq!(Manifest::parse("not json"), None);
        assert_eq!(
            Manifest::parse("{\"format\": x, \"pgMajor\": \"16\"}"),
            None
        );
    }

    #[test]
    fn version_output_is_parsed() {
        assert_eq!(
            parse_version_major("postgres (PostgreSQL) 16.15"),
            Some("16".into())
        );
        assert_eq!(
            parse_version_major("postgres (PostgreSQL) 17.2\n"),
            Some("17".into())
        );
        assert_eq!(parse_version_major("garbage"), None);
        assert_eq!(parse_version_major(""), None);
    }

    #[test]
    fn sibling_names_sit_next_to_the_data_dir() {
        assert_eq!(
            sibling(Path::new("/a/b/pgdata"), ".restoring"),
            PathBuf::from("/a/b/pgdata.restoring")
        );
    }

    #[test]
    fn a_missing_archive_does_not_verify() {
        assert!(verify_archive(Path::new("/definitely/not/here.tar.gz"), 0, 0).is_err());
    }
}
