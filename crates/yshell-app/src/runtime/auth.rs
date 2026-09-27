//! Authentication state: known-hosts manager, host-key policy helpers and the
//! managed private-key inventory (`keys` in the C0 schema).
//!
//! N4 Phase 1 lives here: private-key material parsing/public-key derivation
//! (D0c `ssh-key 0.6.7` usage), the secret-store side of the key inventory, and
//! the `authorized_keys` deployment helper (command assembly + idempotency
//! check; execution goes through the A0 `SshClient::exec` boundary).

use std::collections::BTreeMap;

use crate::{error::AppError, error::AppResult};
use ssh_key::{HashAlg, PrivateKey, PublicKey};
use yshell_config::HostKeyPolicy as ConfigHostKeyPolicy;
use yshell_config::KeyProfile;
use yshell_secret::SecretRef;
use yshell_ssh::{ExecOutput, HostKeyPolicy, KnownHosts, SshAdapter, SshClient, SshResult};

use super::*;

impl AppRuntime {
    pub fn open_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = true;
        self.ensure_known_hosts_selection();
        self.status_text = format!(
            "Known hosts manager opened with {} persisted entr{}.",
            self.known_host_entries().len(),
            if self.known_host_entries().len() == 1 {
                "y"
            } else {
                "ies"
            }
        );
        self.projection()
    }

    pub fn close_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = false;
        self.status_text = "Closed known hosts manager.".to_owned();
        self.projection()
    }

    pub fn select_previous_known_host(&mut self) -> AppProjection {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
        } else if let Some(selected) = self.known_hosts_selected_key.as_ref() {
            let current = entries
                .iter()
                .position(|entry| &entry.key == selected)
                .unwrap_or(0);
            let next = if current == 0 {
                entries.len() - 1
            } else {
                current - 1
            };
            self.known_hosts_selected_key = Some(entries[next].key.clone());
        } else {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
        self.status_text = self.known_hosts_selection_legacy_text();
        self.projection()
    }

    pub fn select_next_known_host(&mut self) -> AppProjection {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
        } else if let Some(selected) = self.known_hosts_selected_key.as_ref() {
            let current = entries
                .iter()
                .position(|entry| &entry.key == selected)
                .unwrap_or(0);
            let next = if current + 1 >= entries.len() {
                0
            } else {
                current + 1
            };
            self.known_hosts_selected_key = Some(entries[next].key.clone());
        } else {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
        self.status_text = self.known_hosts_selection_legacy_text();
        self.projection()
    }

    pub fn remove_selected_known_host(&mut self) -> AppResult<AppProjection> {
        let selected = self
            .selected_known_host_entry()
            .ok_or_else(|| AppError::new("no known host entry is currently selected"))?;
        self.persistent_known_hosts
            .remove(&selected.host, selected.port);
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.ensure_known_hosts_selection();
        self.status_text = format!(
            "Removed known host entry for {}:{} from `{}`.",
            selected.host,
            selected.port,
            self.config_store.known_hosts_file().display()
        );
        Ok(self.projection())
    }

    pub fn update_known_hosts_clear_confirmation(&mut self, value: &str) -> AppProjection {
        self.known_hosts_clear_confirmation = value.to_owned();
        self.projection()
    }

    pub fn clear_all_known_hosts(&mut self) -> AppResult<AppProjection> {
        if self.known_hosts_clear_confirmation.trim() != "CLEAR" {
            return Err(AppError::new(
                "type CLEAR before removing all persisted known hosts",
            ));
        }
        self.persistent_known_hosts = KnownHosts::new();
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.known_hosts_selected_key = None;
        self.known_hosts_clear_confirmation.clear();
        self.status_text = format!(
            "Cleared all persisted known hosts from `{}`.",
            self.config_store.known_hosts_file().display()
        );
        Ok(self.projection())
    }

    pub(crate) fn known_host_entries(&self) -> Vec<KnownHostEntryInfo> {
        let mut entries = self
            .persistent_known_hosts
            .snapshot()
            .into_iter()
            .filter_map(|(key, fingerprint)| {
                let (host, port_text) = key.rsplit_once(':')?;
                let host = host.to_owned();
                let port = port_text.parse::<u16>().ok()?;
                Some(KnownHostEntryInfo {
                    key,
                    host,
                    port,
                    algorithm: fingerprint.algorithm,
                    fingerprint: fingerprint.fingerprint,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }

    pub(crate) fn ensure_known_hosts_selection(&mut self) {
        let entries = self.known_host_entries();
        if entries.is_empty() {
            self.known_hosts_selected_key = None;
            return;
        }
        let still_valid = self
            .known_hosts_selected_key
            .as_ref()
            .is_some_and(|selected| entries.iter().any(|entry| &entry.key == selected));
        if !still_valid {
            self.known_hosts_selected_key = Some(entries[0].key.clone());
        }
    }

    pub(crate) fn selected_known_host_entry(&self) -> Option<KnownHostEntryInfo> {
        let selected = self.known_hosts_selected_key.as_ref()?;
        self.known_host_entries()
            .into_iter()
            .find(|entry| &entry.key == selected)
    }

    pub(crate) fn known_hosts_inventory_rows_text(&self) -> String {
        let entries = self.known_host_entries();
        entries
            .iter()
            .map(|entry| {
                let marker = if self
                    .known_hosts_selected_key
                    .as_ref()
                    .is_some_and(|selected| selected == &entry.key)
                {
                    ">"
                } else {
                    " "
                };
                format!(
                    "{marker} {}:{} [{}]",
                    entry.host, entry.port, entry.algorithm
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn known_hosts_inventory_empty(&self) -> bool {
        self.known_host_entries().is_empty()
    }

    pub(crate) fn known_hosts_selection_kind(&self) -> &'static str {
        let count = self.known_host_entries().len();
        match self.selected_known_host_entry() {
            Some(_) => "selected",
            None if count == 0 => "empty",
            None => "loaded",
        }
    }

    pub(crate) fn known_hosts_selection_host_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.host)
            .unwrap_or_default()
    }

    pub(crate) fn known_hosts_selection_port_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.port.to_string())
            .unwrap_or_default()
    }

    pub(crate) fn known_hosts_selection_index(&self) -> i32 {
        let Some(entry) = self.selected_known_host_entry() else {
            return 0;
        };
        i32::try_from(
            self.known_host_entries()
                .iter()
                .position(|candidate| candidate.key == entry.key)
                .map(|index| index + 1)
                .unwrap_or(1),
        )
        .unwrap_or(i32::MAX)
    }

    pub(crate) fn known_hosts_selection_total(&self) -> i32 {
        i32::try_from(self.known_host_entries().len()).unwrap_or(i32::MAX)
    }

    pub(crate) fn known_hosts_selection_legacy_text(&self) -> String {
        match self.known_hosts_selection_kind() {
            "selected" => format!(
                "Selected known host {}:{} ({} of {}).",
                self.known_hosts_selection_host_text(),
                self.known_hosts_selection_port_text(),
                self.known_hosts_selection_index(),
                self.known_hosts_selection_total()
            ),
            "empty" => "No persisted known hosts are available.".to_owned(),
            _ => format!(
                "Known hosts manager loaded {} entries.",
                self.known_hosts_selection_total()
            ),
        }
    }

    pub(crate) fn known_hosts_details_text(&self) -> String {
        match self.selected_known_host_entry() {
            Some(entry) => format!(
                "Host: {}\nPort: {}\nAlgorithm: {}\nFingerprint: {}",
                entry.host, entry.port, entry.algorithm, entry.fingerprint
            ),
            None => format!(
                "Known hosts path: {}\nTemporary trust entries are not persisted here.",
                self.config_store.known_hosts_file().display()
            ),
        }
    }

    pub(crate) fn resolve_secret_value(&self, secret_key: &str) -> AppResult<String> {
        let keychain = self.keychain.as_ref().ok_or_else(|| {
            AppError::new(format!(
                "saved session auth requires secret `{secret_key}`, but no keychain is configured"
            ))
        })?;
        keychain
            .0
            .get(&SecretRef::new(secret_key))
            .map(|secret| secret.expose_secret().to_owned())
            .map_err(AppError::from_error)
    }

    pub(crate) fn resolve_secret_value_with_override(
        &self,
        secret_key: &str,
        password_override: Option<&str>,
    ) -> AppResult<String> {
        match password_override {
            Some(password) => Ok(password.to_owned()),
            None => self.resolve_secret_value(secret_key),
        }
    }

    pub(crate) fn store_secret_value(&self, secret_key: &str, value: &str) -> AppResult<()> {
        let keychain = self.keychain.as_ref().ok_or_else(|| {
            AppError::new(format!(
                "saving secret `{secret_key}` requires an enabled secret store"
            ))
        })?;
        keychain
            .0
            .put(SecretRef::new(secret_key), value.into())
            .map_err(AppError::from_error)
    }

    pub(crate) fn delete_secret_value(&self, secret_key: &str) -> AppResult<()> {
        let keychain = self.keychain.as_ref().ok_or_else(|| {
            AppError::new(format!(
                "deleting secret `{secret_key}` requires an enabled secret store"
            ))
        })?;
        match keychain.0.delete(&SecretRef::new(secret_key)) {
            Ok(()) | Err(yshell_secret::SecretError::NotFound(_)) => Ok(()),
            Err(error) => Err(AppError::from_error(error)),
        }
    }

    // ------------------------------------------------------------------
    // N4：托管私钥清单（C0 `keys`）与 secret store 读写
    // ------------------------------------------------------------------

    /// 读取 C0 `keys` 配置 + secret store 状态，生成私钥页清单（按 label 排序）。
    pub(crate) fn private_key_entries(&self) -> Vec<PrivateKeyEntryInfo> {
        let used_by = self.private_key_used_by();
        let mut entries = self
            .config_document
            .keys
            .iter()
            .map(|profile| self.private_key_entry_info(profile, &used_by))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        entries
    }

    /// 单条私钥清单项（不存在时返回 `None`）。
    #[must_use]
    pub(crate) fn private_key_entry(&self, key_id: &str) -> Option<PrivateKeyEntryInfo> {
        let profile = self.config_document.find_key(key_id)?;
        let used_by = self.private_key_used_by();
        Some(self.private_key_entry_info(profile, &used_by))
    }

    /// 引用统计：`key_id -> 使用该密钥的已保存会话名`（文件夹树顺序）。
    fn private_key_used_by(&self) -> BTreeMap<String, Vec<String>> {
        let mut used_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for folder in &self.config_document.folders {
            collect_key_usage(folder, &self.config_document, &mut used_by);
        }
        used_by
    }

    fn private_key_entry_info(
        &self,
        profile: &KeyProfile,
        used_by: &BTreeMap<String, Vec<String>>,
    ) -> PrivateKeyEntryInfo {
        let material_ref = key_secret_ref(&profile.id, KeySecretKind::Material);
        let passphrase_ref = key_secret_ref(&profile.id, KeySecretKind::Passphrase);
        let material_ref = if profile.secret_ref.trim().is_empty() {
            material_ref
        } else {
            profile.secret_ref.clone()
        };
        PrivateKeyEntryInfo {
            id: profile.id.clone(),
            label: profile.label.clone(),
            algorithm: profile.algorithm.clone(),
            fingerprint: profile.fingerprint.clone(),
            public_key: profile.public_key.clone(),
            comment: profile.comment.clone().unwrap_or_default(),
            secret_ref: material_ref.clone(),
            material_present: self.secret_value_exists(&material_ref),
            passphrase_stored: self.secret_value_exists(&passphrase_ref),
            used_by: used_by.get(&profile.id).cloned().unwrap_or_default(),
        }
    }

    fn secret_value_exists(&self, secret_key: &str) -> bool {
        self.keychain
            .as_ref()
            .is_some_and(|keychain| keychain.0.get(&SecretRef::new(secret_key)).is_ok())
    }

    /// 导入私钥材料（OpenSSH 文本）：解析 → 派生公钥/指纹 → 材料（与可选口令）
    /// 写 secret store → `keys` 元数据入库。
    ///
    /// 加密私钥允许不带口令导入（OpenSSH 容器里公钥未加密，仍可派生）；带口令时
    /// 会先校验（错误口令 → `KeyMaterialError::InvalidPassphrase`）。`label` 为空时
    /// 回退到密钥注释、再回退到 key id。
    pub(crate) fn import_private_key(
        &mut self,
        label: &str,
        material: &str,
        passphrase: Option<&str>,
        remember_passphrase: bool,
    ) -> AppResult<PrivateKeyEntryInfo> {
        let material = material.trim();
        let parsed = parse_private_key_material(material, passphrase)
            .map_err(|error| AppError::new(format!("import failed: {}", error.message())))?;
        let passphrase = passphrase.filter(|value| !value.is_empty());
        let key_id = self.allocate_private_key_id();
        let material_ref = key_secret_ref(&key_id, KeySecretKind::Material);
        let passphrase_ref = key_secret_ref(&key_id, KeySecretKind::Passphrase);
        self.store_secret_value(&material_ref, material)?;
        if remember_passphrase {
            if let Some(passphrase) = passphrase {
                self.store_secret_value(&passphrase_ref, passphrase)?;
            } else {
                let _ = self.delete_secret_value(&passphrase_ref);
            }
        } else {
            let _ = self.delete_secret_value(&passphrase_ref);
        }
        let profile = KeyProfile {
            id: key_id.clone(),
            label: private_key_label(label, &parsed.comment, &key_id),
            algorithm: parsed.algorithm.clone(),
            fingerprint: parsed.fingerprint.clone(),
            public_key: parsed.public_key.clone(),
            secret_ref: material_ref,
            comment: if parsed.comment.is_empty() {
                None
            } else {
                Some(parsed.comment.clone())
            },
        };
        self.config_document.keys.push(profile);
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.private_key_entry(&key_id)
            .ok_or_else(|| AppError::new(format!("imported key `{key_id}` disappeared")))
    }

    /// 删除私钥：被已保存会话引用时需要 `force = true`（引用清单会写进错误文案）。
    pub(crate) fn remove_private_key(
        &mut self,
        key_id: &str,
        force: bool,
    ) -> AppResult<PrivateKeyEntryInfo> {
        let entry = self
            .private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))?;
        if !entry.used_by.is_empty() && !force {
            return Err(AppError::new(format!(
                "managed key `{}` is still used by {} saved session(s): {}. Reassign those sessions first, or confirm removal.",
                entry.label,
                entry.used_by.len(),
                entry.used_by.join(", ")
            )));
        }
        let material_ref = if entry.secret_ref.trim().is_empty() {
            key_secret_ref(key_id, KeySecretKind::Material)
        } else {
            entry.secret_ref.clone()
        };
        self.delete_secret_value(&material_ref)?;
        let passphrase_ref = key_secret_ref(key_id, KeySecretKind::Passphrase);
        if passphrase_ref != material_ref {
            self.delete_secret_value(&passphrase_ref)?;
        }
        self.config_document.keys.retain(|key| key.id != key_id);
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        Ok(entry)
    }

    /// 重命名私钥（空 label 拒绝）。
    pub(crate) fn rename_private_key(
        &mut self,
        key_id: &str,
        label: &str,
    ) -> AppResult<PrivateKeyEntryInfo> {
        let label = label.trim();
        if label.is_empty() {
            return Err(AppError::new("a private key label must not be empty"));
        }
        let profile = self
            .config_document
            .keys
            .iter_mut()
            .find(|key| key.id == key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))?;
        profile.label = label.to_owned();
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("renamed key `{key_id}` disappeared")))
    }

    /// 记住口令（先校验私钥材料）：错误口令不会覆盖 store 里已保存的口令。
    pub(crate) fn store_private_key_passphrase(
        &mut self,
        key_id: &str,
        passphrase: &str,
    ) -> AppResult<PrivateKeyEntryInfo> {
        let passphrase = passphrase.trim();
        if passphrase.is_empty() {
            return Err(AppError::new("a passphrase to remember must not be empty"));
        }
        let material = self.private_key_material(key_id)?;
        parse_private_key_material(&material, Some(passphrase)).map_err(|error| {
            AppError::new(format!(
                "cannot remember the passphrase: {}",
                error.message()
            ))
        })?;
        self.store_secret_value(
            &key_secret_ref(key_id, KeySecretKind::Passphrase),
            passphrase,
        )?;
        self.private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))
    }

    /// 忘记口令（不清除私钥材料）。
    pub(crate) fn forget_private_key_passphrase(
        &mut self,
        key_id: &str,
    ) -> AppResult<PrivateKeyEntryInfo> {
        self.delete_secret_value(&key_secret_ref(key_id, KeySecretKind::Passphrase))?;
        self.private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))
    }

    /// 从 secret store 取回私钥材料（连接/测试路径用）。
    pub(crate) fn private_key_material(&self, key_id: &str) -> AppResult<String> {
        let entry = self
            .private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))?;
        let secret_ref = if entry.secret_ref.trim().is_empty() {
            key_secret_ref(key_id, KeySecretKind::Material)
        } else {
            entry.secret_ref.clone()
        };
        self.resolve_secret_value(&secret_ref).map_err(|error| {
            AppError::new(format!(
                "managed key `{key_id}` has no private key material in the secret store: {error}"
            ))
        })
    }

    /// 已保存的口令（未保存时 `None`；不报错）。
    #[must_use]
    pub(crate) fn private_key_passphrase(&self, key_id: &str) -> Option<String> {
        self.resolve_secret_value(&key_secret_ref(key_id, KeySecretKind::Passphrase))
            .ok()
    }

    /// 公钥行（复制/导出/部署共用）。
    pub(crate) fn private_key_public_line(&self, key_id: &str) -> AppResult<String> {
        let entry = self
            .private_key_entry(key_id)
            .ok_or_else(|| AppError::new(format!("managed key `{key_id}` was not found")))?;
        if entry.public_key.trim().is_empty() {
            return Err(AppError::new(format!(
                "managed key `{key_id}` has no derived public key; test it against the stored material first"
            )));
        }
        Ok(entry.public_key)
    }

    /// "测试"：重新解析存储的材料（可选口令覆盖），返回派生结果与口令来源。
    ///
    /// 加密私钥且没有任何口令时返回 `Ok`（`decrypted = false`）：公钥仍可派生，
    /// UI 需要提示"口令未保存/未提供"；提供了错误口令则返回 `Err`。
    pub(crate) fn test_private_key(
        &self,
        key_id: &str,
        passphrase_override: Option<&str>,
    ) -> AppResult<PrivateKeyTestReport> {
        let material = self.private_key_material(key_id)?;
        let override_passphrase = passphrase_override.filter(|value| !value.is_empty());
        let stored = self.private_key_passphrase(key_id);
        let candidate = override_passphrase.or(stored.as_deref());
        let parsed = parse_private_key_material(&material, candidate)
            .map_err(|error| AppError::new(format!("key test failed: {}", error.message())))?;
        let passphrase_source = if !parsed.encrypted {
            KeyPassphraseSource::NotRequired
        } else if override_passphrase.is_some() {
            KeyPassphraseSource::Provided
        } else if stored.is_some() {
            KeyPassphraseSource::Stored
        } else {
            KeyPassphraseSource::Missing
        };
        Ok(PrivateKeyTestReport {
            key_id: key_id.to_owned(),
            parsed,
            passphrase_source,
        })
    }

    /// 把托管私钥材料物化成 `0600` 文件（`<config-dir>/keys/<key-id>.key`），供
    /// A0 的 `AuthMethod::PrivateKey { key_path, .. }` 使用（Phase 2 连接路径）。
    ///
    /// 明文私钥落盘是主要秘密，因此 Unix 权限固定 `0600`；同一 key id 反复物化
    /// 覆盖同一路径（幂等）。
    pub(crate) fn materialize_private_key_file(&self, key_id: &str) -> AppResult<PathBuf> {
        let material = self.private_key_material(key_id)?;
        let directory = self.config_dir.join("keys");
        fs::create_dir_all(&directory).map_err(AppError::from_error)?;
        let path = directory.join(format!("{key_id}.key"));
        fs::write(&path, material.as_bytes()).map_err(AppError::from_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(AppError::from_error)?;
        }
        Ok(path)
    }

    /// 生成 `authorized_keys` 部署（命令 + 幂等判断）所需的上下文。
    pub(crate) fn private_key_deployment(
        &self,
        key_id: &str,
    ) -> AppResult<AuthorizedKeysDeployment> {
        let public_key = self.private_key_public_line(key_id)?;
        Ok(AuthorizedKeysDeployment::new(public_key))
    }

    /// 下一个稳定的托管 key id（`key-1`、`key-2`…，避免与现有 id 冲突）。
    fn allocate_private_key_id(&self) -> String {
        let mut next = self.config_document.keys.len() + 1;
        loop {
            let candidate = format!("key-{next}");
            if self.config_document.find_key(&candidate).is_none() {
                return candidate;
            }
            next += 1;
        }
    }
}

/// 递归收集文件夹树里引用托管密钥的会话名。
fn collect_key_usage(
    folder: &yshell_config::FolderProfile,
    document: &yshell_config::ConfigDocument,
    used_by: &mut BTreeMap<String, Vec<String>>,
) {
    for session in &folder.sessions {
        let Some(auth_id) = session.auth_profile_id.as_deref() else {
            continue;
        };
        let Some(auth) = document.auth_profiles.get(auth_id) else {
            continue;
        };
        let Some(key_id) = auth.method.private_key_id() else {
            continue;
        };
        used_by
            .entry(key_id.to_owned())
            .or_default()
            .push(session.name.clone());
    }
    for child in &folder.folders {
        collect_key_usage(child, document, used_by);
    }
}

/// 托管密钥 label 的解析：显式 label → 密钥注释 → key id。
fn private_key_label(label: &str, comment: &str, key_id: &str) -> String {
    let label = label.trim();
    if !label.is_empty() {
        return label.to_owned();
    }
    if !comment.trim().is_empty() {
        return comment.trim().to_owned();
    }
    key_id.to_owned()
}

/// 托管私钥在 secret store 里的两种引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySecretKind {
    /// OpenSSH 私钥文本。
    Material,
    /// 私钥口令（可选；"记住口令"时保存）。
    Passphrase,
}

/// 托管私钥的 secret store 引用（与 C0 样例 `local://yshell/keys/key-1/material` 一致）。
#[must_use]
pub(crate) fn key_secret_ref(key_id: &str, kind: KeySecretKind) -> String {
    let suffix = match kind {
        KeySecretKind::Material => "material",
        KeySecretKind::Passphrase => "passphrase",
    };
    format!("local://yshell/keys/{key_id}/{suffix}")
}

/// 私钥解析/派生结果（导入、"测试"、连接路径共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPrivateKey {
    /// OpenSSH 算法名，如 `ssh-ed25519`。
    pub(crate) algorithm: String,
    /// `SHA256:...` 指纹（D18：用公钥派生，不做独立清单）。
    pub(crate) fingerprint: String,
    /// `to_openssh()` 的公钥行（含注释）。
    pub(crate) public_key: String,
    /// 私钥注释（加密且未解密时为空——注释在加密段内，见 D0c §4）。
    pub(crate) comment: String,
    /// 私钥是否带口令。
    pub(crate) encrypted: bool,
    /// 本次调用是否真的解密成功（无口令/未提供口令时为 `false`）。
    pub(crate) decrypted: bool,
}

/// 公钥行解析结果（主机密钥页导入用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPublicKeyLine {
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
    pub(crate) public_key: String,
}

/// 私钥材料错误（映射规则见 D0c §4：错误口令 → `Crypto`；旧版 PEM → 不支持）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyMaterialError {
    /// 材料为空。
    Empty,
    /// 非 OpenSSH 容器（含旧版 PEM/PKCS#1）。
    UnsupportedFormat,
    /// 口令错误（`ssh_key::Error::Crypto`）。
    InvalidPassphrase,
    /// 算法被当前 feature 集排除。
    UnsupportedAlgorithm,
    /// 看起来是 OpenSSH 但内容损坏/截断。
    Malformed,
}

impl KeyMaterialError {
    #[must_use]
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Empty => "the private key material is empty",
            Self::UnsupportedFormat => {
                "only OpenSSH-format keys are supported (BEGIN OPENSSH PRIVATE KEY)"
            }
            Self::InvalidPassphrase => "the passphrase did not decrypt the private key",
            Self::UnsupportedAlgorithm => "this key algorithm is not supported by this build",
            Self::Malformed => "the key is not a valid OpenSSH key (truncated or damaged)",
        }
    }
}

/// OpenSSH 私钥容器头（用于区分"格式不对"与"内容损坏"）。
const OPENSSH_PRIVATE_KEY_HEADER: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";

/// 解析 OpenSSH 私钥并派生公钥/指纹（`passphrase` 提供时校验口令）。
pub(crate) fn parse_private_key_material(
    material: &str,
    passphrase: Option<&str>,
) -> Result<ParsedPrivateKey, KeyMaterialError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(KeyMaterialError::Empty);
    }
    let looks_openssh = material.contains(OPENSSH_PRIVATE_KEY_HEADER);
    let key = PrivateKey::from_openssh(material)
        .map_err(|error| map_key_parse_error(&error, looks_openssh))?;
    let encrypted = key.is_encrypted();
    let mut decrypted = false;
    let key = match passphrase.filter(|value| !value.is_empty()) {
        Some(passphrase) if encrypted => {
            let key = key
                .decrypt(passphrase)
                .map_err(|error| map_decrypt_error(&error))?;
            decrypted = true;
            key
        }
        _ => key,
    };
    let public = key.public_key();
    let public_key = public
        .to_openssh()
        .map_err(|_| KeyMaterialError::Malformed)?;
    Ok(ParsedPrivateKey {
        algorithm: public.algorithm().as_str().to_owned(),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        public_key,
        comment: key.comment().to_owned(),
        encrypted,
        decrypted,
    })
}

/// 解析一行 OpenSSH 公钥（主机密钥页导入用）。
pub(crate) fn parse_public_key_line(line: &str) -> Result<ParsedPublicKeyLine, KeyMaterialError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(KeyMaterialError::Empty);
    }
    let looks_openssh =
        line.starts_with("ssh-") || line.starts_with("ecdsa-") || line.starts_with("sk-");
    let public = PublicKey::from_openssh(line)
        .map_err(|error| map_key_parse_error(&error, looks_openssh))?;
    Ok(ParsedPublicKeyLine {
        algorithm: public.algorithm().as_str().to_owned(),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        public_key: public
            .to_openssh()
            .map_err(|_| KeyMaterialError::Malformed)?,
    })
}

fn map_key_parse_error(error: &ssh_key::Error, looks_openssh: bool) -> KeyMaterialError {
    match error {
        ssh_key::Error::AlgorithmUnknown | ssh_key::Error::AlgorithmUnsupported { .. } => {
            KeyMaterialError::UnsupportedAlgorithm
        }
        ssh_key::Error::Encoding(_) | ssh_key::Error::FormatEncoding => {
            if looks_openssh {
                KeyMaterialError::Malformed
            } else {
                KeyMaterialError::UnsupportedFormat
            }
        }
        _ => {
            if looks_openssh {
                KeyMaterialError::Malformed
            } else {
                KeyMaterialError::UnsupportedFormat
            }
        }
    }
}

fn map_decrypt_error(error: &ssh_key::Error) -> KeyMaterialError {
    match error {
        // D0c：错误口令固定为 `Error::Crypto`（OpenSSH 无 MAC，误判概率 2^-32）。
        ssh_key::Error::Crypto => KeyMaterialError::InvalidPassphrase,
        other => map_key_parse_error(other, true),
    }
}

/// 口令来源（"测试"结果/清单提示用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyPassphraseSource {
    /// 私钥未加密。
    NotRequired,
    /// 使用了 secret store 里记住的口令。
    Stored,
    /// 使用了本次调用提供的口令（尚未保存）。
    Provided,
    /// 私钥加密但没有任何口令（公钥可派生，连接时需要输入）。
    Missing,
}

impl KeyPassphraseSource {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NotRequired => "not-required",
            Self::Stored => "stored",
            Self::Provided => "provided",
            Self::Missing => "missing",
        }
    }
}

/// "测试私钥"的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateKeyTestReport {
    pub(crate) key_id: String,
    pub(crate) parsed: ParsedPrivateKey,
    pub(crate) passphrase_source: KeyPassphraseSource,
}

impl PrivateKeyTestReport {
    /// 状态行文案（沿用运行时英文状态文本约定；Phase 2 可挂 i18n kind）。
    #[must_use]
    pub(crate) fn summary(&self) -> String {
        let lock_state = if self.parsed.encrypted {
            if self.parsed.decrypted {
                "encrypted (unlocked)"
            } else {
                "encrypted (locked)"
            }
        } else {
            "not encrypted"
        };
        format!(
            "{} · {} · {} · passphrase {}",
            self.parsed.algorithm,
            self.parsed.fingerprint,
            lock_state,
            self.passphrase_source.label()
        )
    }
}

/// 私钥清单项（Rust → Slint 的投影数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateKeyEntryInfo {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
    pub(crate) public_key: String,
    pub(crate) comment: String,
    /// 材料在 secret store 里的引用（C0 `KeyProfile.secret_ref`，空值回退默认）。
    pub(crate) secret_ref: String,
    /// 私钥材料当前是否真的在 secret store 里。
    pub(crate) material_present: bool,
    /// 是否记住了口令。
    pub(crate) passphrase_stored: bool,
    /// 引用该密钥的已保存会话名。
    pub(crate) used_by: Vec<String>,
}

impl PrivateKeyEntryInfo {
    /// 详情面板文本（指纹/公钥/引用）。
    #[must_use]
    pub(crate) fn details_text(&self) -> String {
        let mut text = format!(
            "Label: {}\nAlgorithm: {}\nFingerprint: {}\nSecret ref: {}",
            self.label, self.algorithm, self.fingerprint, self.secret_ref
        );
        if !self.comment.is_empty() {
            text.push_str(&format!("\nComment: {}", self.comment));
        }
        text.push_str(&format!(
            "\nMaterial: {}\nPassphrase: {}\nUsed by: {}",
            if self.material_present {
                "stored"
            } else {
                "missing"
            },
            if self.passphrase_stored {
                "saved"
            } else {
                "not saved"
            },
            self.used_by_label()
        ));
        if !self.public_key.is_empty() {
            text.push_str(&format!("\n\n{}", self.public_key));
        }
        text
    }

    /// 使用方文案（空 = 未被引用）。
    #[must_use]
    pub(crate) fn used_by_label(&self) -> String {
        if self.used_by.is_empty() {
            "no saved session".to_owned()
        } else {
            self.used_by.join(", ")
        }
    }
}

/// 默认远端 `authorized_keys`（`~` 由远端 shell 展开）。
pub(crate) const DEFAULT_AUTHORIZED_KEYS_PATH: &str = "~/.ssh/authorized_keys";

/// 部署公钥到远端 `authorized_keys` 的辅助：命令拼装（幂等 + 权限修正）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizedKeysDeployment {
    public_key_line: String,
    remote_path: String,
}

impl AuthorizedKeysDeployment {
    /// 用一条 OpenSSH 公钥行构造部署（去除首尾空白）。
    #[must_use]
    pub(crate) fn new(public_key_line: impl AsRef<str>) -> Self {
        Self {
            public_key_line: public_key_line.as_ref().trim().to_owned(),
            remote_path: DEFAULT_AUTHORIZED_KEYS_PATH.to_owned(),
        }
    }

    /// 幂等部署命令：追加前先 `grep -qxF`，已存在则只做权限修正。
    #[must_use]
    pub(crate) fn command(&self) -> String {
        let (directory, file) = split_remote_path(&self.remote_path);
        let quoted = shell_single_quote(&self.public_key_line);
        let steps = [
            "umask 077".to_owned(),
            format!("mkdir -p {directory}"),
            format!("chmod 700 {directory}"),
            format!("touch {file}"),
            format!("chmod 600 {file}"),
            format!("grep -qxF {quoted} {file} 2>/dev/null || printf '%s\\n' {quoted} >> {file}"),
        ];
        steps.join("; ")
    }
}

/// POSIX 单引号转义（`'` → `'\''`）。
#[must_use]
pub(crate) fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

/// 拆分远端路径为 `(目录, 文件)`（无 `/` 时目录为当前目录，`/file` 时目录为 `/`）。
fn split_remote_path(remote_path: &str) -> (String, String) {
    match remote_path.rsplit_once('/') {
        Some((directory, _)) if !directory.is_empty() => {
            (directory.to_owned(), remote_path.to_owned())
        }
        Some((_, file)) => ("/".to_owned(), file.to_owned()),
        None => (".".to_owned(), remote_path.to_owned()),
    }
}

/// 通过 A0 的 `exec` 接口执行部署命令（Phase 2 在认证后的会话上调用）。
pub(crate) fn deploy_authorized_keys_with_client<A: SshAdapter>(
    client: &SshClient<A>,
    session: &mut A::Session,
    deployment: &AuthorizedKeysDeployment,
) -> SshResult<ExecOutput> {
    client.exec(session, &deployment.command())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownHostEntryInfo {
    pub(crate) key: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
}

pub(crate) fn config_host_key_policy_label(policy: ConfigHostKeyPolicy) -> &'static str {
    match policy {
        ConfigHostKeyPolicy::Strict => "strict",
        ConfigHostKeyPolicy::TrustOnFirstUse => "trust-on-first-use",
        ConfigHostKeyPolicy::AcceptAnyForTesting => "accept-any-for-testing",
    }
}

pub(crate) fn config_host_key_policy_to_runtime(policy: ConfigHostKeyPolicy) -> HostKeyPolicy {
    match policy {
        ConfigHostKeyPolicy::Strict => HostKeyPolicy::Strict,
        ConfigHostKeyPolicy::TrustOnFirstUse => HostKeyPolicy::TrustOnFirstUse,
        ConfigHostKeyPolicy::AcceptAnyForTesting => HostKeyPolicy::AcceptAnyForTesting,
    }
}

pub(crate) fn runtime_host_key_policy_to_config(policy: &HostKeyPolicy) -> ConfigHostKeyPolicy {
    match policy {
        HostKeyPolicy::Strict => ConfigHostKeyPolicy::Strict,
        HostKeyPolicy::TrustOnFirstUse => ConfigHostKeyPolicy::TrustOnFirstUse,
        HostKeyPolicy::AcceptAnyForTesting => ConfigHostKeyPolicy::AcceptAnyForTesting,
    }
}

impl AppRuntime {
    #[cfg(test)]
    pub(crate) fn set_host_key_policy_override_for_testing(&mut self, policy: HostKeyPolicy) {
        self.host_key_policy_override = Some(policy);
    }
}

// ---------------------------------------------------------------------------
// N4 Phase 2：私钥管理页 / 主机密钥页的运行时接口（页面 ↔ 宿主）
// ---------------------------------------------------------------------------

/// 私钥清单行（Rust → Slint `PrivateKeyRow` 的镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateKeyRowData {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
    pub(crate) passphrase_stored: bool,
    pub(crate) material_present: bool,
    pub(crate) used_by: String,
    pub(crate) selected: bool,
}

/// 一条已保存的主机密钥（`key_line` 在 known_hosts 数据模型里不存在，留空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostKeyEntryData {
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
    pub(crate) key_line: String,
}

/// 按 `host:port` 分组的主机密钥（Rust → Slint `HostKeyGroup` 的镜像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostKeyGroupData {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) entries: Vec<HostKeyEntryData>,
}

impl AppRuntime {
    // --- 私钥管理页 -------------------------------------------------------

    pub fn open_private_keys_manager(&mut self) -> AppProjection {
        self.private_keys_modal_visible = true;
        self.private_keys_remove_confirm_visible = false;
        self.private_keys_deploy_confirm_visible = false;
        self.ensure_private_keys_selection();
        let count = self.config_document.keys.len();
        self.status_text = format!(
            "Private keys manager opened with {count} managed key{}.",
            if count == 1 { "" } else { "s" }
        );
        self.projection()
    }

    pub fn close_private_keys_manager(&mut self) -> AppProjection {
        self.private_keys_modal_visible = false;
        // 设计 §2：敏感值在关闭/取消时清空（口令输入、导入口令、确认态）。
        self.private_keys_import_passphrase.clear();
        self.private_keys_passphrase_input.clear();
        self.private_keys_remove_confirm_visible = false;
        self.private_keys_deploy_confirm_visible = false;
        self.status_text = "Closed private keys manager.".to_owned();
        self.projection()
    }

    pub fn update_private_keys_import_label(&mut self, value: &str) -> AppProjection {
        self.private_keys_import_label = value.to_owned();
        self.projection()
    }

    pub fn update_private_keys_import_path(&mut self, value: &str) -> AppProjection {
        self.private_keys_import_path = value.to_owned();
        self.projection()
    }

    pub fn update_private_keys_import_passphrase(&mut self, value: &str) -> AppProjection {
        self.private_keys_import_passphrase = value.to_owned();
        self.projection()
    }

    pub fn toggle_private_keys_remember_import_passphrase(
        &mut self,
        checked: bool,
    ) -> AppProjection {
        self.private_keys_remember_import_passphrase = checked;
        self.projection()
    }

    pub fn update_private_keys_passphrase_input(&mut self, value: &str) -> AppProjection {
        self.private_keys_passphrase_input = value.to_owned();
        self.projection()
    }

    /// 私钥页选择（index 来自页面行模型；越界忽略）。
    pub fn select_private_key(&mut self, index: i32) -> AppProjection {
        let rows = self.private_key_row_data();
        let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| rows.get(index))
        else {
            return self.projection();
        };
        self.private_keys_selected_id = Some(row.id.clone());
        self.private_keys_passphrase_input.clear();
        self.private_keys_test_result_text.clear();
        self.private_keys_remove_confirm_visible = false;
        self.private_keys_deploy_confirm_visible = false;
        self.status_text = format!("Selected managed key `{}`.", row.label);
        self.projection()
    }

    /// 从导入表单的路径读取 OpenSSH 私钥并导入（rfd 或手动输入路径都走这里）。
    pub fn import_private_key_from_form(&mut self) -> AppProjection {
        let path_text = self.private_keys_import_path.trim().to_owned();
        if path_text.is_empty() {
            self.private_keys_status_text =
                "Enter or browse a private key file path first.".to_owned();
            return self.projection();
        }
        let material = match fs::read_to_string(&path_text) {
            Ok(material) => material,
            Err(error) => {
                self.private_keys_status_text = format!("Failed to read `{path_text}`: {error}");
                return self.projection();
            }
        };
        let passphrase = self.private_keys_import_passphrase.clone();
        let passphrase = if passphrase.is_empty() {
            None
        } else {
            Some(passphrase.as_str())
        };
        let remember = self.private_keys_remember_import_passphrase;
        match self.import_private_key(
            &self.private_keys_import_label.clone(),
            &material,
            passphrase,
            remember,
        ) {
            Ok(entry) => {
                self.private_keys_selected_id = Some(entry.id.clone());
                self.private_keys_status_text = format!(
                    "Imported `{}` ({} · {}).",
                    entry.label, entry.algorithm, entry.fingerprint
                );
                self.private_keys_import_label.clear();
                self.private_keys_import_path.clear();
                self.private_keys_import_passphrase.clear();
                self.private_keys_remember_import_passphrase = false;
                self.status_text = self.private_keys_status_text.clone();
            }
            Err(error) => {
                self.private_keys_status_text = format!("Import failed: {error}");
            }
        }
        self.projection()
    }

    /// "测试"选中私钥：重新解析存储材料（口令输入非空时作为覆盖）。
    pub fn test_selected_private_key(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.private_keys_status_text = "Select a managed key first.".to_owned();
            return self.projection();
        };
        let override_passphrase = self.private_keys_passphrase_input.clone();
        let override_passphrase = if override_passphrase.is_empty() {
            None
        } else {
            Some(override_passphrase.as_str())
        };
        match self.test_private_key(&entry.id, override_passphrase) {
            Ok(report) => {
                self.private_keys_test_result_text = report.summary();
                self.private_keys_status_text = format!("Key test passed for `{}`.", entry.label);
            }
            Err(error) => {
                self.private_keys_test_result_text = String::new();
                self.private_keys_status_text = format!("Key test failed: {error}");
            }
        }
        self.projection()
    }

    /// 复制派生公钥到系统剪贴板前的取文本（宿主负责写剪贴板）。
    pub fn selected_private_key_public_line(&self) -> AppResult<String> {
        let entry = self
            .selected_private_key_entry()
            .ok_or_else(|| AppError::new("select a managed key first"))?;
        self.private_key_public_line(&entry.id)
    }

    /// 复制/导出后的状态反馈（`detail` 为空 = 成功）。
    pub fn set_private_keys_status(&mut self, detail: &str) -> AppProjection {
        self.private_keys_status_text = detail.to_owned();
        if !detail.is_empty() {
            self.status_text = detail.to_owned();
        }
        self.projection()
    }

    pub fn request_private_key_remove(&mut self) -> AppProjection {
        if let Some(entry) = self.selected_private_key_entry() {
            self.private_keys_remove_confirm_visible = true;
            self.private_keys_status_text =
                format!("Confirm removal of `{}` in the Danger Zone.", entry.label);
            self.status_text = self.private_keys_status_text.clone();
        }
        self.projection()
    }

    pub fn cancel_private_key_remove(&mut self) -> AppProjection {
        self.private_keys_remove_confirm_visible = false;
        self.projection()
    }

    pub fn confirm_private_key_remove(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.private_keys_remove_confirm_visible = false;
            return self.projection();
        };
        // 页面已展示"被引用告警"，这里按确认执行（force = true）。
        match self.remove_private_key(&entry.id, true) {
            Ok(removed) => {
                self.private_keys_remove_confirm_visible = false;
                self.private_keys_passphrase_input.clear();
                self.private_keys_test_result_text.clear();
                self.private_keys_status_text = format!("Removed managed key `{}`.", removed.label);
                self.ensure_private_keys_selection();
                self.status_text = self.private_keys_status_text.clone();
            }
            Err(error) => {
                self.private_keys_status_text = format!("Remove failed: {error}");
            }
        }
        self.projection()
    }

    pub fn request_private_key_deploy(&mut self) -> AppProjection {
        if self.selected_private_key_entry().is_some() {
            self.private_keys_deploy_confirm_visible = true;
        }
        self.projection()
    }

    pub fn cancel_private_key_deploy(&mut self) -> AppProjection {
        self.private_keys_deploy_confirm_visible = false;
        self.projection()
    }

    /// 确认部署：在活动会话的配置上新建一次性连接，执行幂等 `authorized_keys`
    /// 追加命令（A0 `SshClient::exec`），带权限修正。
    pub fn confirm_private_key_deploy(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.private_keys_deploy_confirm_visible = false;
            return self.projection();
        };
        let deployment = match self.private_key_deployment(&entry.id) {
            Ok(deployment) => deployment,
            Err(error) => {
                self.private_keys_deploy_confirm_visible = false;
                self.private_keys_status_text = format!("Deploy failed: {error}");
                return self.projection();
            }
        };
        let Some(session_key) = self.active_session_id.clone() else {
            self.private_keys_deploy_confirm_visible = false;
            self.private_keys_status_text =
                "Deploy requires an active session to the target host.".to_owned();
            return self.projection();
        };
        let Some(session) = self.sessions.get(&session_key) else {
            self.private_keys_deploy_confirm_visible = false;
            self.private_keys_status_text = "The active session is gone.".to_owned();
            return self.projection();
        };
        let ssh_config = session.ssh_config.clone();
        let target = format!(
            "{}@{}:{}",
            session.username_label(),
            session.host_label(),
            ssh_config.port
        );
        match self.deploy_authorized_keys_over_backend(&ssh_config, &deployment) {
            Ok(output) if output.exit_status == 0 => {
                self.private_keys_deploy_confirm_visible = false;
                self.private_keys_status_text = format!(
                    "Deployed `{}` to {target} (idempotent append, exit status 0).",
                    entry.label
                );
                self.status_text = self.private_keys_status_text.clone();
            }
            Ok(output) => {
                self.private_keys_deploy_confirm_visible = false;
                self.private_keys_status_text = format!(
                    "Deploy command failed on {target} with exit status {}.",
                    output.exit_status
                );
            }
            Err(error) => {
                self.private_keys_deploy_confirm_visible = false;
                self.private_keys_status_text = format!("Deploy failed on {target}: {error}");
            }
        }
        self.projection()
    }

    fn deploy_authorized_keys_over_backend(
        &self,
        ssh_config: &yshell_ssh::SshConnectionConfig,
        deployment: &AuthorizedKeysDeployment,
    ) -> yshell_ssh::SshResult<ExecOutput> {
        let effective = self.effective_ssh_config(ssh_config);
        match self.transport_backend {
            TransportBackend::Fake => {
                let client = SshClient::with_fake_backend();
                let mut session = client.connect(&effective)?;
                let output = deploy_authorized_keys_with_client(&client, &mut session, deployment)?;
                let _ = client.disconnect(session);
                Ok(output)
            }
            TransportBackend::Real => {
                let client = SshClient::with_real_backend();
                let mut session = client.connect(&effective)?;
                let output = deploy_authorized_keys_with_client(&client, &mut session, deployment)?;
                let _ = client.disconnect(session);
                Ok(output)
            }
        }
    }

    pub fn save_selected_private_key_passphrase(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            return self.projection();
        };
        let passphrase = self.private_keys_passphrase_input.clone();
        match self.store_private_key_passphrase(&entry.id, &passphrase) {
            Ok(updated) => {
                self.private_keys_passphrase_input.clear();
                self.private_keys_status_text = format!(
                    "Passphrase saved for `{}` (validated against the stored key).",
                    updated.label
                );
                self.status_text = self.private_keys_status_text.clone();
            }
            Err(error) => {
                self.private_keys_status_text = format!("Cannot save passphrase: {error}");
            }
        }
        self.projection()
    }

    pub fn forget_selected_private_key_passphrase(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            return self.projection();
        };
        match self.forget_private_key_passphrase(&entry.id) {
            Ok(updated) => {
                self.private_keys_status_text =
                    format!("Forgot the saved passphrase for `{}`.", updated.label);
                self.status_text = self.private_keys_status_text.clone();
            }
            Err(error) => {
                self.private_keys_status_text = format!("Cannot forget passphrase: {error}");
            }
        }
        self.projection()
    }

    pub fn rename_selected_private_key(&mut self, label: &str) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            return self.projection();
        };
        match self.rename_private_key(&entry.id, label) {
            Ok(updated) => {
                self.private_keys_status_text =
                    format!("Renamed managed key to `{}`.", updated.label);
                self.status_text = self.private_keys_status_text.clone();
            }
            Err(error) => {
                self.private_keys_status_text = format!("Rename failed: {error}");
            }
        }
        self.projection()
    }

    /// 私钥页行模型（选择标记已计算）。
    #[must_use]
    pub(crate) fn private_key_row_data(&self) -> Vec<PrivateKeyRowData> {
        let selected = self.private_keys_selected_id.as_deref();
        self.private_key_entries()
            .into_iter()
            .map(|entry| PrivateKeyRowData {
                selected: selected == Some(entry.id.as_str()),
                used_by: entry.used_by_label(),
                id: entry.id,
                label: entry.label,
                algorithm: entry.algorithm,
                fingerprint: entry.fingerprint,
                passphrase_stored: entry.passphrase_stored,
                material_present: entry.material_present,
            })
            .collect()
    }

    pub(crate) fn ensure_private_keys_selection(&mut self) {
        let entries = self.private_key_entries();
        if entries.is_empty() {
            self.private_keys_selected_id = None;
            return;
        }
        let still_valid = self
            .private_keys_selected_id
            .as_ref()
            .is_some_and(|selected| entries.iter().any(|entry| &entry.id == selected));
        if !still_valid {
            self.private_keys_selected_id = Some(entries[0].id.clone());
        }
    }

    #[must_use]
    pub(crate) fn selected_private_key_entry(&self) -> Option<PrivateKeyEntryInfo> {
        let selected = self.private_keys_selected_id.as_ref()?;
        self.private_key_entry(selected)
    }

    // --- 主机密钥页 -------------------------------------------------------

    pub fn open_host_keys_manager(&mut self) -> AppProjection {
        self.host_keys_modal_visible = true;
        self.host_keys_import_visible = false;
        self.ensure_host_keys_selection();
        self.status_text = format!(
            "Host keys manager opened with {} persisted host key group(s).",
            self.host_key_group_data().len()
        );
        self.projection()
    }

    pub fn close_host_keys_manager(&mut self) -> AppProjection {
        self.host_keys_modal_visible = false;
        self.host_keys_import_visible = false;
        self.host_keys_import_text.clear();
        self.status_text = "Closed host keys manager.".to_owned();
        self.projection()
    }

    pub fn select_host_key(&mut self, group: i32, entry: i32) -> AppProjection {
        let groups = self.host_key_group_data();
        let Some(group_index) = usize::try_from(group).ok() else {
            return self.projection();
        };
        let Some(entry_index) = usize::try_from(entry).ok() else {
            return self.projection();
        };
        if group_index >= groups.len() || entry_index >= groups[group_index].entries.len() {
            return self.projection();
        }
        self.host_keys_selected_group = Some(group_index);
        self.host_keys_selected_entry = Some(entry_index);
        let selected = &groups[group_index];
        self.status_text = format!(
            "Selected host key {}:{} ({}).",
            selected.host, selected.port, selected.entries[entry_index].algorithm
        );
        self.projection()
    }

    pub fn toggle_host_keys_import(&mut self) -> AppProjection {
        self.host_keys_import_visible = !self.host_keys_import_visible;
        if !self.host_keys_import_visible {
            self.host_keys_import_text.clear();
        }
        self.projection()
    }

    pub fn update_host_keys_import_text(&mut self, value: &str) -> AppProjection {
        self.host_keys_import_text = value.to_owned();
        self.projection()
    }

    pub fn update_host_keys_clear_confirmation(&mut self, value: &str) -> AppProjection {
        self.known_hosts_clear_confirmation = value.to_owned();
        self.projection()
    }

    /// 导入主机密钥：每行 `host:port <公钥行>` 或 `host:port <SHA256:指纹>`。
    pub fn import_host_keys_from_text(&mut self) -> AppProjection {
        let mut imported = 0usize;
        let mut failures = Vec::new();
        for line in self.host_keys_import_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((target, rest)) = line.split_once(char::is_whitespace) else {
                failures.push(format!("missing key data: `{line}`"));
                continue;
            };
            let Some((host, port_text)) = target.rsplit_once(':') else {
                failures.push(format!("missing host:port: `{line}`"));
                continue;
            };
            let Ok(port) = port_text.parse::<u16>() else {
                failures.push(format!("invalid port: `{line}`"));
                continue;
            };
            let rest = rest.trim();
            let tokens = rest.split_whitespace().collect::<Vec<_>>();
            let fingerprint = if tokens.len() >= 2 && tokens[1].starts_with("SHA256:") {
                Some((tokens[0].to_owned(), tokens[1].to_owned()))
            } else if tokens
                .first()
                .is_some_and(|token| token.starts_with("SHA256:"))
            {
                Some((
                    tokens.get(1).copied().unwrap_or("unknown").to_owned(),
                    tokens[0].to_owned(),
                ))
            } else {
                match parse_public_key_line(rest) {
                    Ok(parsed) => Some((parsed.algorithm, parsed.fingerprint)),
                    Err(error) => {
                        failures.push(format!("`{line}`: {}", error.message()));
                        continue;
                    }
                }
            };
            let Some((algorithm, fingerprint)) = fingerprint else {
                continue;
            };
            self.persistent_known_hosts.pin(
                host,
                port,
                yshell_ssh::HostKeyFingerprint {
                    algorithm,
                    fingerprint,
                },
            );
            imported += 1;
        }
        if imported > 0 {
            if let Err(error) = self
                .config_store
                .save_known_hosts(&self.persistent_known_hosts)
            {
                self.host_keys_status_text = format!("Import failed to persist: {error}");
                return self.projection();
            }
            self.ensure_host_keys_selection();
        }
        self.host_keys_status_text = if failures.is_empty() {
            format!("Imported {imported} host key(s).")
        } else {
            format!(
                "Imported {imported} host key(s); {} line(s) skipped: {}",
                failures.len(),
                failures.join("; ")
            )
        };
        self.status_text = self.host_keys_status_text.clone();
        self.projection()
    }

    /// 导出主机密钥文本（`host:port algorithm fingerprint` 每行一条）。
    #[must_use]
    pub fn export_host_keys_text(&self) -> String {
        let mut lines = self
            .known_host_entries()
            .into_iter()
            .map(|entry| {
                format!(
                    "{}:{} {} {}",
                    entry.host, entry.port, entry.algorithm, entry.fingerprint
                )
            })
            .collect::<Vec<_>>();
        lines.sort();
        lines.join("\n")
    }

    /// 主机密钥页状态反馈（rfd 导出等宿主侧动作）。
    pub fn set_host_keys_status(&mut self, detail: &str) -> AppProjection {
        self.host_keys_status_text = detail.to_owned();
        if !detail.is_empty() {
            self.status_text = detail.to_owned();
        }
        self.projection()
    }

    pub fn remove_selected_host_key(&mut self) -> AppResult<AppProjection> {
        let selected = self
            .selected_host_key_entry()
            .ok_or_else(|| AppError::new("no host key is currently selected"))?;
        self.persistent_known_hosts
            .remove(&selected.host, selected.port);
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.ensure_host_keys_selection();
        self.host_keys_status_text = format!(
            "Removed the saved host key for {}:{}.",
            selected.host, selected.port
        );
        self.status_text = self.host_keys_status_text.clone();
        Ok(self.projection())
    }

    pub fn clear_all_host_keys(&mut self) -> AppResult<AppProjection> {
        if self.known_hosts_clear_confirmation.trim() != "CLEAR" {
            return Err(AppError::new(
                "type CLEAR before removing all saved host keys",
            ));
        }
        self.persistent_known_hosts = KnownHosts::new();
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.host_keys_selected_group = None;
        self.host_keys_selected_entry = None;
        self.known_hosts_clear_confirmation.clear();
        self.host_keys_status_text = "Cleared all saved host keys.".to_owned();
        self.status_text = self.host_keys_status_text.clone();
        Ok(self.projection())
    }

    /// 主机密钥页分组模型（按 `host:port` 排序，组内按算法）。
    #[must_use]
    pub(crate) fn host_key_group_data(&self) -> Vec<HostKeyGroupData> {
        let mut groups: BTreeMap<(String, u16), Vec<HostKeyEntryData>> = BTreeMap::new();
        for entry in self.known_host_entries() {
            groups
                .entry((entry.host.clone(), entry.port))
                .or_default()
                .push(HostKeyEntryData {
                    algorithm: entry.algorithm,
                    fingerprint: entry.fingerprint,
                    key_line: String::new(),
                });
        }
        groups
            .into_iter()
            .map(|((host, port), entries)| HostKeyGroupData {
                host,
                port,
                entries,
            })
            .collect()
    }

    pub(crate) fn ensure_host_keys_selection(&mut self) {
        let groups = self.host_key_group_data();
        if groups.is_empty() {
            self.host_keys_selected_group = None;
            self.host_keys_selected_entry = None;
            return;
        }
        let valid = self
            .host_keys_selected_group
            .zip(self.host_keys_selected_entry)
            .is_some_and(|(group, entry)| {
                groups
                    .get(group)
                    .is_some_and(|selected| entry < selected.entries.len())
            });
        if !valid {
            self.host_keys_selected_group = Some(0);
            self.host_keys_selected_entry = Some(0);
        }
    }

    #[must_use]
    pub(crate) fn selected_host_key_entry(&self) -> Option<KnownHostEntryInfo> {
        let group = self.host_keys_selected_group?;
        let entry = self.host_keys_selected_entry?;
        let groups = self.host_key_group_data();
        let selected = groups.get(group)?;
        let selected_entry = selected.entries.get(entry)?;
        self.known_host_entries().into_iter().find(|candidate| {
            candidate.host == selected.host
                && candidate.port == selected.port
                && candidate.algorithm == selected_entry.algorithm
                && candidate.fingerprint == selected_entry.fingerprint
        })
    }

    /// 主机密钥页详情文本（与 legacy known_hosts 页分开，避免共享选择态）。
    #[must_use]
    pub(crate) fn host_keys_details_text(&self) -> String {
        match self.selected_host_key_entry() {
            Some(entry) => format!(
                "Host: {}\nPort: {}\nAlgorithm: {}\nFingerprint: {}",
                entry.host, entry.port, entry.algorithm, entry.fingerprint
            ),
            None => format!(
                "Host keys path: {}\nTemporary trust entries are not persisted here.",
                self.config_store.known_hosts_file().display()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::tempdir;
    use yshell_config::{
        AuthMethod as ConfigAuthMethod, AuthProfile, ConfigStore, FolderProfile, SessionProfile,
    };
    use yshell_secret::FakeKeychain;

    use super::*;

    /// D0c spike 生成的一次性测试密钥（仅用于本模块单测；不参与任何生产流程）。
    const ED25519_PLAIN: &str = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n",
        "QyNTUxOQAAACBSYrC396PBUCm8lAJ7daz70WfJiRWrxGkVfMeR9mqo7QAAAJgld3oAJXd6\n",
        "AAAAAAtzc2gtZWQyNTUxOQAAACBSYrC396PBUCm8lAJ7daz70WfJiRWrxGkVfMeR9mqo7Q\n",
        "AAAEBjQwofDuXa019zF7tFRxguRzCAMmLfd6RLo4WsAftZh1JisLf3o8FQKbyUAnt1rPvR\n",
        "Z8mJFavEaRV8x5H2aqjtAAAAE3NwaWtlLWVkMjU1MTktcGxhaW4BAg==\n",
        "-----END OPENSSH PRIVATE KEY-----\n",
    );
    const ED25519_PLAIN_PUBLIC: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFJisLf3o8FQKbyUAnt1rPvRZ8mJFavEaRV8x5H2aqjt spike-ed25519-plain";
    const ED25519_PLAIN_FINGERPRINT: &str = "SHA256:gufPep5T6QepKH9Ggl3KT9znGwIxL0pWn3fxvDUxNPA";

    const ED25519_LOCKED: &str = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABAMwcRdRu\n",
        "oZ1wVQ/f+6Do5UAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIHZevcN/NGZeckhm\n",
        "OzW/no9j8hR9bffYlTnOf+2X2hZIAAAAoGENoJ+EioemQ+gvrrsx1wIan7PVJVkSxqAkeq\n",
        "8pEuhvmyX9jMIiGzLaNYYYwU1iXIZFMlruQaZDMLTJJmGvcNWSu0wdceBNmjEfGyElVJ3X\n",
        "m9SXjr7hwIgZ3dJVn+7/kKgQBAscc6vS0oLvTQBjCoH/eFDSBAtCQvrqYcfiEym3TlZdK3\n",
        "VG0V1NU7Vn02i5iqfBg7DtjerGSZhN7d1d8Uk=\n",
        "-----END OPENSSH PRIVATE KEY-----\n",
    );
    const ED25519_LOCKED_PASSPHRASE: &str = "spike-pass-123";
    const ED25519_LOCKED_PUBLIC: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHZevcN/NGZeckhmOzW/no9j8hR9bffYlTnOf+2X2hZI";
    const ED25519_LOCKED_FINGERPRINT: &str = "SHA256:3ld4ZQO7YV+I/rHXSxRg/ZoPnKda9KQWUYl2FvTtZ/o";

    const ECDSA256: &str = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAaAAAABNlY2RzYS\n",
        "1zaGEyLW5pc3RwMjU2AAAACG5pc3RwMjU2AAAAQQQgTgZXiBuz2Y7w9alPAvic8/JwCFqf\n",
        "cUgNTgdB8cSddvhB6fOYS2zp8jis4XX9qcqm53eeqvJki8c4u7RogeY2AAAAqICz3B6As9\n",
        "weAAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBCBOBleIG7PZjvD1\n",
        "qU8C+Jzz8nAIWp9xSA1OB0HxxJ12+EHp85hLbOnyOKzhdf2pyqbnd56q8mSLxzi7tGiB5j\n",
        "YAAAAhANi/TTHYMG4ENZb+wEBLXZKyqYrgzG/OG6Z/zyIDThJkAAAADnNwaWtlLWVjZHNh\n",
        "MjU2AQ==\n",
        "-----END OPENSSH PRIVATE KEY-----\n",
    );
    const ECDSA256_FINGERPRINT: &str = "SHA256:5fvHxmGDMUQsMNtDPzeqV1yKCawUzW1O3pVr1GJuKo0";

    /// 旧版 PKCS#1 PEM（`ssh-keygen -m PEM`）——只验证错误映射，不参与解析。
    const LEGACY_PEM: &str = concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "MIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu\n",
        "KUpRKfFLfRYC9AIKjbJTWit+CqvjWYzvQwECAwEAAQJAIJLixBy2qpFoS4DSmoEm\n",
        "-----END RSA PRIVATE KEY-----\n",
    );

    fn test_runtime_with_keychain() -> (tempfile::TempDir, AppRuntime) {
        let temp = tempdir().expect("tempdir");
        let keychain: Arc<dyn Keychain> = Arc::new(FakeKeychain::new("n4-master"));
        let runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), Some(keychain))
            .expect("runtime");
        (temp, runtime)
    }

    #[test]
    fn parses_plain_ed25519_and_derives_public_key() {
        let parsed = parse_private_key_material(ED25519_PLAIN, None).expect("parse");

        assert_eq!(parsed.algorithm, "ssh-ed25519");
        assert_eq!(parsed.fingerprint, ED25519_PLAIN_FINGERPRINT);
        assert_eq!(parsed.public_key, ED25519_PLAIN_PUBLIC);
        assert_eq!(parsed.comment, "spike-ed25519-plain");
        assert!(!parsed.encrypted);
        assert!(!parsed.decrypted);
    }

    #[test]
    fn parses_ecdsa_key_with_the_ecdsa_feature_set() {
        let parsed = parse_private_key_material(ECDSA256, None).expect("parse");

        assert_eq!(parsed.algorithm, "ecdsa-sha2-nistp256");
        assert_eq!(parsed.fingerprint, ECDSA256_FINGERPRINT);
        assert_eq!(parsed.comment, "spike-ecdsa256");
        assert!(parsed.public_key.starts_with("ecdsa-sha2-nistp256 "));
    }

    #[test]
    fn encrypted_key_derives_public_key_without_and_with_passphrase() {
        // 无口令：公钥/指纹仍可派生（公钥在 OpenSSH 容器里未加密），注释为空。
        let locked = parse_private_key_material(ED25519_LOCKED, None).expect("locked parse");
        assert!(locked.encrypted);
        assert!(!locked.decrypted);
        assert_eq!(locked.fingerprint, ED25519_LOCKED_FINGERPRINT);
        assert_eq!(locked.public_key, ED25519_LOCKED_PUBLIC);
        assert_eq!(locked.comment, "");

        // 正确口令：解密成功、注释恢复（D0c §4）。
        let unlocked = parse_private_key_material(ED25519_LOCKED, Some(ED25519_LOCKED_PASSPHRASE))
            .expect("unlock");
        assert!(unlocked.encrypted);
        assert!(unlocked.decrypted);
        assert_eq!(unlocked.comment, "spike-ed25519-locked");
        assert_eq!(
            unlocked.public_key,
            format!("{ED25519_LOCKED_PUBLIC} spike-ed25519-locked")
        );
        assert_eq!(unlocked.fingerprint, ED25519_LOCKED_FINGERPRINT);
    }

    #[test]
    fn maps_wrong_passphrase_to_invalid_passphrase() {
        let error = parse_private_key_material(ED25519_LOCKED, Some("definitely-wrong"))
            .expect_err("wrong passphrase must fail");

        assert_eq!(error, KeyMaterialError::InvalidPassphrase);
        assert!(error.message().contains("passphrase"));
    }

    #[test]
    fn maps_legacy_pem_and_garbage_to_unsupported_format() {
        assert_eq!(
            parse_private_key_material(LEGACY_PEM, None).expect_err("legacy PEM"),
            KeyMaterialError::UnsupportedFormat
        );
        assert_eq!(
            parse_private_key_material("ssh-ed25519 AAAA not-a-private-key", None)
                .expect_err("public key pasted as private"),
            KeyMaterialError::UnsupportedFormat
        );
        assert_eq!(
            parse_private_key_material("   ", None).expect_err("empty"),
            KeyMaterialError::Empty
        );
    }

    #[test]
    fn maps_truncated_openssh_key_to_malformed() {
        let truncated = format!("{ED25519_PLAIN}extra-garbage-not-base64!!!");
        let error = parse_private_key_material(&truncated, None).expect_err("truncated");

        assert_eq!(error, KeyMaterialError::Malformed);
    }

    #[test]
    fn parses_public_key_line_for_host_key_import() {
        let parsed = parse_public_key_line(ED25519_LOCKED_PUBLIC).expect("public key");

        assert_eq!(parsed.algorithm, "ssh-ed25519");
        assert_eq!(parsed.fingerprint, ED25519_LOCKED_FINGERPRINT);
        assert_eq!(parsed.public_key, ED25519_LOCKED_PUBLIC);
        assert_eq!(
            parse_public_key_line("not a key").expect_err("garbage"),
            KeyMaterialError::UnsupportedFormat
        );
    }

    #[test]
    fn deployment_command_is_idempotent_and_fixes_permissions() {
        let deployment = AuthorizedKeysDeployment::new(ED25519_PLAIN_PUBLIC);
        let command = deployment.command();

        assert!(command.starts_with("umask 077; mkdir -p ~/.ssh; chmod 700 ~/.ssh;"));
        assert!(command.contains("touch ~/.ssh/authorized_keys"));
        assert!(command.contains("chmod 600 ~/.ssh/authorized_keys"));
        assert!(command.contains(&format!(
            "grep -qxF '{}' ~/.ssh/authorized_keys 2>/dev/null || printf '%s\\n' '{}' >> ~/.ssh/authorized_keys",
            ED25519_PLAIN_PUBLIC, ED25519_PLAIN_PUBLIC
        )));
    }

    #[test]
    fn shell_single_quote_escapes_embedded_quotes() {
        assert_eq!(shell_single_quote("plain"), "'plain'");
        assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_single_quote(""), "''");
    }

    #[test]
    fn deployment_executes_through_the_a0_exec_boundary() {
        let client = yshell_ssh::SshClient::with_fake_backend();
        let config = yshell_ssh::SshConnectionConfig::new(
            "example.test",
            22,
            yshell_ssh::AuthMethod::Agent {
                username: "alice".to_owned(),
            },
        );
        let mut session = client.connect(&config).expect("fake connect");
        let deployment = AuthorizedKeysDeployment::new(ED25519_PLAIN_PUBLIC);

        let output = deploy_authorized_keys_with_client(&client, &mut session, &deployment)
            .expect("fake exec");

        assert_eq!(output.exit_status, 0);
        assert_eq!(session.executed_commands, vec![deployment.command()]);
    }

    #[test]
    fn import_persists_metadata_and_secrets_and_is_listed() {
        let (_temp, mut runtime) = test_runtime_with_keychain();

        let entry = runtime
            .import_private_key("Work key", ED25519_PLAIN, None, false)
            .expect("import");

        assert_eq!(entry.id, "key-1");
        assert_eq!(entry.label, "Work key");
        assert_eq!(entry.algorithm, "ssh-ed25519");
        assert_eq!(entry.fingerprint, ED25519_PLAIN_FINGERPRINT);
        assert!(entry.material_present);
        assert!(!entry.passphrase_stored);
        assert!(entry.used_by.is_empty());
        assert_eq!(
            entry.secret_ref,
            key_secret_ref("key-1", KeySecretKind::Material)
        );

        let material = runtime.private_key_material("key-1").expect("material");
        assert_eq!(material, ED25519_PLAIN.trim());

        // 配置已落盘：重新加载后 keys 元数据仍在。
        let reloaded = runtime
            .config_store
            .load_or_recover()
            .expect("reload")
            .document;
        let stored = reloaded.find_key("key-1").expect("persisted key");
        assert_eq!(stored.label, "Work key");
        assert_eq!(stored.public_key, ED25519_PLAIN_PUBLIC);

        let entries = runtime.private_key_entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0]
            .details_text()
            .contains(ED25519_PLAIN_FINGERPRINT));
    }

    #[test]
    fn import_maps_errors_and_requires_a_secret_store() {
        let (_temp, mut runtime) = test_runtime_with_keychain();

        let error = runtime
            .import_private_key("Bad", LEGACY_PEM, None, false)
            .expect_err("legacy PEM");
        assert!(error.message.contains("OpenSSH"));

        let error = runtime
            .import_private_key("Wrong pass", ED25519_LOCKED, Some("nope"), false)
            .expect_err("wrong passphrase");
        assert!(error.message.contains("did not decrypt"));

        let temp = tempdir().expect("tempdir");
        let mut no_keychain = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None)
            .expect("runtime without keychain");
        let error = no_keychain
            .import_private_key("No store", ED25519_PLAIN, None, false)
            .expect_err("no keychain");
        assert!(error.message.contains("secret store"));
    }

    #[test]
    fn import_locked_key_remembers_passphrase_and_tests_it() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        let entry = runtime
            .import_private_key(
                "Locked",
                ED25519_LOCKED,
                Some(ED25519_LOCKED_PASSPHRASE),
                true,
            )
            .expect("import locked");
        assert!(entry.passphrase_stored);
        assert_eq!(entry.comment, "spike-ed25519-locked");

        let report = runtime.test_private_key(&entry.id, None).expect("test");
        assert_eq!(report.passphrase_source, KeyPassphraseSource::Stored);
        assert!(report.parsed.decrypted);
        assert!(report.summary().contains("unlocked"));

        // 错误口令不会覆盖已保存的口令，也不会让"测试"失真。
        let error = runtime
            .store_private_key_passphrase(&entry.id, "wrong")
            .expect_err("wrong passphrase");
        assert!(error.message.contains("did not decrypt"));
        assert!(
            runtime
                .test_private_key(&entry.id, None)
                .expect("test")
                .parsed
                .decrypted
        );
        assert!(runtime
            .test_private_key(&entry.id, Some("wrong"))
            .expect_err("override must fail")
            .message
            .contains("did not decrypt"));

        let entry = runtime
            .forget_private_key_passphrase(&entry.id)
            .expect("forget");
        assert!(!entry.passphrase_stored);
        let report = runtime.test_private_key(&entry.id, None).expect("test");
        assert_eq!(report.passphrase_source, KeyPassphraseSource::Missing);
        assert!(!report.parsed.decrypted);
        assert!(report.summary().contains("locked"));
    }

    #[test]
    fn remove_private_key_warns_about_referencing_sessions_and_deletes_secrets() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        let entry = runtime
            .import_private_key("Shared", ED25519_PLAIN, None, false)
            .expect("import");
        runtime.config_document.auth_profiles.insert(
            "auth-shared".to_owned(),
            AuthProfile {
                id: "auth-shared".to_owned(),
                name: "Shared key".to_owned(),
                method: ConfigAuthMethod::PrivateKey {
                    key_id: Some(entry.id.clone()),
                    path: String::new(),
                    passphrase_secret_key: None,
                },
            },
        );
        let mut root = FolderProfile::new("root", "Root");
        let mut session = SessionProfile::new("saved-prod", "Prod", "prod.example.test");
        session.auth_profile_id = Some("auth-shared".to_owned());
        root.sessions.push(session);
        runtime.config_document.folders.push(root);

        let listed = runtime.private_key_entry(&entry.id).expect("listed");
        assert_eq!(listed.used_by, vec!["Prod".to_owned()]);

        let error = runtime
            .remove_private_key(&entry.id, false)
            .expect_err("referenced key needs confirmation");
        assert!(error.message.contains("Prod"));

        let removed = runtime.remove_private_key(&entry.id, true).expect("forced");
        assert_eq!(removed.id, entry.id);
        assert!(runtime.private_key_entries().is_empty());
        assert!(runtime.private_key_material(&entry.id).is_err());
        assert!(runtime
            .config_store
            .load_or_recover()
            .expect("reload")
            .document
            .find_key(&entry.id)
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn materialize_writes_0600_private_key_file() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp, mut runtime) = test_runtime_with_keychain();
        let entry = runtime
            .import_private_key("Materialized", ED25519_PLAIN, None, false)
            .expect("import");

        let path = runtime
            .materialize_private_key_file(&entry.id)
            .expect("materialize");
        assert!(path.ends_with("keys/key-1.key"));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            ED25519_PLAIN.trim()
        );
        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // 幂等：重复物化覆盖同一路径。
        let again = runtime
            .materialize_private_key_file(&entry.id)
            .expect("materialize again");
        assert_eq!(path, again);
    }

    #[test]
    fn key_deployment_uses_the_persisted_public_key() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        let entry = runtime
            .import_private_key(
                "Deployable",
                ED25519_LOCKED,
                Some(ED25519_LOCKED_PASSPHRASE),
                true,
            )
            .expect("import");

        let deployment = runtime
            .private_key_deployment(&entry.id)
            .expect("deployment");
        let command = deployment.command();
        assert!(
            command.contains(ED25519_LOCKED_PUBLIC),
            "deployment command must use the persisted public key: {command}"
        );
        assert!(command.contains("authorized_keys"));
    }

    #[test]
    fn private_keys_page_remove_confirmation_round_trip() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime
            .import_private_key("Work", ED25519_PLAIN, None, false)
            .expect("import");
        runtime.open_private_keys_manager();
        runtime.select_private_key(0);

        let projection = runtime.request_private_key_remove();
        assert!(projection.private_keys_remove_confirm_visible);
        let projection = runtime.cancel_private_key_remove();
        assert!(!projection.private_keys_remove_confirm_visible);
        let projection = runtime.request_private_key_remove();
        assert!(projection.private_keys_remove_confirm_visible);
        let projection = runtime.confirm_private_key_remove();
        assert!(!projection.private_keys_remove_confirm_visible);
        assert!(runtime.private_key_entries().is_empty());
    }

    #[test]
    fn rename_private_key_persists_the_new_label() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        let entry = runtime
            .import_private_key("Old", ED25519_PLAIN, None, false)
            .expect("import");

        let renamed = runtime
            .rename_private_key(&entry.id, " New label ")
            .expect("rename");
        assert_eq!(renamed.label, "New label");
        assert!(runtime.rename_private_key(&entry.id, "  ").is_err());
        assert_eq!(
            ConfigStore::new(runtime.config_dir.clone())
                .load_or_recover()
                .expect("reload")
                .document
                .find_key(&entry.id)
                .expect("key")
                .label,
            "New label"
        );
    }
}
