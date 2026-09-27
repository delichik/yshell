//! Real SFTP backend implemented with an embedded ssh2 SFTP session.

use std::{
    fs,
    io::{self, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use ssh2::{
    FileStat, KeyboardInteractivePrompt, OpenFlags, OpenType, Prompt, RenameFlags, Session, Sftp,
};
use yshell_ssh::{AuthMethod, ProxyConfig, SshConnectionConfig};

use crate::{
    client::SftpBackend,
    error::{SftpError, SftpErrorKind, SftpResult},
    fs_entry::{DirectoryListing, FsEntry, FsEntryKind},
};

const FILE_TYPE_MASK: u32 = 0o170000;
const FILE_TYPE_DIRECTORY: u32 = 0o040000;
const FILE_TYPE_FILE: u32 = 0o100000;
const FILE_TYPE_SYMLINK: u32 = 0o120000;
const FILE_PERMISSIONS_MASK: u32 = 0o7777;

#[derive(Debug, Clone)]
pub struct RealSftpBackend {
    config: SshConnectionConfig,
}

impl RealSftpBackend {
    pub fn new(config: SshConnectionConfig) -> Self {
        Self { config }
    }

    fn with_sftp_client<T>(&self, operation: &str, run: impl FnOnce(&Sftp) -> SftpResult<T>) -> SftpResult<T> {
        self.config.validate().map_err(map_ssh_error)?;
        if !matches!(self.config.proxy, ProxyConfig::None) {
            return Err(SftpError::new(
                SftpErrorKind::Backend,
                "native SFTP backend does not support proxy negotiation yet",
            ));
        }
        let tcp = connect_tcp_stream(&self.config)?;
        let session = connect_session(&self.config, tcp)?;
        let sftp = session.sftp().map_err(|error| {
            SftpError::new(
                SftpErrorKind::Backend,
                format!("failed to open native SFTP channel for {operation}: {error}"),
            )
        })?;
        run(&sftp)
    }
}

impl SftpBackend for RealSftpBackend {
    fn list_dir(&self, path: &str) -> SftpResult<DirectoryListing> {
        self.with_sftp_client("list_dir", |sftp| {
            let mut entries = sftp
                .readdir(Path::new(path))
                .map_err(|error| map_sftp_error(path, error, "list directory"))?
                .into_iter()
                .filter_map(|(path_buf, stat)| map_remote_entry(path_buf, stat))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.path.cmp(&right.path));
            Ok(DirectoryListing {
                path: path.to_owned(),
                entries,
            })
        })
    }

    fn chmod(&mut self, path: &str, permissions: u32) -> SftpResult<()> {
        self.with_sftp_client("chmod", |sftp| {
            let existing = sftp
                .stat(Path::new(path))
                .map_err(|error| map_sftp_error(path, error, "read file metadata before chmod"))?;
            let mode = merge_permissions(existing.perm, permissions);
            let stat = FileStat {
                size: None,
                uid: None,
                gid: None,
                perm: Some(mode),
                atime: None,
                mtime: None,
            };
            sftp.setstat(Path::new(path), stat)
                .map_err(|error| map_sftp_error(path, error, "chmod"))?;
            Ok(())
        })
    }

    fn delete(&mut self, path: &str) -> SftpResult<()> {
        self.with_sftp_client("delete", |sftp| {
            if sftp.unlink(Path::new(path)).is_ok() {
                return Ok(());
            }
            sftp.rmdir(Path::new(path))
                .map_err(|error| map_sftp_error(path, error, "delete"))?;
            Ok(())
        })
    }

    fn remove_dir(&mut self, path: &str) -> SftpResult<()> {
        self.with_sftp_client("remove_dir", |sftp| {
            sftp.rmdir(Path::new(path))
                .map_err(|error| map_sftp_error(path, error, "remove directory"))?;
            Ok(())
        })
    }

    fn rename(&mut self, from: &str, to: &str) -> SftpResult<()> {
        self.with_sftp_client("rename", |sftp| {
            sftp.rename(Path::new(from), Path::new(to), Some(RenameFlags::OVERWRITE))
                .map_err(|error| map_sftp_error(from, error, "rename"))?;
            Ok(())
        })
    }

    fn mkdir(&mut self, path: &str) -> SftpResult<()> {
        self.with_sftp_client("mkdir", |sftp| {
            sftp.mkdir(Path::new(path), 0o755)
                .map_err(|error| map_sftp_error(path, error, "mkdir"))?;
            Ok(())
        })
    }

    fn upload_file(&mut self, local_path: &Path, remote_path: &str) -> SftpResult<()> {
        self.with_sftp_client("upload_file", |sftp| {
            let mut local = fs::File::open(local_path).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!(
                        "failed to open local upload source `{}`: {error}",
                        local_path.display()
                    ),
                )
            })?;
            let mut remote = sftp
                .open_mode(
                    Path::new(remote_path),
                    OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                    0o644,
                    OpenType::File,
                )
                .map_err(|error| map_sftp_error(remote_path, error, "open upload target"))?;
            io::copy(&mut local, &mut remote).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!(
                        "failed to upload `{}` to `{remote_path}`: {error}",
                        local_path.display()
                    ),
                )
            })?;
            remote.flush().map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!("failed to flush remote upload target `{remote_path}`: {error}"),
                )
            })?;
            Ok(())
        })
    }

    fn download_file(&self, remote_path: &str, local_path: &Path) -> SftpResult<()> {
        self.with_sftp_client("download_file", |sftp| {
            if let Some(parent) = local_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    SftpError::new(
                        SftpErrorKind::Backend,
                        format!(
                            "failed to create local download directory `{}`: {error}",
                            parent.display()
                        ),
                    )
                })?;
            }
            let mut remote = sftp
                .open(Path::new(remote_path))
                .map_err(|error| map_sftp_error(remote_path, error, "open download source"))?;
            let mut local = fs::File::create(local_path).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!(
                        "failed to open local download target `{}`: {error}",
                        local_path.display()
                    ),
                )
            })?;
            io::copy(&mut remote, &mut local).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!(
                        "failed to download `{remote_path}` to `{}`: {error}",
                        local_path.display()
                    ),
                )
            })?;
            local.flush().map_err(|error| {
                SftpError::new(
                    SftpErrorKind::Backend,
                    format!(
                        "failed to flush local download target `{}`: {error}",
                        local_path.display()
                    ),
                )
            })?;
            Ok(())
        })
    }
}

fn connect_tcp_stream(config: &SshConnectionConfig) -> SftpResult<TcpStream> {
    let address = format!("{}:{}", config.host, config.port);
    let mut candidates = address.to_socket_addrs().map_err(|error| {
        SftpError::new(
            SftpErrorKind::Backend,
            format!("failed to resolve SFTP target `{address}`: {error}"),
        )
    })?;
    let timeout = config.connect_timeout.max(Duration::from_secs(1));
    let mut last_error = None;
    for candidate in candidates.by_ref() {
        match TcpStream::connect_timeout(&candidate, timeout) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(timeout));
                let _ = stream.set_write_timeout(Some(timeout));
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(SftpError::new(
        SftpErrorKind::Backend,
        format!(
            "failed to connect to native SFTP target `{address}`: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no socket addresses resolved".to_owned())
        ),
    ))
}

fn connect_session(config: &SshConnectionConfig, tcp: TcpStream) -> SftpResult<Session> {
    let mut session = Session::new().map_err(|error| {
        SftpError::new(
            SftpErrorKind::Backend,
            format!("failed to create native SSH session for SFTP: {error}"),
        )
    })?;
    session.set_timeout(session_timeout_ms(config.connect_timeout));
    session.set_tcp_stream(tcp);
    session.handshake().map_err(|error| {
        SftpError::new(
            SftpErrorKind::Backend,
            format!("native SSH handshake failed for SFTP: {error}"),
        )
    })?;
    verify_host_key(&session, config)?;
    authenticate_session(&session, config)?;
    Ok(session)
}

fn verify_host_key(session: &Session, config: &SshConnectionConfig) -> SftpResult<()> {
    let Some((key, key_type)) = session.host_key() else {
        return Err(SftpError::new(
            SftpErrorKind::Backend,
            "native SFTP session did not expose a server host key",
        ));
    };
    let presented = yshell_ssh::HostKeyFingerprint {
        algorithm: host_key_algorithm_label(key_type).to_owned(),
        fingerprint: bytes_to_hex(key),
    };
    // 唯一静默信任的分支：显式测试策略。
    if matches!(
        config.host_key_policy,
        yshell_ssh::HostKeyPolicy::AcceptAnyForTesting
    ) {
        return Ok(());
    }
    match config.known_hosts.get(&config.host, config.port) {
        Some(expected) if expected == &presented => Ok(()),
        Some(expected) => Err(SftpError::new(
            SftpErrorKind::PermissionDenied,
            format!(
                "native SFTP host key mismatch for {}:{}: expected {} {}, got {} {}",
                config.host,
                config.port,
                expected.algorithm,
                expected.fingerprint,
                presented.algorithm,
                presented.fingerprint
            ),
        )),
        // 产品决定（2026-09-27）：与 yshell-ssh 一致，TrustOnFirstUse 首次也不静默信任；
        // 信任弹窗只由终端连接路径负责，SFTP 只复用其结果（Trust Once 的内存 pin 会
        // 通过 effective ssh config 合并进来）。
        None => Err(SftpError::new(
            SftpErrorKind::PermissionDenied,
            format!(
                "native SFTP host-key verification failed: no known host key for {}:{}",
                config.host, config.port
            ),
        )),
    }
}

fn authenticate_session(session: &Session, config: &SshConnectionConfig) -> SftpResult<()> {
    match &config.auth {
        AuthMethod::Password { username, password } => {
            session.userauth_password(username, password).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::PermissionDenied,
                    format!("native SFTP password authentication failed: {error}"),
                )
            })?;
        }
        AuthMethod::PrivateKey {
            username,
            key_path,
            passphrase,
        } => {
            session
                .userauth_pubkey_file(username, None, Path::new(key_path), passphrase.as_deref())
                .map_err(|error| {
                    SftpError::new(
                        SftpErrorKind::PermissionDenied,
                        format!("native SFTP private-key authentication failed: {error}"),
                    )
                })?;
        }
        AuthMethod::Agent { username } => {
            session.userauth_agent(username).map_err(|error| {
                SftpError::new(
                    SftpErrorKind::PermissionDenied,
                    format!("native SFTP ssh-agent authentication failed: {error}"),
                )
            })?;
        }
        AuthMethod::KeyboardInteractive { username, secret } => {
            let mut prompter = SftpKeyboardInteractivePrompt::new(secret.clone());
            session
                .userauth_keyboard_interactive(username, &mut prompter)
                .map_err(|error| {
                    SftpError::new(
                        SftpErrorKind::PermissionDenied,
                        format!(
                            "native SFTP keyboard-interactive authentication failed: {error}; prompts={}",
                            prompter.describe_prompts()
                        ),
                    )
                })?;
        }
    }
    if !session.authenticated() {
        return Err(SftpError::new(
            SftpErrorKind::PermissionDenied,
            "native SFTP session is still unauthenticated after auth attempt",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct SftpKeyboardInteractivePrompt {
    secret: String,
    prompt_log: Vec<String>,
}

impl SftpKeyboardInteractivePrompt {
    fn new(secret: String) -> Self {
        Self {
            secret,
            prompt_log: Vec::new(),
        }
    }

    fn describe_prompts(&self) -> String {
        if self.prompt_log.is_empty() {
            "none".to_owned()
        } else {
            self.prompt_log.join(" | ")
        }
    }
}

impl KeyboardInteractivePrompt for SftpKeyboardInteractivePrompt {
    fn prompt<'a>(&mut self, username: &str, instructions: &str, prompts: &[Prompt<'a>]) -> Vec<String> {
        self.prompt_log.clear();
        if !instructions.trim().is_empty() {
            self.prompt_log
                .push(format!("instructions={}", instructions.trim()));
        }
        self.prompt_log.extend(prompts.iter().map(|prompt| {
            format!("user={username} prompt=`{}` echo={}", prompt.text.trim(), prompt.echo)
        }));
        prompts
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                if index == 0 || (!prompt.echo && looks_like_secret_prompt(&prompt.text)) {
                    self.secret.clone()
                } else {
                    String::new()
                }
            })
            .collect()
    }
}

fn looks_like_secret_prompt(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("password")
        || normalized.contains("passcode")
        || normalized.contains("otp")
        || normalized.contains("token")
        || normalized.contains("verification code")
}

fn merge_permissions(existing_mode: Option<u32>, permissions: u32) -> u32 {
    let file_type = existing_mode.unwrap_or(FILE_TYPE_FILE) & FILE_TYPE_MASK;
    let file_type = if file_type == 0 {
        FILE_TYPE_FILE
    } else {
        file_type
    };
    file_type | (permissions & FILE_PERMISSIONS_MASK)
}

fn map_remote_entry(path: PathBuf, stat: FileStat) -> Option<FsEntry> {
    let path = normalize_remote_path(&path);
    let name = path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("/")
        .to_owned();
    if matches!(name.as_str(), "." | "..") {
        return None;
    }
    Some(FsEntry {
        kind: entry_kind_from_perm(stat.perm),
        size_bytes: stat.size.unwrap_or(0),
        permissions: stat.perm.unwrap_or(0) & FILE_PERMISSIONS_MASK,
        modified: stat
            .mtime
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds)),
        path,
        name,
    })
}

fn normalize_remote_path(path: &Path) -> String {
    let display = path.to_string_lossy().replace('\\', "/");
    if display.is_empty() {
        "/".to_owned()
    } else {
        display
    }
}

fn entry_kind_from_perm(perm: Option<u32>) -> FsEntryKind {
    match perm.unwrap_or(FILE_TYPE_FILE) & FILE_TYPE_MASK {
        FILE_TYPE_DIRECTORY => FsEntryKind::Directory,
        FILE_TYPE_FILE => FsEntryKind::File,
        FILE_TYPE_SYMLINK => FsEntryKind::Symlink,
        _ => FsEntryKind::Other,
    }
}

fn map_sftp_error(path: &str, error: ssh2::Error, action: &str) -> SftpError {
    use ssh2::ErrorCode;

    let kind = match error.code() {
        ErrorCode::SFTP(code) => match code {
            2 => SftpErrorKind::NotFound,
            3 => SftpErrorKind::PermissionDenied,
            4 => SftpErrorKind::Backend,
            6 => SftpErrorKind::Backend,
            11 => SftpErrorKind::AlreadyExists,
            _ => SftpErrorKind::Backend,
        },
        ErrorCode::Session(code) => match code {
            -31 => SftpErrorKind::PermissionDenied,
            _ => SftpErrorKind::Backend,
        },
    };
    SftpError::new(
        kind,
        format!("native SFTP {action} failed against `{path}`: {error}"),
    )
}

fn map_ssh_error(error: yshell_ssh::SshError) -> SftpError {
    SftpError::new(
        SftpErrorKind::Backend,
        format!("invalid SSH config for SFTP backend: {error}"),
    )
}

fn session_timeout_ms(timeout: Duration) -> u32 {
    let millis = timeout.as_millis().max(1);
    millis.min(u32::MAX as u128) as u32
}

fn host_key_algorithm_label(key_type: ssh2::HostKeyType) -> &'static str {
    match key_type {
        ssh2::HostKeyType::Unknown => "unknown",
        ssh2::HostKeyType::Rsa => "ssh-rsa",
        ssh2::HostKeyType::Dss => "ssh-dss",
        ssh2::HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
        ssh2::HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
        ssh2::HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
        ssh2::HostKeyType::Ed25519 => "ssh-ed25519",
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use tempfile::tempdir;
    use yshell_ssh::{AuthMethod, HostKeyPolicy, SshConnectionConfig};

    use super::{entry_kind_from_perm, merge_permissions, RealSftpBackend, FILE_TYPE_DIRECTORY};
    use crate::{SftpBackend, SftpClient};

    #[test]
    fn permissions_merge_keeps_existing_file_type_bits() {
        let mode = merge_permissions(Some(FILE_TYPE_DIRECTORY | 0o700), 0o755);
        assert_eq!(mode & super::FILE_TYPE_MASK, FILE_TYPE_DIRECTORY);
        assert_eq!(mode & super::FILE_PERMISSIONS_MASK, 0o755);
    }

    #[test]
    fn permission_bits_map_to_directory_entries() {
        assert_eq!(entry_kind_from_perm(Some(FILE_TYPE_DIRECTORY | 0o755)), crate::FsEntryKind::Directory);
    }

    #[test]
    fn live_native_sftp_round_trip_when_env_target_is_set() {
        let Some(target) = std::env::var("YSHELL_LIVE_SFTP_TARGET").ok() else {
            return;
        };
        let (username, host, port) = parse_live_target(&target);
        let mut config = SshConnectionConfig::new(host, port, AuthMethod::Agent { username });
        config.host_key_policy = HostKeyPolicy::AcceptAnyForTesting;
        let mut client = SftpClient::with_real_backend(config);
        let temp = tempdir().expect("tempdir");
        let local_upload_path = temp.path().join("upload.txt");
        let local_download_path = temp.path().join("download.txt");
        fs::write(&local_upload_path, b"yshell live sftp").expect("write upload");

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_millis();
        let remote_root = format!("/tmp/yshell-live-sftp-{unique}");
        let remote_dir = format!("{remote_root}/demo");
        let remote_uploaded = format!("{remote_dir}/upload.txt");
        let remote_renamed = format!("{remote_dir}/renamed.txt");

        client.mkdir(&remote_root).expect("mkdir root");
        client.mkdir(&remote_dir).expect("mkdir dir");
        client
            .upload_file(&local_upload_path, &remote_uploaded)
            .expect("upload");
        let listing = client.list_dir(&remote_dir).expect("list dir");
        assert!(listing.entries.iter().any(|entry| entry.name == "upload.txt"));
        client.chmod(&remote_uploaded, 0o600).expect("chmod");
        client
            .rename(&remote_uploaded, &remote_renamed)
            .expect("rename");
        client
            .download_file(&remote_renamed, &local_download_path)
            .expect("download");
        assert_eq!(
            fs::read(&local_download_path).expect("read download"),
            b"yshell live sftp"
        );

        // Tree transfer round trip (covers the shared tree engine on the real
        // backend, including `remove_dir`).
        let tree_source = temp.path().join("tree-src");
        fs::create_dir_all(tree_source.join("nested")).expect("create tree source");
        fs::write(tree_source.join("a.txt"), b"tree-a").expect("write tree a");
        fs::write(tree_source.join("nested/b.txt"), b"tree-b").expect("write tree b");
        let remote_tree_root = format!("{remote_root}/tree");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut progress = |_event: crate::TransferProgress| {};
        let upload_report = client
            .upload_tree(
                &tree_source,
                &remote_tree_root,
                crate::TreeTransferOptions::default(),
                &mut progress,
                &cancel,
            )
            .expect("upload tree");
        assert!(upload_report.failed.is_empty(), "{upload_report:?}");
        assert_eq!(upload_report.completed, 4);

        let tree_target = temp.path().join("tree-out");
        let download_report = client
            .download_tree(
                &remote_tree_root,
                &tree_target,
                crate::TreeTransferOptions::default(),
                &mut progress,
                &cancel,
            )
            .expect("download tree");
        assert!(download_report.failed.is_empty(), "{download_report:?}");
        assert_eq!(
            fs::read(tree_target.join("nested/b.txt")).expect("read tree b"),
            b"tree-b"
        );

        let results = client.delete_many(&[
            format!("{remote_tree_root}/a.txt"),
            format!("{remote_tree_root}/nested/b.txt"),
        ]);
        assert!(results.iter().all(crate::BatchOpResult::is_ok), "{results:?}");
        client
            .remove_dir(&format!("{remote_tree_root}/nested"))
            .expect("remove nested");
        client.remove_dir(&remote_tree_root).expect("remove tree root");

        client.delete(&remote_renamed).expect("delete file");
        client.remove_dir(&remote_dir).expect("delete dir");
        client.remove_dir(&remote_root).expect("delete root");
    }

    fn parse_live_target(target: &str) -> (String, String, u16) {
        let (username, host_port) = target
            .split_once('@')
            .unwrap_or_else(|| panic!("YSHELL_LIVE_SFTP_TARGET must be user@host:port"));
        let (host, port) = host_port
            .rsplit_once(':')
            .unwrap_or_else(|| panic!("YSHELL_LIVE_SFTP_TARGET must include :port"));
        let port = port.parse::<u16>().expect("live sftp port");
        (username.to_owned(), host.to_owned(), port)
    }

    #[test]
    fn real_backend_rejects_proxy_configs_for_now() {
        let mut config = SshConnectionConfig::new(
            "example.test",
            22,
            AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        config.proxy = yshell_ssh::ProxyConfig::Socks5 {
            address: "127.0.0.1:1080".to_owned(),
            username: None,
            password: None,
            resolve_dns_by_proxy: true,
        };
        let backend = RealSftpBackend::new(config);
        let error = backend.list_dir("/").expect_err("proxy unsupported");
        assert_eq!(error.kind, crate::SftpErrorKind::Backend);
        assert!(error.message.contains("proxy"));
    }
}
