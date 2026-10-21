//! Linger's proxy, as one signed-in user: the account's details, App Store purchases, and deletion.

use std::sync::Arc;

use linger_core::api::{self, Api, ApiError};

use crate::study::{TokenProvider, Tokens};

/// Why a call to the proxy failed, with the message to show.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AccountError {
    /// The session ended: sign in again.
    #[error("{message}")]
    SignIn { message: String },
    #[error("{message}")]
    Failed { message: String },
}

impl From<ApiError> for AccountError {
    fn from(e: ApiError) -> Self {
        let message = e.to_string();
        match e {
            ApiError::SignIn(_) => Self::SignIn { message },
            _ => Self::Failed { message },
        }
    }
}

/// `GET /v1/me`: the plan and what's left of it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AccountInfo {
    pub email: Option<String>,
    /// For StoreKit purchases: ties an App Store transaction to this account.
    pub app_account_token: Option<String>,
    pub pro: bool,
    pub renews_at: Option<String>,
    /// "app_store" or "stripe".
    pub source: Option<String>,
    pub questions_left: u32,
    pub sermon_hours_left: f64,
    /// Library storage in bytes, when the server reports it.
    pub storage_used: Option<u64>,
    pub storage_limit: Option<u64>,
}

impl From<api::Me> for AccountInfo {
    fn from(me: api::Me) -> Self {
        Self {
            pro: me.is_pro(),
            email: me.user.email,
            app_account_token: me.user.app_account_token,
            renews_at: me.renews_at,
            source: me.source,
            questions_left: me.ask.questions_left,
            sermon_hours_left: me.sermons.hours_left,
            storage_used: me.library.as_ref().map(|l| l.storage_used),
            storage_limit: me.library.as_ref().map(|l| l.storage_limit),
        }
    }
}

/// The proxy at `base_url` (production when `None`), as the user `tokens` signs in. Its calls
/// block: make them off the main thread.
#[derive(uniffi::Object)]
pub struct Account {
    api: Api,
}

#[uniffi::export]
impl Account {
    #[uniffi::constructor]
    pub fn new(base_url: Option<String>, tokens: Arc<dyn TokenProvider>) -> Arc<Self> {
        let base = base_url.map_or_else(api::base_url, |u| u.trim_end_matches('/').to_string());
        Arc::new(Self { api: Api::new(base, Arc::new(Tokens(tokens))) })
    }

    pub fn me(&self) -> Result<AccountInfo, AccountError> {
        Ok(self.api.me()?.into())
    }

    /// An App Store purchase, restore or renewal: the server verifies the transaction's JWS and
    /// records it for this account. Returns the account after.
    pub fn attach_apple_transaction(&self, jws: String) -> Result<AccountInfo, AccountError> {
        Ok(self.api.attach_apple_transaction(&jws)?.into())
    }

    /// Deletes the account: the Clerk user, the library on the server, and any audio in flight.
    pub fn delete_account(&self) -> Result<(), AccountError> {
        Ok(self.api.delete_account()?)
    }
}
