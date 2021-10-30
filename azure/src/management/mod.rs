use anyhow::Context;

mod dns;

pub mod log_analytics;

pub struct Client<'a> {
	subscription_id: &'a str,
	resource_group_name: &'a str,
	auth: &'a crate::Auth,

	client: http_common::Client,
	cached_authorization: tokio::sync::RwLock<Option<http::HeaderValue>>,
	logger: &'a log2::Logger,
}

impl<'a> Client<'a> {
	pub fn new(
		subscription_id: &'a str,
		resource_group_name: &'a str,
		auth: &'a crate::Auth,
		user_agent: http::HeaderValue,
		logger: &'a log2::Logger,
	) -> anyhow::Result<Self> {
		Ok(Client {
			subscription_id,
			resource_group_name,
			auth,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			cached_authorization: Default::default(),
			logger,
		})
	}
}

impl crate::Client for Client<'_> {
	const AUTH_RESOURCE: &'static str = "https://management.azure.com";

	fn make_url(&self, path_and_query: std::fmt::Arguments<'_>) -> anyhow::Result<http::uri::Parts> {
		static AUTHORITY: once_cell2::race::LazyBox<http::uri::Authority> =
			once_cell2::race::LazyBox::new(|| http::uri::Authority::from_static("management.azure.com"));

		let mut url: http::uri::Parts = Default::default();
		url.scheme = Some(http::uri::Scheme::HTTPS);
		url.authority = Some(AUTHORITY.clone());
		url.path_and_query = Some(
			format!("/subscriptions/{}/resourceGroups/{}{}", self.subscription_id, self.resource_group_name, path_and_query)
			.try_into().context("could not parse request URL")?,
		);
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
