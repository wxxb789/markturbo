//! GPUI bridge for the GUI-independent credential vault in `mt-core`.

use std::ops::Deref;
use std::sync::Arc;

use gpui_kit::{App, Global};
use mt_core::credentials::{CredentialVault, SecureCredentialStore};

#[derive(Clone)]
pub struct AppCredentialVault(CredentialVault);

impl Global for AppCredentialVault {}

impl AppCredentialVault {
    pub fn production() -> Self {
        Self(CredentialVault::production())
    }

    pub fn with_store(secure: Arc<dyn SecureCredentialStore>) -> Self {
        Self(CredentialVault::with_store(secure))
    }

    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::production());
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

impl Deref for AppCredentialVault {
    type Target = CredentialVault;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
