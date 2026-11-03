//! Authentication state: known-hosts manager, host-key policy helpers and the
//! managed private-key inventory (`keys` in the C0 schema).
//!
//! N4 Phase 1 lives here: private-key material parsing/public-key derivation
//! (D0c `ssh-key 0.6.7` usage), the secret-store side of the key inventory, and
//! the `authorized_keys` deployment helper (command assembly + idempotency
//! check; execution goes through the A0 `SshClient::exec` boundary).

use std::collections::BTreeMap;

use crate::{error::AppError, error::AppResult};
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};
use yshell_config::HostKeyPolicy as ConfigHostKeyPolicy;
use yshell_config::KeyProfile;
use yshell_secret::SecretRef;
use yshell_ssh::{ExecOutput, HostKeyPolicy, KnownHosts, SshAdapter, SshClient, SshResult};

use super::*;

impl AppRuntime {
    pub fn open_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = true;
        self.ensure_known_hosts_selection();
        let count = self.known_host_entries().len();
        self.set_status_kind(
            "known-hosts-opened",
            format!(
                "Known hosts manager opened with {count} persisted entr{}.",
                if count == 1 { "y" } else { "ies" }
            ),
            count.to_string(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_known_hosts_manager(&mut self) -> AppProjection {
        self.known_hosts_modal_visible = false;
        self.set_status_kind(
            "known-hosts-closed",
            "Closed known hosts manager.".to_owned(),
            String::new(),
            String::new(),
        );
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
        self.set_known_hosts_selection_status();
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
        self.set_known_hosts_selection_status();
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
        let host_text = format!("{}:{}", selected.host, selected.port);
        let path_text = self.config_store.known_hosts_file().display().to_string();
        self.set_status_kind(
            "known-hosts-removed",
            format!("Removed known host entry for {host_text} from `{path_text}`."),
            host_text,
            path_text,
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
        let path_text = self.config_store.known_hosts_file().display().to_string();
        self.set_status_kind(
            "known-hosts-cleared",
            format!("Cleared all persisted known hosts from `{path_text}`."),
            path_text,
            String::new(),
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

    /// I18N-1：把已知主机选择摘要投影成状态栏 kind。
    pub(crate) fn set_known_hosts_selection_status(&mut self) {
        let text = self.known_hosts_selection_legacy_text();
        match self.known_hosts_selection_kind() {
            "selected" => {
                let host_text = format!(
                    "{}:{}",
                    self.known_hosts_selection_host_text(),
                    self.known_hosts_selection_port_text()
                );
                self.set_status_kind("known-hosts-selected", text, host_text, String::new());
            }
            "empty" => {
                self.set_status_kind("known-hosts-empty", text, String::new(), String::new());
            }
            _ => {
                let total = self.known_hosts_selection_total().to_string();
                self.set_status_kind("known-hosts-loaded", text, total, String::new());
            }
        }
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
                entry.host,
                entry.port,
                entry.algorithm,
                format_fingerprint_groups(&entry.fingerprint)
            ),
            None => format!(
                "Known hosts path: {}\nTemporary trust entries are not persisted here.",
                self.config_store.known_hosts_file().display()
            ),
        }
    }

    /// D17：已知主机详情拆分字段（不含英文标签；指纹沿用 D9 的分组格式）。
    #[must_use]
    pub(crate) fn known_hosts_detail_host_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.host)
            .unwrap_or_default()
    }

    /// D17：已知主机详情端口（无选中时为空串）。
    #[must_use]
    pub(crate) fn known_hosts_detail_port_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.port.to_string())
            .unwrap_or_default()
    }

    /// D17：已知主机详情算法（无选中时为空串）。
    #[must_use]
    pub(crate) fn known_hosts_detail_algorithm_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| entry.algorithm)
            .unwrap_or_default()
    }

    /// D17：已知主机详情指纹（D9 分组格式；无选中时为空串）。
    #[must_use]
    pub(crate) fn known_hosts_detail_fingerprint_text(&self) -> String {
        self.selected_known_host_entry()
            .map(|entry| format_fingerprint_groups(&entry.fingerprint))
            .unwrap_or_default()
    }

    /// D17：已知主机文件路径（始终可用）。
    #[must_use]
    pub(crate) fn known_hosts_detail_path_text(&self) -> String {
        self.config_store.known_hosts_file().display().to_string()
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
        let base_label = private_key_label(label, &parsed.comment, &key_id);
        let unique_label = self.allocate_unique_key_label(&base_label);
        let profile = KeyProfile {
            id: key_id.clone(),
            label: unique_label,
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
        self.key_catalog
            .save_key(profile)
            .map_err(AppError::from_error)?;
        self.config_document.keys = self.key_catalog.to_vec();
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
        self.key_catalog
            .delete_key(key_id)
            .map_err(AppError::from_error)?;
        self.config_document.keys = self.key_catalog.to_vec();
        // SSH-1：正在编辑的会话草稿若引用了这个 key，清掉选择，避免保存出悬空
        // key_id（密钥管理器页面不把未保存草稿计入 used_by）。
        if self.editor.key_id.as_deref() == Some(key_id) {
            self.editor.key_id = None;
        }
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
        self.key_catalog
            .rename_key(key_id, label)
            .map_err(AppError::from_error)?;
        self.config_document.keys = self.key_catalog.to_vec();
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

    /// Allocates a label that does not collide with existing key files (case-insensitive).
    fn allocate_unique_key_label(&self, base: &str) -> String {
        if !self.key_catalog.label_in_use(base, None) {
            return base.to_owned();
        }
        let mut ordinal = 2usize;
        loop {
            let candidate = format!("{base} {ordinal}");
            if !self.key_catalog.label_in_use(&candidate, None) {
                return candidate;
            }
            ordinal += 1;
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
///
/// The result is always a filesystem-safe unique-ish display name (key files
/// are `keys/<label>.toml`).
fn private_key_label(label: &str, comment: &str, key_id: &str) -> String {
    let candidates = [label.trim(), comment.trim(), key_id];
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Ok(name) = yshell_config::validate_entry_name(candidate) {
            return name;
        }
        let sanitized: String = candidate
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ' {
                    ch
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if let Ok(name) = yshell_config::validate_entry_name(&sanitized) {
            return name;
        }
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
        self.reset_private_keys_dialog_state();
        self.ensure_private_keys_selection();
        let count = self.config_document.keys.len();
        self.set_status_kind(
            "private-keys-opened",
            format!(
                "Private keys manager opened with {count} managed key{}.",
                if count == 1 { "" } else { "s" }
            ),
            count.to_string(),
            String::new(),
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
        self.reset_private_keys_dialog_state();
        self.set_status_kind(
            "private-keys-closed",
            "Closed private keys manager.".to_owned(),
            String::new(),
            String::new(),
        );
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
        self.set_status_kind(
            "private-key-selected",
            format!("Selected managed key `{}`.", row.label),
            row.label.clone(),
            String::new(),
        );
        self.projection()
    }

    /// 从导入表单的路径读取 OpenSSH 私钥并导入（rfd 或手动输入路径都走这里）。
    pub fn import_private_key_from_form(&mut self) -> AppProjection {
        let path_text = self.private_keys_import_path.trim().to_owned();
        if path_text.is_empty() {
            self.set_private_keys_status_kind(
                "private-key-import-path-empty",
                "Enter or browse a private key file path first.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        let material = match fs::read_to_string(&path_text) {
            Ok(material) => material,
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-import-read-failed",
                    format!("Failed to read `{path_text}`: {error}"),
                    path_text,
                    error.to_string(),
                );
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
                let detail = format!("{} · {}", entry.algorithm, entry.fingerprint);
                let text = format!("Imported `{}` ({detail}).", entry.label);
                self.set_status_kind(
                    "private-key-imported",
                    text.clone(),
                    entry.label.clone(),
                    detail.clone(),
                );
                self.set_private_keys_status_kind(
                    "private-key-imported",
                    text,
                    entry.label,
                    detail,
                );
                self.private_keys_import_label.clear();
                self.private_keys_import_path.clear();
                self.private_keys_import_passphrase.clear();
                self.private_keys_remember_import_passphrase = false;
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-import-failed",
                    format!("Import failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    /// "测试"选中私钥：重新解析存储材料（口令输入非空时作为覆盖）。
    pub fn test_selected_private_key(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.set_private_keys_status_kind(
                "private-key-test-requires-selection",
                "Select a managed key first.".to_owned(),
                String::new(),
                String::new(),
            );
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
                self.set_private_keys_status_kind(
                    "private-key-test-passed",
                    format!("Key test passed for `{}`.", entry.label),
                    entry.label,
                    String::new(),
                );
            }
            Err(error) => {
                self.private_keys_test_result_text = String::new();
                self.set_private_keys_status_kind(
                    "private-key-test-failed",
                    format!("Key test failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
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
    /// I18N-1：写私钥页内联状态（kind + 参数 + 英文回退），不触碰状态栏。
    pub(crate) fn set_private_keys_status_kind(
        &mut self,
        kind: &str,
        text: String,
        param_1: String,
        param_2: String,
    ) {
        self.private_keys_status_text = text;
        self.private_keys_status_kind = kind.to_owned();
        self.private_keys_status_param_1 = param_1;
        self.private_keys_status_param_2 = param_2;
    }

    /// 页面反馈同时镜像到状态栏（复制/导出等由宿主触发的动作）。
    pub fn set_private_keys_status_feedback(
        &mut self,
        kind: &str,
        text: String,
        param_1: String,
        param_2: String,
    ) -> AppProjection {
        if !text.is_empty() {
            self.set_status_kind(kind, text.clone(), param_1.clone(), param_2.clone());
        }
        self.set_private_keys_status_kind(kind, text, param_1, param_2);
        self.projection()
    }

    pub fn request_private_key_remove(&mut self) -> AppProjection {
        if let Some(entry) = self.selected_private_key_entry() {
            self.private_keys_remove_confirm_visible = true;
            let text = format!("Confirm removal of `{}` in the Danger Zone.", entry.label);
            self.set_status_kind(
                "private-key-remove-confirm",
                text.clone(),
                entry.label.clone(),
                String::new(),
            );
            self.set_private_keys_status_kind(
                "private-key-remove-confirm",
                text,
                entry.label,
                String::new(),
            );
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
                let text = format!("Removed managed key `{}`.", removed.label);
                self.set_private_keys_status_kind(
                    "private-key-removed",
                    text.clone(),
                    removed.label.clone(),
                    String::new(),
                );
                self.ensure_private_keys_selection();
                self.set_status_kind("private-key-removed", text, removed.label, String::new());
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-remove-failed",
                    format!("Remove failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    pub fn request_private_key_deploy(&mut self) -> AppProjection {
        if self.selected_private_key_entry().is_some() {
            self.private_keys_deploy_confirm_visible = true;
            self.keys_deploy_target_index = 0;
            let targets = self.private_key_deploy_targets();
            match targets.first() {
                Some((_, target)) => {
                    let text = format!("Ready to deploy to {target}.");
                    self.set_private_keys_status_kind(
                        "private-key-deploy-ready",
                        text,
                        target.clone(),
                        String::new(),
                    );
                }
                None => {
                    self.set_private_keys_status_kind(
                        "private-key-deploy-no-targets",
                        "No live SSH session is available to deploy to. Open an SSH session first."
                            .to_owned(),
                        String::new(),
                        String::new(),
                    );
                }
            }
        }
        self.projection()
    }

    /// Deploy 子窗可选的目标会话（session key, `user@host:port`；本地终端不参与）。
    #[must_use]
    pub(crate) fn private_key_deploy_targets(&self) -> Vec<(String, String)> {
        let mut targets = self
            .sessions
            .iter()
            .filter(|(_, session)| !session.is_local())
            .map(|(session_key, session)| {
                (
                    session_key.clone(),
                    format!(
                        "{}@{}:{}",
                        session.username_label(),
                        session.host_label(),
                        session.ssh_config.port
                    ),
                )
            })
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| left.1.cmp(&right.1));
        targets
    }

    /// Deploy 子窗切换目标会话（越界忽略）。
    // TODO(wave2-private-key-deploy): Private Key Deploy 子窗接线后删除。
    #[allow(dead_code)]
    pub fn select_private_key_deploy_target(&mut self, index: i32) -> AppProjection {
        let targets = self.private_key_deploy_targets();
        let Some(target) = usize::try_from(index)
            .ok()
            .and_then(|index| targets.get(index))
        else {
            return self.projection();
        };
        self.keys_deploy_target_index = index;
        let text = format!("Deploy target set to {}.", target.1);
        let target_text = target.1.clone();
        self.set_private_keys_status_kind(
            "private-key-deploy-target",
            text,
            target_text,
            String::new(),
        );
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
                self.set_private_keys_status_kind(
                    "private-key-deploy-failed",
                    format!("Deploy failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
                return self.projection();
            }
        };
        // Deploy 子窗：按目标会话选择（本地终端不参与；无目标时给出明确提示）。
        let targets = self.private_key_deploy_targets();
        let selected_index = usize::try_from(self.keys_deploy_target_index).unwrap_or(0);
        let Some((session_key, _)) = targets
            .get(selected_index)
            .or_else(|| targets.first())
            .cloned()
        else {
            self.private_keys_deploy_confirm_visible = false;
            self.set_private_keys_status_kind(
                "private-key-deploy-no-targets",
                "No live SSH session is available to deploy to. Open an SSH session first."
                    .to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        let Some(session) = self.sessions.get(&session_key) else {
            self.private_keys_deploy_confirm_visible = false;
            self.set_private_keys_status_kind(
                "private-key-deploy-session-gone",
                "The deploy target session is gone.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        // TAB-4：本地终端没有 SSH 目标，密钥部署绝不能拿占位配置去连网。
        if session.is_local() {
            self.private_keys_deploy_confirm_visible = false;
            self.set_private_keys_status_kind(
                "private-key-deploy-requires-ssh",
                "Deploy requires an SSH session; the selected target is a local terminal."
                    .to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
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
                let text = format!(
                    "Deployed `{}` to {target} (idempotent append, exit status 0).",
                    entry.label
                );
                self.set_status_kind(
                    "private-key-deployed",
                    text.clone(),
                    entry.label.clone(),
                    target.clone(),
                );
                self.set_private_keys_status_kind(
                    "private-key-deployed",
                    text,
                    entry.label,
                    target,
                );
            }
            Ok(output) => {
                self.private_keys_deploy_confirm_visible = false;
                let exit_status = output.exit_status.to_string();
                let text =
                    format!("Deploy command failed on {target} with exit status {exit_status}.");
                self.set_private_keys_status_kind(
                    "private-key-deploy-exit-failed",
                    text,
                    target,
                    exit_status,
                );
            }
            Err(error) => {
                self.private_keys_deploy_confirm_visible = false;
                self.set_private_keys_status_kind(
                    "private-key-deploy-target-failed",
                    format!("Deploy failed on {target}: {error}"),
                    target,
                    error.to_string(),
                );
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
        let client = SshClient::with_real_backend();
        let mut session = client.connect(&effective)?;
        let output = deploy_authorized_keys_with_client(&client, &mut session, deployment)?;
        let _ = client.disconnect(session);
        Ok(output)
    }

    pub fn save_selected_private_key_passphrase(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            return self.projection();
        };
        let passphrase = self.private_keys_passphrase_input.clone();
        match self.store_private_key_passphrase(&entry.id, &passphrase) {
            Ok(updated) => {
                self.private_keys_passphrase_input.clear();
                let text = format!(
                    "Passphrase saved for `{}` (validated against the stored key).",
                    updated.label
                );
                self.set_status_kind(
                    "private-key-passphrase-saved",
                    text.clone(),
                    updated.label.clone(),
                    String::new(),
                );
                self.set_private_keys_status_kind(
                    "private-key-passphrase-saved",
                    text,
                    updated.label,
                    String::new(),
                );
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-passphrase-save-failed",
                    format!("Cannot save passphrase: {error}"),
                    error.to_string(),
                    String::new(),
                );
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
                let text = format!("Forgot the saved passphrase for `{}`.", updated.label);
                self.set_status_kind(
                    "private-key-passphrase-forgotten",
                    text.clone(),
                    updated.label.clone(),
                    String::new(),
                );
                self.set_private_keys_status_kind(
                    "private-key-passphrase-forgotten",
                    text,
                    updated.label,
                    String::new(),
                );
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-passphrase-forget-failed",
                    format!("Cannot forget passphrase: {error}"),
                    error.to_string(),
                    String::new(),
                );
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
                let text = format!("Renamed managed key to `{}`.", updated.label);
                self.set_status_kind(
                    "private-key-renamed",
                    text.clone(),
                    updated.label.clone(),
                    String::new(),
                );
                self.set_private_keys_status_kind(
                    "private-key-renamed",
                    text,
                    updated.label,
                    String::new(),
                );
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-rename-failed",
                    format!("Rename failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
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
        self.reset_host_keys_dialog_state();
        self.ensure_host_keys_selection();
        let groups = self.host_key_group_data().len();
        self.set_status_kind(
            "host-keys-opened",
            format!("Host keys manager opened with {groups} persisted host key group(s)."),
            groups.to_string(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_host_keys_manager(&mut self) -> AppProjection {
        self.host_keys_modal_visible = false;
        self.host_keys_import_visible = false;
        self.host_keys_import_text.clear();
        self.reset_host_keys_dialog_state();
        self.set_status_kind(
            "host-keys-closed",
            "Closed host keys manager.".to_owned(),
            String::new(),
            String::new(),
        );
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
        let host_text = format!("{}:{}", selected.host, selected.port);
        let algorithm = selected.entries[entry_index].algorithm.clone();
        self.set_status_kind(
            "host-key-selected",
            format!("Selected host key {host_text} ({algorithm})."),
            host_text,
            algorithm,
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

    /// 从文件导入主机密钥（rfd 选路后读文本，再走 `import_host_keys_from_text`）。
    pub fn import_host_keys_from_path(&mut self, path: &str) -> AppProjection {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.host_keys_import_text = text;
                self.host_keys_import_visible = false;
                let _ = self.import_host_keys_from_text();
                self.host_keys_import_text.clear();
                self.projection()
            }
            Err(error) => self.set_host_keys_status_feedback(
                "host-keys-import-read-failed",
                format!("Import failed to read `{path}`: {error}"),
                path.to_owned(),
                error.to_string(),
            ),
        }
    }

    /// 解析 `host:port <公钥行 | SHA256:指纹>` 行并写入 persistent known hosts。
    ///
    /// 返回 `(写入条数, 跳过行及原因)`；不落盘、不写状态（调用方负责）。
    fn apply_host_key_lines(&mut self, text: &str) -> (usize, Vec<String>) {
        let mut imported = 0usize;
        let mut failures = Vec::new();
        for line in text.lines() {
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
        (imported, failures)
    }

    /// 导入主机密钥：每行 `host:port <公钥行>` 或 `host:port <SHA256:指纹>`。
    pub fn import_host_keys_from_text(&mut self) -> AppProjection {
        let (imported, failures) = self.apply_host_key_lines(&self.host_keys_import_text.clone());
        if imported > 0 {
            if let Err(error) = self
                .config_store
                .save_known_hosts(&self.persistent_known_hosts)
            {
                self.set_host_keys_status_kind(
                    "host-keys-import-persist-failed",
                    format!("Import failed to persist: {error}"),
                    error.to_string(),
                    String::new(),
                );
                return self.projection();
            }
            self.ensure_host_keys_selection();
        }
        if failures.is_empty() {
            let status_text = format!("Imported {imported} host key(s).");
            let imported_text = imported.to_string();
            self.set_host_keys_status_kind(
                "host-keys-imported",
                status_text.clone(),
                imported_text.clone(),
                String::new(),
            );
            self.set_status_kind(
                "host-keys-imported",
                status_text,
                imported_text,
                String::new(),
            );
        } else {
            tracing::warn!(
                target: "yshell::runtime",
                "host key import skipped lines: {}",
                failures.join("; ")
            );
            let skipped = failures.len();
            let failures_text = failures.join("; ");
            let status_text = format!(
                "Imported {imported} host key(s); {skipped} line(s) skipped: {failures_text}"
            );
            self.set_host_keys_status_kind(
                "host-keys-imported-with-skips",
                status_text.clone(),
                imported.to_string(),
                failures_text,
            );
            self.set_status_kind(
                "host-keys-imported",
                status_text,
                imported.to_string(),
                skipped.to_string(),
            );
        }
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

    /// I18N-1：写主机密钥页内联状态（kind + 参数 + 英文回退），不触碰状态栏。
    pub(crate) fn set_host_keys_status_kind(
        &mut self,
        kind: &str,
        text: String,
        param_1: String,
        param_2: String,
    ) {
        self.host_keys_status_text = text;
        self.host_keys_status_kind = kind.to_owned();
        self.host_keys_status_param_1 = param_1;
        self.host_keys_status_param_2 = param_2;
    }

    /// 页面反馈同时镜像到状态栏（导出等由宿主触发的动作）。
    pub fn set_host_keys_status_feedback(
        &mut self,
        kind: &str,
        text: String,
        param_1: String,
        param_2: String,
    ) -> AppProjection {
        if !text.is_empty() {
            self.set_status_kind(kind, text.clone(), param_1.clone(), param_2.clone());
        }
        self.set_host_keys_status_kind(kind, text, param_1, param_2);
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
        let host_text = format!("{}:{}", selected.host, selected.port);
        let status_text = format!("Removed the saved host key for {host_text}.");
        self.set_host_keys_status_kind(
            "host-key-removed",
            status_text.clone(),
            host_text.clone(),
            String::new(),
        );
        self.set_status_kind("host-key-removed", status_text, host_text, String::new());
        Ok(self.projection())
    }

    pub fn clear_all_host_keys(&mut self) -> AppResult<AppProjection> {
        self.persistent_known_hosts = KnownHosts::new();
        self.config_store
            .save_known_hosts(&self.persistent_known_hosts)
            .map_err(AppError::from_error)?;
        self.host_keys_selected_group = None;
        self.host_keys_selected_entry = None;
        let status_text = "Cleared all saved host keys.".to_owned();
        self.set_host_keys_status_kind(
            "host-keys-cleared",
            status_text.clone(),
            String::new(),
            String::new(),
        );
        self.set_status_kind(
            "host-keys-cleared",
            status_text,
            String::new(),
            String::new(),
        );
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
                entry.host,
                entry.port,
                entry.algorithm,
                format_fingerprint_groups(&entry.fingerprint)
            ),
            None => format!(
                "Host keys path: {}\nTemporary trust entries are not persisted here.",
                self.config_store.known_hosts_file().display()
            ),
        }
    }

    /// D20：主机密钥页详情拆分字段（不含英文标签；指纹沿用 D9 的分组格式）。
    #[must_use]
    pub(crate) fn host_keys_detail_host_text(&self) -> String {
        self.selected_host_key_entry()
            .map(|entry| entry.host)
            .unwrap_or_default()
    }

    /// D20：主机密钥页详情算法（无选中时为空串）。
    #[must_use]
    pub(crate) fn host_keys_detail_algorithm_text(&self) -> String {
        self.selected_host_key_entry()
            .map(|entry| entry.algorithm)
            .unwrap_or_default()
    }

    /// D20：主机密钥页详情指纹（D9 分组格式；无选中时为空串）。
    #[must_use]
    pub(crate) fn host_keys_detail_fingerprint_text(&self) -> String {
        self.selected_host_key_entry()
            .map(|entry| format_fingerprint_groups(&entry.fingerprint))
            .unwrap_or_default()
    }

    /// D20：主机密钥文件路径（始终可用）。
    #[must_use]
    pub(crate) fn host_keys_detail_path_text(&self) -> String {
        self.config_store.known_hosts_file().display().to_string()
    }
}

// ---------------------------------------------------------------------------
// --- managers-dialogs ---（波2-C：管理器子窗与行级动作）
// ---------------------------------------------------------------------------

/// Private Keys → Generate 的算法选项（数量与 Slint ComboBox 的顺序一致）。
///
/// 生成由 `ssh-key` 的 `PrivateKey::random` 完成：ED25519 与 RSA（4096）在
/// 当前 feature 集下可用；ECDSA 曲线 feature 未启用，故不提供。
pub(crate) const PRIVATE_KEY_GENERATE_ALGORITHMS: [&str; 2] = ["ED25519", "RSA"];

// TODO(wave2-managers): Known Hosts / Private Keys 的 Add·Import·Export·右键菜单
// 子窗接线后删除本行 allow（当前这些窗口尚未接线）。
#[allow(dead_code)]
impl AppRuntime {
    /// 清空私钥管理器的子窗/菜单暂态（打开/关闭管理器时调用）。
    fn reset_private_keys_dialog_state(&mut self) {
        self.keys_generate_visible = false;
        self.keys_generate_result_visible = false;
        self.keys_generate_algorithm_index = 0;
        self.keys_generate_label.clear();
        self.keys_generate_comment.clear();
        self.keys_generate_passphrase.clear();
        self.keys_generate_confirm.clear();
        self.keys_generate_error_text.clear();
        self.keys_generate_public_key.clear();
        self.keys_generate_fingerprint.clear();
        self.keys_generate_result_label.clear();
        self.keys_import_visible = false;
        self.keys_import_tab = 0;
        self.keys_import_paste_text.clear();
        self.keys_export_visible = false;
        self.keys_export_path_text.clear();
        self.keys_deploy_target_index = 0;
        self.keys_menu_key_id = None;
    }

    /// 清空主机密钥管理器的子窗/菜单暂态。
    fn reset_host_keys_dialog_state(&mut self) {
        self.hosts_add_visible = false;
        self.hosts_add_text.clear();
        self.hosts_add_error_text.clear();
        self.hosts_add_edit_target = None;
        self.hosts_import_path_text.clear();
        self.hosts_export_visible = false;
        self.hosts_export_path_text.clear();
        self.hosts_menu_group = None;
        self.hosts_menu_entry = None;
    }

    // --- Known Hosts：Add / Edit 单字段窗 ---------------------------------

    pub fn open_host_keys_add_dialog(&mut self) -> AppProjection {
        self.hosts_add_visible = true;
        self.hosts_add_text.clear();
        self.hosts_add_error_text.clear();
        self.hosts_add_edit_target = None;
        self.set_host_keys_status_kind(
            "host-keys-add-opened",
            "Paste a `host:port` entry with an OpenSSH public key or a SHA256 fingerprint."
                .to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    /// 行右键 Edit：用现有条目预填 Add 单字段窗（保存时替换旧条目）。
    pub fn open_host_keys_edit_dialog(&mut self, group: i32, entry: i32) -> AppProjection {
        let groups = self.host_key_group_data();
        let Some(group_index) = usize::try_from(group)
            .ok()
            .filter(|index| *index < groups.len())
        else {
            return self.projection();
        };
        let Some(entry_index) = usize::try_from(entry)
            .ok()
            .filter(|index| *index < groups[group_index].entries.len())
        else {
            return self.projection();
        };
        let selected = &groups[group_index];
        let key = &selected.entries[entry_index];
        let target = format!("{}:{}", selected.host, selected.port);
        self.hosts_add_visible = true;
        self.hosts_add_edit_target = Some(target.clone());
        self.hosts_add_text = format!(
            "{target} {} {}",
            key.algorithm,
            format_fingerprint_groups(&key.fingerprint)
        );
        self.hosts_add_error_text.clear();
        self.host_keys_selected_group = Some(group_index);
        self.host_keys_selected_entry = Some(entry_index);
        let text = format!("Editing the saved host key for {target}.");
        self.set_host_keys_status_kind("host-keys-edit-opened", text, target, String::new());
        self.projection()
    }

    pub fn update_host_keys_add_text(&mut self, value: &str) -> AppProjection {
        self.hosts_add_text = value.to_owned();
        self.projection()
    }

    pub fn cancel_host_keys_add_dialog(&mut self) -> AppProjection {
        self.hosts_add_visible = false;
        self.hosts_add_text.clear();
        self.hosts_add_error_text.clear();
        self.hosts_add_edit_target = None;
        self.projection()
    }

    /// Add/Edit 提交：解析粘贴行 → pin；Edit 时若 host:port 变化则删掉旧条目。
    pub fn submit_host_keys_add_dialog(&mut self) -> AppProjection {
        let text = self.hosts_add_text.trim().to_owned();
        if text.is_empty() {
            self.hosts_add_error_text = "Paste a host key entry first.".to_owned();
            self.set_host_keys_status_kind(
                "host-keys-add-empty",
                self.hosts_add_error_text.clone(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        let requested_target = Self::first_host_key_target(&text);
        let (imported, failures) = self.apply_host_key_lines(&text);
        if imported == 0 {
            let reason = if failures.is_empty() {
                "the entry could not be parsed".to_owned()
            } else {
                failures.join("; ")
            };
            self.hosts_add_error_text = reason.clone();
            self.set_host_keys_status_kind(
                "host-keys-add-failed",
                reason.clone(),
                reason,
                String::new(),
            );
            return self.projection();
        }
        let mut removed_old = false;
        let was_edit = self.hosts_add_edit_target.is_some();
        if let Some(old_target) = self.hosts_add_edit_target.take() {
            if requested_target.as_deref() != Some(old_target.as_str()) {
                if let Some((host, port_text)) = old_target.rsplit_once(':') {
                    if let Ok(port) = port_text.parse::<u16>() {
                        self.persistent_known_hosts.remove(host, port);
                        removed_old = true;
                    }
                }
            }
        }
        if let Err(error) = self
            .config_store
            .save_known_hosts(&self.persistent_known_hosts)
        {
            self.hosts_add_error_text = error.to_string();
            self.set_host_keys_status_kind(
                "host-keys-add-persist-failed",
                format!("Could not save the host key: {error}"),
                error.to_string(),
                String::new(),
            );
            return self.projection();
        }
        self.ensure_host_keys_selection();
        let target_text = requested_target.unwrap_or_else(|| "the pasted entry".to_owned());
        let kind = if was_edit || removed_old {
            "host-keys-edited"
        } else {
            "host-keys-added"
        };
        self.hosts_add_visible = false;
        self.hosts_add_text.clear();
        self.hosts_add_error_text.clear();
        let text = if kind == "host-keys-edited" {
            format!("Updated the saved host key for {target_text}.")
        } else {
            format!("Added the saved host key for {target_text}.")
        };
        self.set_host_keys_status_kind(kind, text.clone(), target_text.clone(), String::new());
        self.set_status_kind(kind, text, target_text, String::new());
        self.projection()
    }

    /// 取首行里的 `host:port`（用于状态文案/Edit 替换判断）。
    fn first_host_key_target(text: &str) -> Option<String> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))?;
        let target = line.split_whitespace().next()?;
        let (host, port) = target.rsplit_once(':')?;
        port.parse::<u16>().ok()?;
        Some(format!("{host}:{port}"))
    }

    // --- Known Hosts：Import / Export 子窗 --------------------------------

    pub fn open_host_keys_import_dialog(&mut self) -> AppProjection {
        self.host_keys_import_visible = true;
        self.hosts_import_path_text.clear();
        self.set_host_keys_status_kind(
            "host-keys-import-opened",
            "Choose a `known_hosts` file to import.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_host_keys_import_dialog(&mut self) -> AppProjection {
        self.host_keys_import_visible = false;
        self.hosts_import_path_text.clear();
        self.projection()
    }

    pub fn update_host_keys_import_path(&mut self, value: &str) -> AppProjection {
        self.hosts_import_path_text = value.to_owned();
        self.projection()
    }

    /// Import 子窗提交：读文件 → 解析 → 落盘；成功时关闭子窗。
    pub fn import_host_keys_from_dialog(&mut self) -> AppProjection {
        let path = self.hosts_import_path_text.trim().to_owned();
        if path.is_empty() {
            self.set_host_keys_status_kind(
                "host-keys-import-path-empty",
                "Choose a `known_hosts` file to import.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.set_host_keys_status_kind(
                    "host-keys-import-read-failed",
                    format!("Import failed to read `{path}`: {error}"),
                    path,
                    error.to_string(),
                );
                return self.projection();
            }
        };
        let (imported, failures) = self.apply_host_key_lines(&text);
        if imported > 0 {
            if let Err(error) = self
                .config_store
                .save_known_hosts(&self.persistent_known_hosts)
            {
                self.set_host_keys_status_kind(
                    "host-keys-import-persist-failed",
                    format!("Import failed to persist: {error}"),
                    error.to_string(),
                    String::new(),
                );
                return self.projection();
            }
            self.ensure_host_keys_selection();
        }
        if imported == 0 {
            let reason = if failures.is_empty() {
                "the file contains no host key entries".to_owned()
            } else {
                failures.join("; ")
            };
            self.set_host_keys_status_kind(
                "host-keys-import-failed",
                format!("Import failed: {reason}"),
                reason,
                String::new(),
            );
            return self.projection();
        }
        self.host_keys_import_visible = false;
        self.hosts_import_path_text.clear();
        let skipped = failures.len();
        if skipped == 0 {
            let text = format!("Imported {imported} host key(s).");
            self.set_host_keys_status_kind(
                "host-keys-imported",
                text.clone(),
                imported.to_string(),
                String::new(),
            );
            self.set_status_kind(
                "host-keys-imported",
                text,
                imported.to_string(),
                String::new(),
            );
        } else {
            let failures_text = failures.join("; ");
            let text = format!(
                "Imported {imported} host key(s); {skipped} line(s) skipped: {failures_text}"
            );
            self.set_host_keys_status_kind(
                "host-keys-imported-with-skips",
                text.clone(),
                imported.to_string(),
                failures_text,
            );
            self.set_status_kind(
                "host-keys-imported",
                text,
                imported.to_string(),
                skipped.to_string(),
            );
        }
        self.projection()
    }

    pub fn open_host_keys_export_dialog(&mut self) -> AppProjection {
        self.hosts_export_visible = true;
        self.hosts_export_path_text = self
            .config_dir
            .join("known_hosts_export")
            .display()
            .to_string();
        self.projection()
    }

    pub fn close_host_keys_export_dialog(&mut self) -> AppProjection {
        self.hosts_export_visible = false;
        self.projection()
    }

    pub fn update_host_keys_export_path(&mut self, value: &str) -> AppProjection {
        self.hosts_export_path_text = value.to_owned();
        self.projection()
    }

    /// Export 子窗提交：把 `host:port algorithm fingerprint` 行写到目标路径。
    pub fn export_host_keys_to_path(&mut self) -> AppProjection {
        let path = self.hosts_export_path_text.trim().to_owned();
        if path.is_empty() {
            self.set_host_keys_status_kind(
                "host-keys-export-path-empty",
                "Choose a destination file first.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        let text = self.export_host_keys_text();
        match fs::write(&path, format!("{text}\n")) {
            Ok(()) => {
                self.hosts_export_visible = false;
                let message = format!("Exported host keys to `{path}`.");
                self.set_host_keys_status_kind(
                    "host-keys-exported",
                    message.clone(),
                    path.clone(),
                    String::new(),
                );
                self.set_status_kind("host-keys-exported", message, path, String::new());
            }
            Err(error) => {
                self.set_host_keys_status_kind(
                    "host-keys-export-failed",
                    format!("Export failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    // --- Known Hosts：行右键菜单 -----------------------------------------

    pub fn prepare_host_keys_context_menu(&mut self, group: i32, entry: i32) -> AppProjection {
        let groups = self.host_key_group_data();
        let Some(group_index) = usize::try_from(group)
            .ok()
            .filter(|index| *index < groups.len())
        else {
            return self.projection();
        };
        let Some(entry_index) = usize::try_from(entry)
            .ok()
            .filter(|index| *index < groups[group_index].entries.len())
        else {
            return self.projection();
        };
        self.host_keys_selected_group = Some(group_index);
        self.host_keys_selected_entry = Some(entry_index);
        self.hosts_menu_group = Some(group_index);
        self.hosts_menu_entry = Some(entry_index);
        let selected = &groups[group_index];
        let host_text = format!("{}:{}", selected.host, selected.port);
        let algorithm = selected.entries[entry_index].algorithm.clone();
        let text = format!("Selected host key {host_text} ({algorithm}).");
        self.set_host_keys_status_kind("host-key-selected", text, host_text, algorithm);
        self.projection()
    }

    /// 行右键 Edit（无菜单目标时退化为当前选中条目）。
    pub fn edit_host_key_from_menu(&mut self) -> AppProjection {
        let (group, entry) = self
            .hosts_menu_group
            .zip(self.hosts_menu_entry)
            .or_else(|| {
                self.host_keys_selected_group
                    .zip(self.host_keys_selected_entry)
            })
            .map_or((0, 0), |(group, entry)| (group as i32, entry as i32));
        self.open_host_keys_edit_dialog(group, entry)
    }

    /// 行右键 Remove（复用选中条目的删除逻辑，由宿主确认框承接）。
    pub fn remove_host_key_from_menu(&mut self) -> AppResult<AppProjection> {
        if let Some((group, entry)) = self.hosts_menu_group.zip(self.hosts_menu_entry) {
            self.host_keys_selected_group = Some(group);
            self.host_keys_selected_entry = Some(entry);
        }
        self.hosts_menu_group = None;
        self.hosts_menu_entry = None;
        self.remove_selected_host_key()
    }

    // --- Private Keys：Generate 子窗 --------------------------------------

    pub fn open_private_keys_generate_dialog(&mut self) -> AppProjection {
        self.keys_generate_visible = true;
        self.keys_generate_result_visible = false;
        self.keys_generate_algorithm_index = 0;
        self.keys_generate_label.clear();
        self.keys_generate_comment.clear();
        self.keys_generate_passphrase.clear();
        self.keys_generate_confirm.clear();
        self.keys_generate_error_text.clear();
        self.keys_generate_public_key.clear();
        self.keys_generate_fingerprint.clear();
        self.keys_generate_result_label.clear();
        self.set_private_keys_status_kind(
            "private-key-generate-opened",
            "Generate a new managed key: choose the algorithm, then confirm the passphrase."
                .to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_private_keys_generate_dialog(&mut self) -> AppProjection {
        self.keys_generate_visible = false;
        self.keys_generate_result_visible = false;
        // 敏感值：关闭时清空口令（与导入/密码输入同一约定）。
        self.keys_generate_passphrase.clear();
        self.keys_generate_confirm.clear();
        self.projection()
    }

    pub fn update_private_keys_generate_label(&mut self, value: &str) -> AppProjection {
        self.keys_generate_label = value.to_owned();
        self.projection()
    }

    pub fn update_private_keys_generate_comment(&mut self, value: &str) -> AppProjection {
        self.keys_generate_comment = value.to_owned();
        self.projection()
    }

    pub fn update_private_keys_generate_passphrase(&mut self, value: &str) -> AppProjection {
        self.keys_generate_passphrase = value.to_owned();
        self.projection()
    }

    pub fn update_private_keys_generate_confirm(&mut self, value: &str) -> AppProjection {
        self.keys_generate_confirm = value.to_owned();
        self.projection()
    }

    pub fn select_private_keys_generate_algorithm(&mut self, index: i32) -> AppProjection {
        if (0..PRIVATE_KEY_GENERATE_ALGORITHMS.len() as i32).contains(&index) {
            self.keys_generate_algorithm_index = index;
        }
        self.projection()
    }

    /// Generate 表单 → 生成材料 → 走现有导入流程 → 结果页（公钥 + Copy）。
    pub fn generate_private_key_from_form(&mut self) -> AppProjection {
        self.keys_generate_error_text.clear();
        if self.keys_generate_passphrase != self.keys_generate_confirm {
            self.keys_generate_error_text =
                "The passphrase confirmation does not match.".to_owned();
            self.set_private_keys_status_kind(
                "private-key-generate-passphrase-mismatch",
                self.keys_generate_error_text.clone(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        let algorithm_index = self.keys_generate_algorithm_index;
        let comment = self.keys_generate_comment.clone();
        let passphrase_text = self.keys_generate_passphrase.clone();
        let material = match Self::generate_private_key_material(
            algorithm_index,
            &comment,
            &passphrase_text,
        ) {
            Ok(material) => material,
            Err(error) => {
                self.keys_generate_error_text = format!("Key generation failed: {error}");
                self.set_private_keys_status_kind(
                    "private-key-generate-failed",
                    self.keys_generate_error_text.clone(),
                    error.to_string(),
                    String::new(),
                );
                return self.projection();
            }
        };
        let passphrase = self.keys_generate_passphrase.clone();
        let passphrase = if passphrase.is_empty() {
            None
        } else {
            Some(passphrase.as_str())
        };
        let label = self.keys_generate_label.clone();
        match self.import_private_key(&label, &material, passphrase, true) {
            Ok(entry) => {
                self.private_keys_selected_id = Some(entry.id.clone());
                self.keys_generate_public_key = entry.public_key.clone();
                self.keys_generate_fingerprint = entry.fingerprint.clone();
                self.keys_generate_result_label = entry.label.clone();
                self.keys_generate_result_visible = true;
                self.keys_generate_passphrase.clear();
                self.keys_generate_confirm.clear();
                let detail = format!("{} · {}", entry.algorithm, entry.fingerprint);
                let text = format!("Generated `{}` ({detail}).", entry.label);
                self.set_status_kind(
                    "private-key-generated",
                    text.clone(),
                    entry.label.clone(),
                    detail.clone(),
                );
                self.set_private_keys_status_kind(
                    "private-key-generated",
                    text,
                    entry.label,
                    detail,
                );
            }
            Err(error) => {
                self.keys_generate_error_text = format!("Key generation failed: {error}");
                self.set_private_keys_status_kind(
                    "private-key-generate-failed",
                    self.keys_generate_error_text.clone(),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    /// 结果页 Copy Public Key 用（生成成功后有效）。
    #[must_use]
    pub fn generated_private_key_public_line(&self) -> Option<String> {
        (!self.keys_generate_public_key.is_empty()).then(|| self.keys_generate_public_key.clone())
    }

    /// 结果页 Done：关闭 Generate 子窗（保留已生成密钥）。
    pub fn finish_private_keys_generate_dialog(&mut self) -> AppProjection {
        self.keys_generate_visible = false;
        self.keys_generate_result_visible = false;
        self.keys_generate_public_key.clear();
        self.projection()
    }

    /// 用 `ssh-key` 生成 OpenSSH 私钥文本（ED25519 / RSA 4096）。
    ///
    /// 口令非空时用 OpenSSH 默认的 `Aes256Ctr` 加密材料——与「加密托管」一致：
    /// secret store 里存的是加密后的 OpenSSH 容器，回读仍需口令。
    fn generate_private_key_material(
        algorithm_index: i32,
        comment: &str,
        passphrase: &str,
    ) -> AppResult<String> {
        let algorithm = match algorithm_index {
            0 => Algorithm::Ed25519,
            1 => Algorithm::Rsa { hash: None },
            other => {
                return Err(AppError::new(format!(
                    "unsupported key algorithm index {other}"
                )));
            }
        };
        let mut rng = ssh_key::rand_core::OsRng;
        let mut key = PrivateKey::random(&mut rng, algorithm)
            .map_err(|error| AppError::new(format!("key generation failed: {error}")))?;
        let comment = comment.trim();
        if !comment.is_empty() {
            key.set_comment(comment);
        }
        if !passphrase.is_empty() {
            key = key
                .encrypt(&mut rng, passphrase.as_bytes())
                .map_err(|error| AppError::new(format!("key encryption failed: {error}")))?;
        }
        let material = key
            .to_openssh(LineEnding::LF)
            .map_err(|error| AppError::new(format!("key generation failed: {error}")))?;
        Ok(material.to_string())
    }

    // --- Private Keys：Import（文件 / 粘贴）子窗 ---------------------------

    pub fn open_private_keys_import_dialog(&mut self) -> AppProjection {
        self.keys_import_visible = true;
        self.keys_import_tab = 0;
        self.keys_import_paste_text.clear();
        self.private_keys_import_label.clear();
        self.private_keys_import_path.clear();
        self.private_keys_import_passphrase.clear();
        self.set_private_keys_status_kind(
            "private-key-import-opened",
            "Import an OpenSSH private key from a file or paste its text.".to_owned(),
            String::new(),
            String::new(),
        );
        self.projection()
    }

    pub fn close_private_keys_import_dialog(&mut self) -> AppProjection {
        self.keys_import_visible = false;
        self.keys_import_paste_text.clear();
        self.private_keys_import_passphrase.clear();
        self.projection()
    }

    /// Import 子窗 tab：0 = 文件，1 = 粘贴（两个 tab 可互跳）。
    pub fn select_private_keys_import_tab(&mut self, tab: i32) -> AppProjection {
        self.keys_import_tab = if tab == 1 { 1 } else { 0 };
        self.projection()
    }

    pub fn update_private_keys_import_paste_text(&mut self, value: &str) -> AppProjection {
        self.keys_import_paste_text = value.to_owned();
        self.projection()
    }

    pub fn import_private_key_from_dialog(&mut self) -> AppProjection {
        let material = if self.keys_import_tab == 1 {
            let text = self.keys_import_paste_text.trim().to_owned();
            if text.is_empty() {
                self.set_private_keys_status_kind(
                    "private-key-import-text-empty",
                    "Paste an OpenSSH private key first.".to_owned(),
                    String::new(),
                    String::new(),
                );
                return self.projection();
            }
            text
        } else {
            let path = self.private_keys_import_path.trim().to_owned();
            if path.is_empty() {
                self.set_private_keys_status_kind(
                    "private-key-import-path-empty",
                    "Enter or browse a private key file path first.".to_owned(),
                    String::new(),
                    String::new(),
                );
                return self.projection();
            }
            match fs::read_to_string(&path) {
                Ok(material) => material,
                Err(error) => {
                    self.set_private_keys_status_kind(
                        "private-key-import-read-failed",
                        format!("Failed to read `{path}`: {error}"),
                        path,
                        error.to_string(),
                    );
                    return self.projection();
                }
            }
        };
        let label = self.private_keys_import_label.clone();
        let passphrase = self.private_keys_import_passphrase.clone();
        let passphrase = if passphrase.is_empty() {
            None
        } else {
            Some(passphrase.as_str())
        };
        match self.import_private_key(&label, &material, passphrase, true) {
            Ok(entry) => {
                self.private_keys_selected_id = Some(entry.id.clone());
                self.keys_import_visible = false;
                self.keys_import_paste_text.clear();
                self.private_keys_import_label.clear();
                self.private_keys_import_path.clear();
                self.private_keys_import_passphrase.clear();
                let detail = format!("{} · {}", entry.algorithm, entry.fingerprint);
                let text = format!("Imported `{}` ({detail}).", entry.label);
                self.set_status_kind(
                    "private-key-imported",
                    text.clone(),
                    entry.label.clone(),
                    detail.clone(),
                );
                self.set_private_keys_status_kind(
                    "private-key-imported",
                    text,
                    entry.label,
                    detail,
                );
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-import-failed",
                    format!("Import failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    // --- Private Keys：Export 子窗 ---------------------------------------

    pub fn open_private_keys_export_dialog(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.set_private_keys_status_kind(
                "private-key-export-requires-selection",
                "Select a managed key to export its public key.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        self.keys_export_visible = true;
        self.keys_export_path_text = self
            .config_dir
            .join(format!("{}.pub", entry.label))
            .display()
            .to_string();
        self.projection()
    }

    pub fn close_private_keys_export_dialog(&mut self) -> AppProjection {
        self.keys_export_visible = false;
        self.projection()
    }

    pub fn update_private_keys_export_path(&mut self, value: &str) -> AppProjection {
        self.keys_export_path_text = value.to_owned();
        self.projection()
    }

    pub fn export_private_key_to_path(&mut self) -> AppProjection {
        let Some(entry) = self.selected_private_key_entry() else {
            self.set_private_keys_status_kind(
                "private-key-export-requires-selection",
                "Select a managed key to export its public key.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        let public_line = match self.private_key_public_line(&entry.id) {
            Ok(line) => line,
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-export-failed",
                    format!("Export failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
                return self.projection();
            }
        };
        let path = self.keys_export_path_text.trim().to_owned();
        if path.is_empty() {
            self.set_private_keys_status_kind(
                "private-key-export-path-empty",
                "Choose a destination file first.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        match fs::write(&path, format!("{public_line}\n")) {
            Ok(()) => {
                self.keys_export_visible = false;
                let text = format!("Exported the public key to `{path}`.");
                self.set_status_kind(
                    "private-key-exported",
                    text.clone(),
                    path.clone(),
                    String::new(),
                );
                self.set_private_keys_status_kind(
                    "private-key-exported",
                    text,
                    path,
                    String::new(),
                );
            }
            Err(error) => {
                self.set_private_keys_status_kind(
                    "private-key-export-failed",
                    format!("Export failed: {error}"),
                    error.to_string(),
                    String::new(),
                );
            }
        }
        self.projection()
    }

    // --- Private Keys：Deploy 子窗 ---------------------------------------

    pub fn close_private_keys_deploy_dialog(&mut self) -> AppProjection {
        self.private_keys_deploy_confirm_visible = false;
        self.keys_deploy_target_index = 0;
        self.projection()
    }

    // --- Private Keys：行右键菜单 ----------------------------------------

    pub fn prepare_private_key_context_menu(&mut self, index: i32) -> AppProjection {
        let rows = self.private_key_row_data();
        let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| rows.get(index))
        else {
            return self.projection();
        };
        self.private_keys_selected_id = Some(row.id.clone());
        self.keys_menu_key_id = Some(row.id.clone());
        self.private_keys_passphrase_input.clear();
        self.private_keys_test_result_text.clear();
        self.private_keys_remove_confirm_visible = false;
        self.private_keys_deploy_confirm_visible = false;
        let text = format!("Selected managed key `{}`.", row.label);
        self.set_private_keys_status_kind(
            "private-key-selected",
            text,
            row.label.clone(),
            String::new(),
        );
        self.projection()
    }

    /// 行右键 Test：先按菜单目标选中，再走既有测试路径。
    pub fn test_private_key_from_menu(&mut self) -> AppProjection {
        self.apply_private_key_menu_selection();
        self.test_selected_private_key()
    }

    /// 行右键 Export public key：按菜单目标打开 Export 子窗。
    pub fn export_private_key_from_menu(&mut self) -> AppProjection {
        self.apply_private_key_menu_selection();
        self.open_private_keys_export_dialog()
    }

    /// 行右键 Deploy：按菜单目标打开 Deploy 子窗。
    pub fn deploy_private_key_from_menu(&mut self) -> AppProjection {
        self.apply_private_key_menu_selection();
        self.request_private_key_deploy()
    }

    /// 行右键 Remove：按菜单目标打开既有 Danger Zone 确认。
    pub fn remove_private_key_from_menu(&mut self) -> AppProjection {
        self.apply_private_key_menu_selection();
        self.request_private_key_remove()
    }

    fn apply_private_key_menu_selection(&mut self) {
        if let Some(key_id) = self.keys_menu_key_id.clone() {
            if self.private_key_entry(&key_id).is_some() {
                self.private_keys_selected_id = Some(key_id);
            }
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
    fn deployment_builds_an_idempotent_authorized_keys_command() {
        let deployment = AuthorizedKeysDeployment::new(ED25519_PLAIN_PUBLIC);
        let command = deployment.command();
        assert!(command.contains("mkdir -p"));
        assert!(command.contains("authorized_keys"));
        assert!(command.contains(ED25519_PLAIN_PUBLIC));
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

        // 元数据落在 keys/<label>.toml；私钥材料只在 keychain。
        let key_file = runtime.config_store.keys_dir().join("Work key.toml");
        assert!(key_file.exists(), "missing {}", key_file.display());
        let key_body = std::fs::read_to_string(&key_file).expect("read key file");
        assert!(key_body.contains("secret_ref"));
        assert!(!key_body.contains("BEGIN OPENSSH PRIVATE KEY"));
        let config_body =
            std::fs::read_to_string(runtime.config_store.config_file()).expect("config");
        assert!(!config_body.contains("[[keys]]"));

        // 重新加载后 keys 元数据仍在。
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

    // --- managers-dialogs：生成 / 导入 / 主机密钥单字段窗 / 行右键 -------------

    #[test]
    fn generate_private_key_round_trips_through_parse_and_fingerprint() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime.open_private_keys_manager();
        runtime.open_private_keys_generate_dialog();
        runtime.update_private_keys_generate_label("generated-ed25519");
        runtime.update_private_keys_generate_comment("generated comment");
        runtime.update_private_keys_generate_passphrase("generated-pass");
        runtime.update_private_keys_generate_confirm("generated-pass");
        assert_eq!(runtime.keys_generate_algorithm_index, 0);

        let projection = runtime.generate_private_key_from_form();
        assert!(projection.keys_generate_result_visible);
        assert!(projection
            .keys_generate_public_key_text
            .starts_with("ssh-ed25519 "));
        assert!(projection
            .keys_generate_fingerprint_text
            .starts_with("SHA256:"));

        // 生成材料可被现有 import/parse 流程读回，且指纹一致。
        let key_id = runtime
            .private_keys_selected_id
            .clone()
            .expect("generated key selected");
        let material = runtime.private_key_material(&key_id).expect("material");
        assert!(material.contains("BEGIN OPENSSH PRIVATE KEY"));
        let parsed = parse_private_key_material(&material, Some("generated-pass"))
            .expect("generated material parses");
        assert!(
            parsed.encrypted,
            "passphrase-protected material is encrypted"
        );
        assert!(parsed.decrypted);
        assert_eq!(
            parsed.fingerprint,
            runtime
                .private_key_entry(&key_id)
                .expect("entry")
                .fingerprint
        );
        assert!(runtime.generated_private_key_public_line().is_some());

        // 结果页 Done 关闭子窗，但保留已入库的密钥。
        let projection = runtime.finish_private_keys_generate_dialog();
        assert!(!projection.keys_generate_visible);
        assert_eq!(runtime.private_key_entries().len(), 1);
    }

    #[test]
    fn generate_private_key_rejects_passphrase_mismatch() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime.open_private_keys_generate_dialog();
        runtime.update_private_keys_generate_passphrase("one");
        runtime.update_private_keys_generate_confirm("two");

        let projection = runtime.generate_private_key_from_form();
        assert!(!projection.keys_generate_result_visible);
        assert!(projection
            .keys_generate_error_text
            .contains("does not match"));
        assert!(runtime.private_key_entries().is_empty());
    }

    #[test]
    fn host_keys_add_dialog_adds_and_updates_entries() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime.open_host_keys_manager();

        // Add：单字段粘贴 `host:port algorithm fingerprint`。
        runtime.open_host_keys_add_dialog();
        runtime.update_host_keys_add_text(
            "edge.example.com:22 ssh-ed25519 SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKwDEjsOE",
        );
        let projection = runtime.submit_host_keys_add_dialog();
        assert!(!projection.hosts_add_visible);
        assert_eq!(runtime.known_host_entries().len(), 1);
        assert_eq!(projection.hosts_add_error_text, "");

        // 非法行：报错并留在窗内。
        runtime.open_host_keys_add_dialog();
        runtime.update_host_keys_add_text("not-a-host-key");
        let projection = runtime.submit_host_keys_add_dialog();
        assert!(projection.hosts_add_visible);
        assert!(!projection.hosts_add_error_text.is_empty());
        assert_eq!(runtime.known_host_entries().len(), 1);

        // Edit：预填 + 改 host:port 时替换旧条目。
        runtime.open_host_keys_edit_dialog(0, 0);
        assert!(runtime.hosts_add_edit_target.is_some());
        runtime.update_host_keys_add_text(
            "edge2.example.com:2222 ssh-ed25519 SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKwDEjsOE",
        );
        let projection = runtime.submit_host_keys_add_dialog();
        assert!(!projection.hosts_add_visible);
        let entries = runtime.known_host_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host, "edge2.example.com");
        assert_eq!(entries[0].port, 2222);
    }

    #[test]
    fn host_keys_context_menu_tracks_target_and_removes_it() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime.open_host_keys_manager();
        runtime.open_host_keys_add_dialog();
        runtime.update_host_keys_add_text(
            "one.example.com:22 ssh-ed25519 SHA256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let _ = runtime.submit_host_keys_add_dialog();
        runtime.open_host_keys_add_dialog();
        runtime.update_host_keys_add_text(
            "two.example.com:22 ssh-ed25519 SHA256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let _ = runtime.submit_host_keys_add_dialog();
        assert_eq!(runtime.known_host_entries().len(), 2);

        let _ = runtime.prepare_host_keys_context_menu(1, 0);
        assert_eq!(runtime.hosts_menu_group, Some(1));
        assert_eq!(runtime.hosts_menu_entry, Some(0));
        let edited = runtime.open_host_keys_edit_dialog(1, 0);
        assert!(edited.hosts_add_edit_mode);
        let _ = runtime.cancel_host_keys_add_dialog();

        let _ = runtime.prepare_host_keys_context_menu(1, 0);
        let projection = runtime
            .remove_host_key_from_menu()
            .expect("remove from menu");
        assert_eq!(projection.host_keys_key_count, 1);
        assert_eq!(runtime.hosts_menu_group, None);
    }

    #[test]
    fn private_key_import_dialog_pastes_material_and_closes() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime.open_private_keys_manager();
        runtime.open_private_keys_import_dialog();
        runtime.select_private_keys_import_tab(1);
        runtime.update_private_keys_import_paste_text(ED25519_PLAIN);
        runtime.update_private_keys_import_label("pasted-key");

        let projection = runtime.import_private_key_from_dialog();
        assert!(!projection.keys_import_visible);
        assert_eq!(runtime.private_key_entries().len(), 1);
        assert_eq!(projection.keys_import_tab, 1);
        assert!(runtime.private_key_entries()[0]
            .fingerprint
            .starts_with("SHA256:"));
    }

    #[test]
    fn private_key_menu_actions_target_the_right_clicked_row() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        let first = runtime
            .import_private_key("first", ED25519_PLAIN, None, false)
            .expect("import");
        let second = runtime
            .import_private_key("second", ED25519_PLAIN, None, false)
            .expect("import");
        runtime.open_private_keys_manager();
        // 行模型按 label 排序：first, second。
        let _ = runtime.prepare_private_key_context_menu(1);
        assert_eq!(
            runtime.keys_menu_key_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_ne!(first.id, second.id);
        // 菜单目标已选中，导出子窗打开的是该行公钥。
        let projection = runtime.export_private_key_from_menu();
        assert!(projection.keys_export_visible);
        assert!(!projection.keys_export_public_key_text.is_empty());
        let _ = runtime.close_private_keys_export_dialog();
        assert!(!runtime.projection().keys_export_visible);
    }

    #[test]
    fn deploy_dialog_reports_when_no_ssh_target_exists() {
        let (_temp, mut runtime) = test_runtime_with_keychain();
        runtime
            .import_private_key("deploy-key", ED25519_PLAIN, None, false)
            .expect("import");
        runtime.open_private_keys_manager();
        let projection = runtime.request_private_key_deploy();
        assert!(projection.private_keys_deploy_confirm_visible);
        assert!(projection.keys_deploy_target_options.is_empty());
        assert_eq!(
            runtime.private_keys_status_kind,
            "private-key-deploy-no-targets"
        );
        let _ = runtime.close_private_keys_deploy_dialog();
        assert!(!runtime.private_keys_deploy_confirm_visible);
    }
}
