//! Global logging settings and per-session logging toggles.
//!
//! N6（终端日志 UI，设计 `docs/product/yshell-next-n6-logging-ui.md`）在
//! [`AppRuntime`] 上增加每会话手动日志开关：
//!
//! * [`AppRuntime::start_session_logging`] / [`AppRuntime::stop_session_logging`]：
//!   弹窗（`ui/components/logging_dialog.slint`）的语义入口，复用
//!   `SessionLogger::open_with`（Append/Truncate、时间戳、raw/sanitized）。
//! * [`AppRuntime::stop_session_logging_for_close`] / [`AppRuntime::stop_all_session_logging`]：
//!   标签关闭 / 应用退出的强制停止与 flush 入口（Phase 2 接线）。
//! * [`AppRuntime::flush_session_logging`]：定期 flush。
//!
//! 同一会话同一时刻只有一个日志输出（手动优先，开启时暂停自动日志，停止后不
//! 自动恢复）；写失败由 `SessionRuntime` 自动停止并把 `SessionLogger::last_error`
//! 折进日志提示，经 [`AppRuntime::fold_logging_notice_from_session`] 反映到状态栏。

use std::path::{Path, PathBuf};

use crate::session_runtime::{SessionLoggingMode, SessionSource};
use yshell_config::LoggingProfile;
use yshell_logging::{LogMode, LogPathContext, PathTemplate, SessionLogOptions, TranscriptFormat};

use super::*;

impl AppRuntime {
    pub fn toggle_global_logging_enabled(&mut self) -> AppProjection {
        self.config_document.logging.enabled = !self.config_document.logging.enabled;
        self.status_text = format!(
            "Global logging is now {}. Save logging settings to persist this policy.",
            if self.config_document.logging.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        self.projection()
    }

    pub fn set_global_logging_format_raw(&mut self) -> AppProjection {
        self.config_document.logging.format = "raw".to_owned();
        self.status_text =
            "Global logging format set to raw transcript. Save logging settings to persist."
                .to_owned();
        self.projection()
    }

    pub fn set_global_logging_format_sanitized(&mut self) -> AppProjection {
        self.config_document.logging.format = "sanitized".to_owned();
        self.status_text =
            "Global logging format set to sanitized text. Save logging settings to persist."
                .to_owned();
        self.projection()
    }

    pub fn update_global_logging_directory(&mut self, value: &str) -> AppProjection {
        let directory = value.trim();
        self.config_document.logging.directory = if directory.is_empty() {
            None
        } else {
            Some(directory.to_owned())
        };
        self.projection()
    }

    pub fn save_global_logging_settings(&mut self) -> AppResult<AppProjection> {
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        let summary = self.logging_summary_legacy_text();
        self.set_status_kind(
            "logging-saved",
            format!("Saved global logging policy: {summary}."),
            summary,
            String::new(),
        );
        Ok(self.projection())
    }

    pub(crate) fn logging_enabled_text(&self) -> String {
        if self.config_document.logging.enabled {
            "enabled".to_owned()
        } else {
            "disabled".to_owned()
        }
    }

    pub(crate) fn logging_format_text(&self) -> String {
        if self
            .config_document
            .logging
            .format
            .eq_ignore_ascii_case("raw")
        {
            "raw".to_owned()
        } else {
            "sanitized".to_owned()
        }
    }

    pub(crate) fn logging_directory_text(&self) -> String {
        self.config_document
            .logging
            .directory
            .clone()
            .unwrap_or_default()
    }

    pub(crate) fn logging_directory_display_text(&self) -> String {
        self.config_document
            .logging
            .directory
            .as_deref()
            .filter(|directory| !directory.trim().is_empty())
            .unwrap_or("logs")
            .to_owned()
    }

    pub(crate) fn logging_summary_legacy_text(&self) -> String {
        format!(
            "global={} format={} directory={}",
            self.logging_enabled_text(),
            self.logging_format_text(),
            self.logging_directory_display_text()
        )
    }

    pub(crate) fn resolved_logging_profile_for_runtime(
        &self,
        runtime: &SessionRuntime,
    ) -> LoggingProfile {
        match &runtime.source {
            SessionSource::SavedSession { profile_id } => self
                .config_document
                .resolve_session(profile_id)
                .map(|resolved| resolved.logging)
                .unwrap_or_else(|| self.config_document.logging.clone()),
            SessionSource::QuickConnect | SessionSource::Draft => {
                self.config_document.logging.clone()
            }
        }
    }

    pub(crate) fn configure_runtime_logging(&self, runtime: &mut SessionRuntime) {
        let logging = self.resolved_logging_profile_for_runtime(runtime);
        runtime.configure_logging(&self.config_dir, &logging);
    }

    pub(crate) fn fold_logging_notice_from_session(&mut self, session_key: &str) {
        let notice = self
            .sessions
            .get_mut(session_key)
            .and_then(SessionRuntime::take_logging_notice);
        if let Some(notice) = notice {
            self.fold_logging_notice_into_status(notice);
        }
    }

    pub(crate) fn fold_logging_notice_into_status(&mut self, notice: String) {
        if self.status_text.is_empty() {
            self.status_text = format!("Logging notice: {notice}");
        } else {
            self.status_text = format!("{} Logging notice: {}", self.status_text, notice);
        }
    }
}

/// N6：运行时开启终端日志的请求（日志弹窗 → 运行时）。
///
/// 弹窗只产出路径与选项，不内嵌文件逻辑：路径/文件名校验由
/// [`validate_log_file_name`] 等纯函数在前端完成，打开失败由
/// [`AppRuntime::start_session_logging`] 返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionLoggingRequest {
    /// 目标文件（保存位置 + 文件名）。
    pub path: PathBuf,
    /// `Raw` 保留 ANSI 字节 / `Sanitized` 纯文本；设计默认 sanitized。
    pub format: TranscriptFormat,
    /// 行首本地 `[HH:MM:SS]` 时间戳；设计默认开启。
    pub timestamps: bool,
    /// 是否记录本地输入（设计默认关，开启需单独确认，R-106）。
    pub include_input: bool,
    /// 文件已存在时的确认结果：追加或覆盖（截断）。
    pub mode: LogMode,
}

/// N6：设计默认值 —— sanitized、时间戳开、不记输入、追加写。
impl SessionLoggingRequest {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: TranscriptFormat::Sanitized,
            timestamps: true,
            include_input: false,
            mode: LogMode::Append,
        }
    }
}

/// N6：日志文件名非法字符校验（弹窗字段校验；不访问文件系统）。
///
/// 拒绝：空/纯空白、`.`/`..`、路径分隔符与控制字符，以及 Windows 保留字符
/// （`:` `*` `?` `"` `<` `>` `|`）。是否覆盖已存在文件由调用方单独确认。
pub(crate) fn validate_log_file_name(file_name: &str) -> Result<(), String> {
    let name = file_name.trim();
    if name.is_empty() {
        return Err("Enter a file name.".to_owned());
    }
    if name == "." || name == ".." {
        return Err("`.` and `..` are not valid file names.".to_owned());
    }
    for character in name.chars() {
        if character.is_control()
            || matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        {
            return Err(format!("The file name cannot contain `{character}`."));
        }
    }
    Ok(())
}

/// N6：默认日志文件名 `{session}-{timestamp}.log`。
///
/// 与自动日志命名（`{session_id}/{kind}-{timestamp}.log`）使用同一
/// [`PathTemplate`] 语义：`{timestamp}` 为 Unix 秒，会话标签里的非法字符转 `_`。
pub(crate) fn default_session_log_file_name(session_label: &str) -> String {
    let context = LogPathContext::now(session_label, "terminal");
    PathTemplate::new("{session_id}-{timestamp}.log")
        .render(&context)
        .display()
        .to_string()
}

/// N6：把保存位置与文件名组装成完整路径（不检查存在性）。
pub(crate) fn session_log_path_for(directory: &Path, file_name: &str) -> PathBuf {
    directory.join(file_name.trim())
}

/// N6：终端日志弹窗（开启表单 / 停止提示）的运行时状态。
///
/// 文本字段以 Rust 为准（弹窗编辑回调进来后重新校验）；布尔字段由 Slint 的
/// `in-out` 属性驱动、经回调写回，投影时原样下发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggingDialogState {
    pub(crate) visible: bool,
    /// 目标会话语义 key（打开弹窗时取 active session；会话消失时提示）。
    pub(crate) session_key: String,
    pub(crate) session_name_text: String,
    pub(crate) directory_text: String,
    pub(crate) file_name_text: String,
    pub(crate) file_name_error_text: String,
    pub(crate) path_error_text: String,
    pub(crate) file_exists: bool,
    /// 格式：`true` = raw（保留 ANSI），`false` = sanitized（默认）。
    pub(crate) raw_format: bool,
    pub(crate) timestamps: bool,
    pub(crate) include_input: bool,
    pub(crate) input_confirmed: bool,
    pub(crate) overwrite_confirmed: bool,
}

impl Default for LoggingDialogState {
    fn default() -> Self {
        Self {
            visible: false,
            session_key: String::new(),
            session_name_text: String::new(),
            directory_text: String::new(),
            file_name_text: String::new(),
            file_name_error_text: String::new(),
            path_error_text: String::new(),
            file_exists: false,
            raw_format: false,
            timestamps: true,
            include_input: false,
            input_confirmed: false,
            overwrite_confirmed: false,
        }
    }
}

/// N6：每会话手动日志开关 / 弹窗状态 / 关闭退出入口。
impl AppRuntime {
    /// N6：开始某个运行时会话的手动日志。
    ///
    /// 手动日志优先：调用后本会话的自动日志暂停，且停止后不自动恢复。打开失败
    /// 时现有日志不受影响，状态写入 `logging-error` 并返回错误（弹窗保持打开）。
    pub(crate) fn start_session_logging(
        &mut self,
        session_key: &str,
        request: &SessionLoggingRequest,
    ) -> AppResult<AppProjection> {
        let options = SessionLogOptions::new(request.path.clone())
            .with_format(request.format)
            .with_timestamps(request.timestamps)
            .with_mode(request.mode);
        let start_result = self
            .sessions
            .get_mut(session_key)
            .ok_or_else(|| AppError::new(format!("no runtime session `{session_key}`")))?
            .start_logging(options, request.include_input);
        let path = match start_result {
            Ok(path) => path,
            Err(error) => {
                let path_text = request.path.display().to_string();
                let detail = error.to_string();
                self.set_status_kind(
                    "logging-error",
                    format!("Could not open log file `{path_text}`: {detail}"),
                    path_text,
                    detail,
                );
                return Err(AppError::new(format!(
                    "could not open log file `{}`: {error}",
                    request.path.display()
                )));
            }
        };
        let path_text = path.display().to_string();
        let format_text = transcript_format_label(request.format);
        self.set_status_kind(
            "logging-started",
            format!("Logging terminal output to `{path_text}` ({format_text})."),
            path_text,
            format_text.to_owned(),
        );
        Ok(self.projection())
    }

    /// N6：停止某个会话的日志输出（手动或配置驱动的自动日志）并 flush。
    ///
    /// 停止后本会话不会自动恢复自动日志。没有活动日志时返回错误（菜单项据此
    /// 禁用）；flush/关闭失败时状态写入 `logging-error`，已写内容保留。
    pub(crate) fn stop_session_logging(&mut self, session_key: &str) -> AppResult<AppProjection> {
        let mode_before = self
            .sessions
            .get(session_key)
            .map(SessionRuntime::logging_mode)
            .ok_or_else(|| AppError::new(format!("no runtime session `{session_key}`")))?;
        if mode_before == SessionLoggingMode::Off {
            return Err(AppError::new(format!(
                "session `{session_key}` is not logging"
            )));
        }
        let stop_result = self
            .sessions
            .get_mut(session_key)
            .expect("session existence was checked above")
            .stop_logging();
        match stop_result {
            Ok(Some(path)) => {
                let path_text = path.display().to_string();
                let mode_text = logging_mode_label(mode_before);
                self.set_status_kind(
                    "logging-stopped",
                    format!("Stopped {mode_text} logging. Log file: `{path_text}`."),
                    path_text,
                    mode_text.to_owned(),
                );
                Ok(self.projection())
            }
            Ok(None) => Err(AppError::new(format!(
                "session `{session_key}` is not logging"
            ))),
            Err(error) => {
                let path_text = self
                    .sessions
                    .get(session_key)
                    .and_then(SessionRuntime::last_logging_path)
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                let detail = error.to_string();
                self.set_status_kind(
                    "logging-error",
                    format!("Log file flush failed for `{path_text}`: {detail}"),
                    path_text,
                    detail,
                );
                Err(AppError::new(error.to_string()))
            }
        }
    }

    /// N6：标签关闭 / 会话断开路径的强制停止入口（Phase 2 调用）。
    ///
    /// 停止并 flush 该会话的活动日志，返回日志文件路径文本（用于状态提示）。
    /// 没有活动日志时返回 `None`。
    pub(crate) fn stop_session_logging_for_close(&mut self, session_key: &str) -> Option<String> {
        let session = self.sessions.get_mut(session_key)?;
        match session.stop_logging() {
            Ok(Some(path)) => Some(path.display().to_string()),
            Ok(None) => None,
            // flush 失败也要给出文件位置：已写内容保留在最近路径上。
            Err(_) => session
                .last_logging_path()
                .map(|path| path.display().to_string()),
        }
    }

    /// N6：应用退出路径的强制 flush 入口（Phase 2 调用）。
    ///
    /// 停止所有活动日志输出并 flush，返回停止的会话数。
    pub(crate) fn stop_all_session_logging(&mut self) -> usize {
        let session_keys: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.logging_active())
            .map(|(key, _)| key.clone())
            .collect();
        let mut stopped = 0;
        for session_key in session_keys {
            if let Some(session) = self.sessions.get_mut(&session_key) {
                if session.stop_logging().is_ok() {
                    stopped += 1;
                }
            }
        }
        stopped
    }

    /// N6：会话是否有活动日志输出（REC 指示 / 菜单启用条件）。
    pub(crate) fn session_logging_active(&self, session_key: &str) -> bool {
        self.sessions
            .get(session_key)
            .is_some_and(SessionRuntime::logging_active)
    }

    /// N6：会话的活动日志是否由手动开启（REC 归属：manual/auto）。
    pub(crate) fn session_logging_mode(&self, session_key: &str) -> SessionLoggingMode {
        self.sessions
            .get(session_key)
            .map(SessionRuntime::logging_mode)
            .unwrap_or_default()
    }

    /// N6：状态栏 / "打开日志文件"用的路径文本（活动文件，回退最近文件）。
    pub(crate) fn session_logging_path_text(&self, session_key: &str) -> String {
        let Some(session) = self.sessions.get(session_key) else {
            return String::new();
        };
        session
            .logging_path()
            .or_else(|| session.last_logging_path())
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    /// N6：活动会话的日志文件路径（右键菜单/状态栏"打开"入口用）。
    pub(crate) fn active_session_logging_path_text(&self) -> String {
        self.active_session_id
            .as_deref()
            .map(|session_key| self.session_logging_path_text(session_key))
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- 弹窗

    /// N6：打开终端日志弹窗（针对活动会话；默认 = 上次目录/配置 logs 目录 + 默认文件名）。
    pub(crate) fn open_logging_dialog(&mut self) -> AppProjection {
        let Some(session_key) = self.active_session_id.clone() else {
            self.set_status_kind(
                "logging-no-session",
                "Open a session before starting terminal logging.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        };
        let session_name = self
            .sessions
            .get(&session_key)
            .map(|session| session.display_name.clone())
            .unwrap_or_else(|| session_key.clone());
        let directory_text = self.default_logging_directory_text();
        let file_name_text = default_session_log_file_name(&session_name);
        self.logging_dialog = LoggingDialogState {
            visible: true,
            session_key,
            session_name_text: session_name,
            directory_text,
            file_name_text,
            ..LoggingDialogState::default()
        };
        self.refresh_logging_dialog_file_state();
        self.projection()
    }

    /// N6：关闭弹窗并清空临时状态。
    pub(crate) fn close_logging_dialog(&mut self) -> AppProjection {
        self.logging_dialog = LoggingDialogState::default();
        self.projection()
    }

    /// N6：弹窗目录字段编辑（重新校验文件存在性）。
    pub(crate) fn update_logging_dialog_directory(&mut self, value: &str) -> AppProjection {
        self.logging_dialog.directory_text = value.to_owned();
        self.logging_dialog.path_error_text.clear();
        self.refresh_logging_dialog_file_state();
        self.projection()
    }

    /// N6：弹窗文件名字段编辑（非法字符/空值内联提示）。
    pub(crate) fn update_logging_dialog_file_name(&mut self, value: &str) -> AppProjection {
        self.logging_dialog.file_name_text = value.to_owned();
        self.logging_dialog.path_error_text.clear();
        self.logging_dialog.overwrite_confirmed = false;
        self.refresh_logging_dialog_file_state();
        self.projection()
    }

    /// N6：写入弹窗的错误提示（如系统目录选择器不可用的降级提示）。
    pub(crate) fn set_logging_dialog_path_error(&mut self, message: &str) -> AppProjection {
        self.logging_dialog.path_error_text = message.to_owned();
        self.projection()
    }

    /// N6：弹窗开关快照（任一开关变化时由弹窗回传；关闭"记录输入"时撤销确认）。
    pub(crate) fn set_logging_dialog_options(
        &mut self,
        raw_format: bool,
        timestamps: bool,
        include_input: bool,
        input_confirmed: bool,
        overwrite_confirmed: bool,
    ) -> AppProjection {
        self.logging_dialog.raw_format = raw_format;
        self.logging_dialog.timestamps = timestamps;
        self.logging_dialog.include_input = include_input;
        self.logging_dialog.input_confirmed = include_input && input_confirmed;
        self.logging_dialog.overwrite_confirmed = overwrite_confirmed;
        self.projection()
    }

    /// N6：按弹窗当前字段开启日志；失败时保留弹窗并回填 `path_error_text`。
    ///
    /// 校验类问题（文件名非法/目录为空）只更新弹窗字段；会话消失或打开文件失败
    /// 同样保留弹窗，方便用户改路径重试。
    pub(crate) fn start_logging_from_dialog(&mut self) -> AppResult<AppProjection> {
        if !self.logging_dialog.visible {
            return Err(AppError::new("the logging dialog is not open"));
        }
        self.refresh_logging_dialog_file_state();
        if !self.logging_dialog.file_name_error_text.is_empty() {
            return Ok(self.projection());
        }
        let session_key = self.logging_dialog.session_key.clone();
        if !self.sessions.contains_key(&session_key) {
            self.logging_dialog.path_error_text =
                "The session is no longer available. Close the dialog and try again.".to_owned();
            return Ok(self.projection());
        }
        let directory_text = self.logging_dialog.directory_text.trim().to_owned();
        if directory_text.is_empty() {
            self.logging_dialog.path_error_text = "Choose a folder for the log file.".to_owned();
            return Ok(self.projection());
        }
        let directory = PathBuf::from(&directory_text);
        let path = session_log_path_for(&directory, &self.logging_dialog.file_name_text);
        let mut request = SessionLoggingRequest::new(path);
        request.format = if self.logging_dialog.raw_format {
            TranscriptFormat::Raw
        } else {
            TranscriptFormat::Sanitized
        };
        request.timestamps = self.logging_dialog.timestamps;
        request.include_input = self.logging_dialog.include_input;
        request.mode = if self.logging_dialog.file_exists && self.logging_dialog.overwrite_confirmed
        {
            LogMode::Truncate
        } else {
            LogMode::Append
        };
        match self.start_session_logging(&session_key, &request) {
            Ok(_) => {
                // 上次使用目录：下一次弹窗的默认保存位置（运行时会话态，不写配置）。
                self.last_logging_directory = Some(directory);
                self.logging_dialog = LoggingDialogState::default();
                Ok(self.projection())
            }
            Err(error) => {
                self.logging_dialog.path_error_text = error.to_string();
                Ok(self.projection())
            }
        }
    }

    /// N6：弹窗"停止日志"（停止提示视图）。
    pub(crate) fn stop_logging_from_dialog(&mut self) -> AppResult<AppProjection> {
        let session_key = if self.logging_dialog.visible {
            self.logging_dialog.session_key.clone()
        } else {
            self.active_session_id.clone().unwrap_or_default()
        };
        if session_key.is_empty() {
            return Err(AppError::new("no runtime session is available for logging"));
        }
        self.stop_session_logging(&session_key)?;
        self.logging_dialog = LoggingDialogState::default();
        Ok(self.projection())
    }

    /// N6：右键菜单/状态栏"Start Logging…"：打开弹窗。
    ///
    /// 已在记录时弹窗显示"停止提示"视图（当前文件 + Open File/Folder + Stop）；
    /// 没有活动会话时只更新状态（菜单项此时禁用）。
    pub(crate) fn terminal_logging_start(&mut self) -> AppProjection {
        if self.logging_dialog.visible {
            return self.projection();
        }
        if self.active_session_id.is_none() {
            self.set_status_kind(
                "logging-no-session",
                "Open a session before starting terminal logging.".to_owned(),
                String::new(),
                String::new(),
            );
            return self.projection();
        }
        self.open_logging_dialog()
    }

    /// N6：右键菜单/状态栏"Stop Logging"：停止活动会话的日志输出。
    pub(crate) fn terminal_logging_stop(&mut self) -> AppResult<AppProjection> {
        let Some(session_key) = self.active_session_id.clone() else {
            return Err(AppError::new("no runtime session is available for logging"));
        };
        self.stop_session_logging(&session_key)
    }

    /// N6：弹窗默认目录：上次使用目录 → 配置的 logs 目录（解析到绝对路径）。
    fn default_logging_directory_text(&self) -> String {
        if let Some(directory) = &self.last_logging_directory {
            return directory.display().to_string();
        }
        let configured = self
            .config_document
            .logging
            .directory
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("logs"));
        let resolved = if configured.is_absolute() {
            configured
        } else {
            self.config_dir.join(configured)
        };
        resolved.display().to_string()
    }

    /// N6：重新计算文件名合法性/覆盖确认（目录或文件名变化后调用）。
    fn refresh_logging_dialog_file_state(&mut self) {
        let file_name = self.logging_dialog.file_name_text.clone();
        let directory = self.logging_dialog.directory_text.trim().to_owned();
        self.logging_dialog.file_name_error_text =
            validate_log_file_name(&file_name).err().unwrap_or_default();
        self.logging_dialog.file_exists = self.logging_dialog.file_name_error_text.is_empty()
            && !directory.is_empty()
            && session_log_path_for(Path::new(&directory), &file_name).is_file();
    }
}

fn transcript_format_label(format: TranscriptFormat) -> &'static str {
    match format {
        TranscriptFormat::Raw => "raw",
        TranscriptFormat::Sanitized => "sanitized",
    }
}

fn logging_mode_label(mode: SessionLoggingMode) -> &'static str {
    match mode {
        SessionLoggingMode::Off => "off",
        SessionLoggingMode::Auto => "automatic",
        SessionLoggingMode::Manual => "manual",
    }
}
