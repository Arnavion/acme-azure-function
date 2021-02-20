#[cfg(feature = "key_vault_cert")]
mod certificate;
#[cfg(feature = "key_vault_cert")]
pub use certificate::{Certificate, CreateCsrKeyType};

#[cfg(feature = "key_vault_key")]
mod key;
#[cfg(feature = "key_vault_key")]
pub use key::{EcCurve, EcKty, Jwk, Key};

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

	async fn key_vault_request_parameters(&self, key_vault_name: &str, relative_url: &str) -> anyhow::Result<(String, hyper::header::HeaderValue)> {
		let url = format!("https://{}.vault.azure.net{}", key_vault_name, relative_url);

		let authorization = self.key_vault_authorization().await?;

		Ok((url, authorization))
	}
}
