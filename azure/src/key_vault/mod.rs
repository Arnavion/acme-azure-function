mod certificate;
pub use certificate::{Certificate, CreateCsrKeyType};

mod key;
pub use key::{EcKty, Key};

use anyhow::Context;

pub struct Client<'a> {
	key_vault_name: &'a str,
	auth: &'a crate::Auth,

	client: http_common::Client,
	cached_authorization: tokio::sync::RwLock<Option<http::HeaderValue>>,
	logger: &'a log2::Logger,
}

impl<'a> Client<'a> {
	pub fn new(
		key_vault_name: &'a str,
		auth: &'a crate::Auth,
		user_agent: http::HeaderValue,
		logger: &'a log2::Logger,
	) -> anyhow::Result<Self> {
		Ok(Client {
			key_vault_name,
			auth,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			cached_authorization: Default::default(),
			logger,
		})
	}

	async fn authorization(&self) -> anyhow::Result<http::HeaderValue> {
		{
			let cached_authorization = self.cached_authorization.read().await;
			if let Some(authorization) = &*cached_authorization {
				return Ok(authorization.clone());
			}
		}

		let mut cached_authorization = self.cached_authorization.write().await;
		match &mut *cached_authorization {
			Some(authorization) => Ok(authorization.clone()),

			None => {
				const RESOURCE: &str = "https://vault.azure.net";

				let authorization =
					self.auth.get_authorization(&self.client, RESOURCE, self.logger).await
					.context("could not get KeyVault API authorization")?;
				*cached_authorization = Some(authorization.clone());
				Ok(authorization)
			},
		}
	}

	fn request_parameters(&self, relative_url: std::fmt::Arguments<'_>) ->
		impl std::future::Future<Output = anyhow::Result<(String, http::HeaderValue)>> + '_
	{
		let url = format!("https://{}.vault.azure.net{}", self.key_vault_name, relative_url);
		async move {
			let authorization = self.authorization().await?;
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
