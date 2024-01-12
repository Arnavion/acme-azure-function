use anyhow::Context;

mod dns;

pub mod log_analytics;

pub struct Client<'a> {
	subscription_id: &'a str,
	resource_group_name: &'a str,
	auth: &'a crate::Auth,

	client: http_common::Client,
	cached_authorization: tokio::sync::OnceCell<http_common::HeaderValue>,
	logger: &'a log2::Logger,
}

impl<'a> Client<'a> {
	pub fn new(
		subscription_id: &'a str,
		resource_group_name: &'a str,
		auth: &'a crate::Auth,
		user_agent: http_common::HeaderValue,
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

	fn make_url(&self, path_and_query: std::fmt::Arguments<'_>) -> anyhow::Result<http_common::UriParts> {
		static AUTHORITY: once_cell::race::OnceBox<http_common::UriAuthority> = once_cell::race::OnceBox::new();

		let mut url: http_common::UriParts = Default::default();
		url.scheme = Some(http_common::UriScheme::HTTPS);
		url.authority = Some(AUTHORITY.get_or_init(|| Box::new(http_common::UriAuthority::from_static("management.azure.com"))).clone());
		url.path_and_query = Some(
			format!("/subscriptions/{}/resourceGroups/{}{path_and_query}", self.subscription_id, self.resource_group_name)
			.try_into().context("could not parse request URL")?,
		);
		Ok(url)
	}

	fn request_parameters(&self) -> (
		&crate::Auth,
		&http_common::Client,
		&tokio::sync::OnceCell<http_common::HeaderValue>,
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
