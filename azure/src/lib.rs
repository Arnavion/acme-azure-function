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
))]
use anyhow::Context;

#[cfg(feature = "cdn")]
mod cdn;
#[cfg(feature = "cdn")]
pub use cdn::{
	CustomDomainSecret as CdnCustomDomainSecret,
	CustomDomainKeyVaultSecret as CdnCustomDomainKeyVaultSecret,
};

#[cfg(feature = "dns")]
mod dns;

#[cfg(any(feature = "key_vault_cert", feature = "key_vault_key"))]
mod key_vault;
#[cfg(feature = "key_vault_cert")]
pub use key_vault::{
	Certificate as KeyVaultCertificate,
	CreateCsrKeyType as KeyVaultCreateCsrKeyType,
};
#[cfg(feature = "key_vault_key")]
pub use key_vault::{
	EcCurve,
	EcKty,
	Key as KeyVaultKey,
};

#[cfg(feature = "log_analytics")]
mod log_analytics;
#[cfg(feature = "log_analytics")]
pub use log_analytics::{
	LogSender as LogAnalyticsLogSender,
};

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]
pub struct Account<'a> {
	#[cfg_attr(not(any(feature = "cdn", feature = "dns")), allow(dead_code))]
	subscription_id: &'a str,
	#[cfg_attr(not(any(feature = "cdn", feature = "dns")), allow(dead_code))]
	resource_group_name: &'a str,
	auth: &'a Auth,

	client: http_common::Client,

	#[cfg(any(feature = "cdn", feature = "dns"))]
	cached_management_authorization: tokio::sync::RwLock<Option<hyper::header::HeaderValue>>,

	#[cfg(any(feature = "key_vault_cert", feature = "key_vault_key"))]
	cached_key_vault_authorization: tokio::sync::RwLock<Option<hyper::header::HeaderValue>>,
}

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]
impl<'a> Account<'a> {
	pub fn new(
		subscription_id: &'a str,
		resource_group_name: &'a str,
		auth: &'a Auth,
		user_agent: &str,
	) -> anyhow::Result<Self> {
		Ok(Account {
			subscription_id,
			resource_group_name,
			auth,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,

			#[cfg(any(feature = "cdn", feature = "dns"))]
			cached_management_authorization: Default::default(),

			#[cfg(any(feature = "key_vault_cert", feature = "key_vault_key"))]
			cached_key_vault_authorization: Default::default(),
		})
	}

	#[cfg(any(feature = "cdn", feature = "dns"))]
	async fn management_authorization(&self) -> anyhow::Result<hyper::header::HeaderValue> {
		{
			let cached_management_authorization = self.cached_management_authorization.read().await;
			if let Some(authorization) = &*cached_management_authorization {
				return Ok(authorization.clone());
			}
		}

		let mut cached_management_authorization = self.cached_management_authorization.write().await;
		match &mut *cached_management_authorization {
			Some(authorization) => Ok(authorization.clone()),

			None => {
				const RESOURCE: &str = "https://management.azure.com";

				let log2::Secret(authorization) = log2::report_operation("azure/authorization", RESOURCE, <log2::ScopedObjectOperation>::Get, async {
					let authorization =
						get_authorization(&self.client, &self.auth, RESOURCE).await
						.context("could not get Management API authorization")?;
					Ok::<_, anyhow::Error>(log2::Secret(authorization))
				}).await?;

				*cached_management_authorization = Some(authorization.clone());
				Ok(authorization)
			},
		}
	}

	#[cfg(any(feature = "cdn", feature = "dns"))]
	fn management_request_parameters(&self, relative_url: std::fmt::Arguments<'_>) ->
		impl std::future::Future<Output = anyhow::Result<(String, hyper::header::HeaderValue)>> + '_
	{
		let url =
			format!(
				"https://management.azure.com/subscriptions/{}/resourceGroups/{}{}",
				self.subscription_id,
				self.resource_group_name,
				relative_url,
			);

		async move {
			let authorization = self.management_authorization().await?;

			Ok((url, authorization))
		}
	}
}

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]
async fn get_authorization(
	client: &http_common::Client,
	auth: &Auth,
	resource: &str,
) -> anyhow::Result<hyper::header::HeaderValue> {
	#[derive(serde::Deserialize)]
	struct Response {
		access_token: String,
		token_type: String,
	}

	impl http_common::FromResponse for Response {
		fn from_response(
			status: hyper::StatusCode,
			body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
			_headers: hyper::HeaderMap,
		) -> anyhow::Result<Option<Self>> {
			Ok(match (status, body) {
				(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
					Some(serde_json::from_reader(body)?),
				_ => None,
			})
		}
	}

	let req = match auth {
		Auth::ManagedIdentity { endpoint, secret } => {
			static SECRET: once_cell2::race::LazyBox<hyper::header::HeaderName> =
				once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("secret"));

			let mut req = hyper::Request::new(Default::default());
			*req.method_mut() = hyper::Method::GET;
			*req.uri_mut() =
				format!("{}?resource={}&api-version=2017-09-01", endpoint, resource)
				.parse().context("could not construct authorization request URI")?;
			req.headers_mut().insert(SECRET.clone(), secret.clone());
			req
		},

		#[cfg(debug_assertions)]
		Auth::ServicePrincipal { client_id, client_secret, tenant_id } => {
			let body =
				form_urlencoded::Serializer::new(String::new())
				.append_pair("grant_type", "client_credentials")
				.append_pair("client_id", client_id)
				.append_pair("client_secret", client_secret)
				.append_pair("resource", resource)
				.finish();
			let mut req = hyper::Request::new(body.into());
			*req.method_mut() = hyper::Method::POST;
			*req.uri_mut() =
				format!("https://login.microsoftonline.com/{}/oauth2/token", tenant_id)
				.parse().context("could not construct authorization request URI")?;
			req
		},
	};

	let Response { access_token, token_type } =
		client.request_inner(req).await.context("could not get authorization")?;

	let header_value = format!("{} {}", token_type, access_token);
	let header_value: hyper::header::HeaderValue = header_value.parse().context("could not parse token as HeaderValue")?;
	Ok(header_value)
}

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]
pub enum Auth {
	ManagedIdentity {
		endpoint: String,
		secret: hyper::header::HeaderValue,
	},

	#[cfg(debug_assertions)]
	ServicePrincipal {
		client_id: String,
		client_secret: String,
		tenant_id: String,
	},
}

#[cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]
impl Auth {
	pub fn from_env(
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		client_id: Option<String>,
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		client_secret: Option<String>,
		#[cfg_attr(not(debug_assertions), allow(unused_variables))]
		tenant_id: Option<String>,
	) -> anyhow::Result<Self> {
		if let (Ok(endpoint), Ok(secret)) = (std::env::var("MSI_ENDPOINT"), std::env::var("MSI_SECRET")) {
			let secret = std::convert::TryInto::try_into(&secret).context("could not parse MSI_SECRET as HeaderValue")?;
			return Ok(Auth::ManagedIdentity { endpoint, secret });
		}

		#[cfg(debug_assertions)]
		if let (Some(client_id), Some(client_secret), Some(tenant_id)) = (client_id, client_secret, tenant_id) {
			return Ok(Auth::ServicePrincipal { client_id, client_secret, tenant_id });
		}

		Err(anyhow::anyhow!("found neither MSI_ENDPOINT+MSI_SECRET nor client_id+client_secret+tenant_id"))
	}
}
