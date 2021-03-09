use anyhow::Context;

#[cfg(feature = "cdn")]
pub mod cdn;

#[cfg(feature = "dns")]
mod dns;

#[cfg(feature = "log_analytics")]
pub mod log_analytics;

pub struct Client<'a> {
	subscription_id: &'a str,
	resource_group_name: &'a str,
	auth: &'a crate::Auth,

	client: http_common::Client,
	cached_authorization: tokio::sync::RwLock<Option<hyper::header::HeaderValue>>,
}

impl<'a> Client<'a> {
	pub fn new(
		subscription_id: &'a str,
		resource_group_name: &'a str,
		auth: &'a crate::Auth,
		user_agent: hyper::header::HeaderValue,
	) -> anyhow::Result<Self> {
		Ok(Client {
			subscription_id,
			resource_group_name,
			auth,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			cached_authorization: Default::default(),
		})
	}

	async fn authorization(&self) -> anyhow::Result<hyper::header::HeaderValue> {
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
				const RESOURCE: &str = "https://management.azure.com";

				let authorization =
					self.auth.get_authorization(&self.client, RESOURCE).await
					.context("could not get Management API authorization")?;
				*cached_authorization = Some(authorization.clone());
				Ok(authorization)
			},
		}
	}

	fn request_parameters(&self, relative_url: std::fmt::Arguments<'_>) ->
		impl std::future::Future<Output = anyhow::Result<(String, hyper::header::HeaderValue)>> + '_
	{
		let url = format!("https://management.azure.com/subscriptions/{}/resourceGroups/{}{}", self.subscription_id, self.resource_group_name, relative_url);
		async move {
			let authorization = self.authorization().await?;
			Ok((url, authorization))
		}
	}
}
