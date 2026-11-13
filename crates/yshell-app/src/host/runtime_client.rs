//! Local / remote AppRuntime access for Shell (D38 Phase 4).

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;

use super::shell_ipc::HostShellClient;

/// Handle used by WindowManager / bootstrap instead of bare `Rc<RefCell<AppRuntime>>`.
#[derive(Clone)]
pub enum AppRuntimeClient {
    /// In-process (tests / `YSHELL_TEST_INPROCESS_HOST=1`).
    Local(Rc<RefCell<AppRuntime>>),
    /// Out-of-process Host via shell IPC.
    Remote(Arc<HostShellClient>),
}

impl AppRuntimeClient {
    #[must_use]
    pub fn local(runtime: Rc<RefCell<AppRuntime>>) -> Self {
        Self::Local(runtime)
    }

    #[must_use]
    pub fn remote(client: Arc<HostShellClient>) -> Self {
        Self::Remote(client)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    #[allow(dead_code)]
    pub fn borrow_local(&self) -> AppResult<Ref<'_, AppRuntime>> {
        match self {
            Self::Local(runtime) => Ok(runtime.borrow()),
            Self::Remote(_) => Err(AppError::new(
                "local AppRuntime borrow is unavailable in remote Shell mode",
            )),
        }
    }

    #[allow(dead_code)]
    pub fn borrow_mut_local(&self) -> AppResult<RefMut<'_, AppRuntime>> {
        match self {
            Self::Local(runtime) => Ok(runtime.borrow_mut()),
            Self::Remote(_) => Err(AppError::new(
                "local AppRuntime borrow is unavailable in remote Shell mode",
            )),
        }
    }

    /// Prefer this for UI callbacks — works in both Local and Remote modes
    /// when `method` is registered in `runtime_dispatch`. Returns wire JSON.
    #[allow(dead_code)] // used by remote Shell / future WindowManager migration
    pub fn call_projection(&self, method: &str, args: Value) -> AppResult<Value> {
        self.call_value(method, args)
    }

    #[allow(dead_code)] // remote Shell startup
    pub fn call_null(&self, method: &str, args: Value) -> AppResult<()> {
        match self {
            Self::Local(runtime) => {
                let mut runtime = runtime.borrow_mut();
                let _ = super::runtime_dispatch::dispatch(&mut runtime, method, args)
                    .map_err(AppError::new)?;
                Ok(())
            }
            Self::Remote(client) => {
                let _ = client.ui_call(method, args)?;
                Ok(())
            }
        }
    }

    #[allow(dead_code)]
    pub fn call_bool(&self, method: &str, args: Value) -> AppResult<bool> {
        match self {
            Self::Local(runtime) => {
                let mut runtime = runtime.borrow_mut();
                let value = super::runtime_dispatch::dispatch(&mut runtime, method, args)
                    .map_err(AppError::new)?;
                Ok(value.as_bool().unwrap_or(false))
            }
            Self::Remote(client) => {
                let value = client.ui_call(method, args)?;
                Ok(value.as_bool().unwrap_or(false))
            }
        }
    }

    pub fn call_value(&self, method: &str, args: Value) -> AppResult<Value> {
        match self {
            Self::Local(runtime) => {
                let mut runtime = runtime.borrow_mut();
                super::runtime_dispatch::dispatch(&mut runtime, method, args).map_err(AppError::new)
            }
            Self::Remote(client) => client.ui_call(method, args),
        }
    }

    #[allow(dead_code)]
    pub fn projection_for_window(&self, window_id: u64) -> AppResult<Value> {
        self.call_value("projection_for_window", json!([window_id]))
    }

    #[allow(dead_code)]
    pub fn host_tick(&self) -> AppResult<(bool, u32, bool)> {
        match self {
            Self::Local(runtime) => {
                let mut runtime = runtime.borrow_mut();
                let dirty = match runtime.poll_all_terminal_outputs() {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(error) => return Err(AppError::from_error(error)),
                };
                Ok((dirty || runtime.take_ui_dirty(), 0, false))
            }
            Self::Remote(client) => client.host_tick(),
        }
    }

    #[allow(dead_code)]
    pub fn remote_client(&self) -> Option<Arc<HostShellClient>> {
        match self {
            Self::Remote(client) => Some(Arc::clone(client)),
            Self::Local(_) => None,
        }
    }

    #[allow(dead_code)]
    pub fn local_rc(&self) -> Option<Rc<RefCell<AppRuntime>>> {
        match self {
            Self::Local(runtime) => Some(Rc::clone(runtime)),
            Self::Remote(_) => None,
        }
    }
}
