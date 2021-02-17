#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
)]

#![cfg(any(
	feature = "cdn",
	feature = "dns",
	feature = "key_vault_cert",
	feature = "key_vault_key",
))]

#[cfg(feature = "cdn")]
mod cdn;
#[cfg(feature = "cdn")]
pub use cdn::CustomDomainSecret as CdnCustomDomainSecret;

#[cfg(feature = "dns")]
mod dns;

#[cfg(any(feature = "key_vault_cert", feature = "key_vault_key"))]
mod key_vault;
#[cfg(feature = "key_vault_cert")]
pub use key_vault::Certificate as KeyVaultCertificate;
#[cfg(feature = "key_vault_key")]
pub use key_vault::Key as KeyVaultKey;

use anyhow::Context;

pub struct Account<'a> {
	subscription_id: &'a str,
	resource_group_name: &'a str,
	auth: &'a Auth<'a>,

	client: http_common::Client,
	cached_management_authorization: Option<hyper::header::HeaderValue>,
	cached_key_vault_authorization: Option<hyper::header::HeaderValue>,
}

impl<'a> Account<'a> {
	pub fn new(
		subscription_id: &'a str,
		resource_group_name: &'a str,
		auth: &'a Auth<'a>,
		user_agent: &str,
	) -> anyhow::Result<Self> {
		Ok(Account {
			subscription_id,
			resource_group_name,
			auth,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			cached_management_authorization: None,
			cached_key_vault_authorization: None,
		})
	}

	#[cfg(any(feature = "cdn", feature = "dns"))]
	async fn management_request_parameters(&mut self, relative_url: &str) -> anyhow::Result<(String, hyper::header::HeaderValue)> {
		let url =
			format!(
				"https://management.azure.com/subscriptions/{}/resourceGroups/{}{}",
				self.subscription_id,
				self.resource_group_name,
				relative_url,
			);

		let authorization = match &mut self.cached_management_authorization {
			Some(authorization) => authorization.clone(),

			None => {
				eprintln!("Getting Management API authorization...");
				let authorization =
					get_authorization(
						&self.client,
						&self.auth,
						"https://management.azure.com",
					).await.context("could not get Management API authorization")?;
				eprintln!("Got Management API authorization");
				self.cached_management_authorization = Some(authorization.clone());
				authorization
			},
		};

		Ok((url, authorization))
	}
}

async fn get_authorization(
	client: &http_common::Client,
	auth: &Auth<'_>,
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
			let mut req = hyper::Request::new(Default::default());
			*req.method_mut() = hyper::Method::GET;
			*req.uri_mut() =
				format!("{}?resource={}&api-version=2017-09-01", endpoint, resource)
				.parse().context("could not construct authorization request URI")?;
			req.headers_mut().insert("secret", secret.clone());
			req
		},

		Auth::ServicePrincipal { client_id, client_secret, tenant_id } => {
			let body = {
				let mut body = form_urlencoded::Serializer::new(String::new());
				body.append_pair("grant_type", "client_credentials");
				body.append_pair("client_id", client_id);
				body.append_pair("client_secret", client_secret);
				body.append_pair("resource", resource);
				body.finish()
			};
			let mut req = hyper::Request::new(body.into_bytes().into());
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

pub enum Auth<'a> {
	ManagedIdentity {
		endpoint: String,
		secret: hyper::header::HeaderValue,
	},

	ServicePrincipal {
		client_id: &'a str,
		client_secret: &'a str,
		tenant_id: &'a str,
	},
}

impl<'a> Auth<'a> {
	pub fn from_env(
		client_id: Option<&'a str>,
		client_secret: Option<&'a str>,
		tenant_id: Option<&'a str>,
	) -> anyhow::Result<Self> {
		if let (Ok(endpoint), Ok(secret)) = (std::env::var("MSI_ENDPOINT"), std::env::var("MSI_SECRET")) {
			let secret = std::convert::TryInto::try_into(&secret).context("could not parse MSI_SECRET as HeaderValue")?;
			Ok(Auth::ManagedIdentity { endpoint, secret })
		}
		else if let (Some(client_id), Some(client_secret), Some(tenant_id)) = (client_id, client_secret, tenant_id) {
			Ok(Auth::ServicePrincipal { client_id, client_secret, tenant_id })
		}
		else {
			Err(anyhow::anyhow!("found neither MSI_ENDPOINT+MSI_SECRET nor client_id+client_secret+tenant_id"))
		}
	}
}
