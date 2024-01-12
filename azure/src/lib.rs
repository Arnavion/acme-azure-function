use anyhow::Context;

mod auth;
pub use auth::Auth;

pub mod key_vault;

pub mod management;

#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http_common::HeaderValue
const APPLICATION_JSON: http_common::HeaderValue = http_common::HeaderValue::from_static("application/json");

trait Client {
	const AUTH_RESOURCE: &'static str;

	fn make_url(&self, path_and_query: std::fmt::Arguments<'_>) -> anyhow::Result<http_common::UriParts>;

	fn request_parameters(&self) -> (
		&Auth,
		&http_common::Client,
		&tokio::sync::OnceCell<http_common::HeaderValue>,
		&log2::Logger,
	);
}

enum Url<'a> {
	PathAndQuery(std::fmt::Arguments<'a>),
	Uri(http_common::Uri),
}

impl<'a> From<std::fmt::Arguments<'a>> for Url<'a> {
	fn from(path_and_query: std::fmt::Arguments<'a>) -> Self {
		Url::PathAndQuery(path_and_query)
	}
}

impl From<http_common::Uri> for Url<'_> {
	fn from(uri: http_common::Uri) -> Self {
		Url::Uri(uri)
	}
}

fn request<'client, 'url, TClient, TUrl, TBody, TResponse>(
	client: &'client TClient,
	method: http_common::Method,
	url: TUrl,
	body: Option<TBody>,
) -> impl std::future::Future<Output = anyhow::Result<TResponse>> + 'client
where
	TClient: Client,
	TUrl: Into<Url<'url>>,
	TBody: serde::Serialize,
	TResponse: http_common::FromResponse,
{
	// This fn encapsulates the non-generic parts of `request` to reduce code size from monomorphization.
	fn make_request(
		method: http_common::Method,
		url: anyhow::Result<http_common::Uri>,
		authorization: anyhow::Result<&http_common::HeaderValue>,
		body: Option<serde_json::Result<Vec<u8>>>,
	) -> anyhow::Result<http_common::Request<http_common::RequestBody>> {
		let url = url?;
		let authorization = authorization.context("could not get API authorization")?.clone();

		let mut req =
			if let Some(body) = body {
				let mut req = http_common::Request::new(body.context("could not serialize request body")?.into());
				req.headers_mut().insert(http_common::CONTENT_TYPE, APPLICATION_JSON);
				req
			}
			else {
				let mut req = http_common::Request::new(Default::default());
				if method != http_common::Method::GET {
					req.headers_mut().insert(http_common::CONTENT_LENGTH, 0.into());
				}
				req
			};

		*req.method_mut() = method;
		*req.uri_mut() = url;

		req.headers_mut().insert(http_common::AUTHORIZATION, authorization);

		Ok(req)
	}

	let url = match url.into() {
		Url::PathAndQuery(path_and_query) =>
			client.make_url(path_and_query)
			.and_then(|uri| uri.try_into().context("could not parse request URL")),

		Url::Uri(uri) => Ok(uri),
	};

	let body = body.map(|body| serde_json::to_vec(&body));

	async move {
		let (auth, client, cached_authorization, logger) = client.request_parameters();
		let authorization = cached_authorization.get_or_try_init(|| auth.get_authorization(client, TClient::AUTH_RESOURCE, logger)).await;
		let req = make_request(method, url, authorization, body)?;
		let value = client.request(req).await?;
		Ok(value)
	}
}
