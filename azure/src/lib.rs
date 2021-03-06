#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_and_return,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::must_use_candidate,
	clippy::similar_names,
	clippy::too_many_lines,
)]

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
	feature = "log_analytics",
))]
mod auth;
#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
	feature = "log_analytics",
))]
pub use auth::Auth;

#[cfg(any(feature = "key_vault_cert", feature = "key_vault_key"))]
pub mod key_vault;

#[cfg(any(feature = "cdn", feature = "dns", feature = "log_analytics"))]
pub mod management;
