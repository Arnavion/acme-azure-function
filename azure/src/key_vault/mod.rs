mod certificate;
pub use certificate::{Certificate, CreateCsrKeyType};

mod key;
pub use key::{EcKty, Key};

use anyhow::Context;

pub struct Client<'a> {
	key_vault_name: &'a str,
	auth: &'a crate::Auth,

	client: http_common::Client,
	authority: http::uri::Authority,
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
			authority: {
				// http::uri::Authority does not impl TryFrom<String>, only TryFrom<&[u8]> and TryFrom<&str> which copy.
				// So use http::uri::Authority::from_maybe_shared with a manually-constructed bytes::Bytes instead.
				//
				// Ref: https://github.com/hyperium/http/pull/477

				let authority: bytes::Bytes = format!("{}.vault.azure.net", key_vault_name).into();
				http::uri::Authority::from_maybe_shared(authority).context("could not construct URL authority")?
			},
			cached_authorization: Default::default(),
			logger,
		})
	}
}

impl crate::Client for Client<'_> {
	const AUTH_RESOURCE: &'static str = "https://vault.azure.net";

	fn make_url(&self, path_and_query: std::fmt::Arguments<'_>) -> anyhow::Result<http::uri::Parts> {
		let mut url: http::uri::Parts = Default::default();
		url.scheme = Some(http::uri::Scheme::HTTPS);
		url.authority = Some(self.authority.clone());
		url.path_and_query = Some({
			// http::uri::PathAndQuery implements TryFrom<String> by copying instead of reusing the String.
			// So use http::uri::PathAndQuery::from_maybe_shared with a manually-constructed bytes::Bytes instead.
			//
			// Ref: https://github.com/hyperium/http/pull/477

			let path_and_query: bytes::Bytes = path_and_query.to_string().into();
			http::uri::PathAndQuery::from_maybe_shared(path_and_query).context("could not parse request URL")?
		});
		Ok(url)
	}

	fn request_parameters(&self) -> (
		&crate::Auth,
		&http_common::Client,
		&tokio::sync::RwLock<Option<http::HeaderValue>>,
		&log2::Logger,
	) {
		(
			self.auth,
			&self.client,
			&self.cached_authorization,
			self.logger,
		)
	}
}
