#[cfg(feature = "key_vault_cert")]
mod certificate;
#[cfg(feature = "key_vault_cert")]
pub use certificate::{Certificate, CreateCsrKeyType};

#[cfg(feature = "key_vault_key")]
mod key;
#[cfg(feature = "key_vault_key")]
pub use key::{EcKty, Jwk, Key};

use anyhow::Context;

impl<'a> crate::Account<'a> {
	async fn key_vault_authorization(&self) -> anyhow::Result<hyper::header::HeaderValue> {
		{
			let cached_key_vault_authorization = self.cached_key_vault_authorization.read().await;
			if let Some(authorization) = &*cached_key_vault_authorization {
				return Ok(authorization.clone());
			}
		}

		let mut cached_key_vault_authorization = self.cached_key_vault_authorization.write().await;
		match &mut *cached_key_vault_authorization {
			Some(authorization) => Ok(authorization.clone()),

			None => {
				eprintln!("Getting KeyVault API authorization...");
				let authorization =
					crate::get_authorization(
						&self.client,
						&self.auth,
						"https://vault.azure.net",
					).await.context("could not get KeyVault API authorization")?;
				eprintln!("Got KeyVault API authorization");
				*cached_key_vault_authorization = Some(authorization.clone());
				Ok(authorization)
			},
		}
	}

	fn key_vault_request_parameters(&self, key_vault_name: &str, relative_url: std::fmt::Arguments<'_>) ->
		impl std::future::Future<Output = anyhow::Result<(String, hyper::header::HeaderValue)>> + '_
	{
		let url = format!("https://{}.vault.azure.net{}", key_vault_name, relative_url);

		async move {
			let authorization = self.key_vault_authorization().await?;

			Ok((url, authorization))
		}
	}
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub enum EcCurve {
	#[serde(rename = "P-256")]
	P256,

	#[serde(rename = "P-384")]
	P384,

	#[serde(rename = "P-521")]
	P521,
}
