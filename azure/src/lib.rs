#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_and_return,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::similar_names,
	clippy::too_many_lines,
)]

use anyhow::Context;

mod auth;
pub use auth::Auth;

pub mod key_vault;

pub mod management;

#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderValue
const APPLICATION_JSON: http::HeaderValue = http::HeaderValue::from_static("application/json");

trait Client {
	const AUTH_RESOURCE: &'static str;

	fn make_url(&self, path_and_query: std::fmt::Arguments<'_>) -> anyhow::Result<http::uri::Parts>;

	fn request_parameters(&self) -> (
		&Auth,
		&http_common::Client,
		&tokio::sync::OnceCell<http::HeaderValue>,
		&log2::Logger,
	);
}

enum Url<'a> {
	PathAndQuery(std::fmt::Arguments<'a>),
	Uri(http::Uri),
}

impl<'a> From<std::fmt::Arguments<'a>> for Url<'a> {
	fn from(path_and_query: std::fmt::Arguments<'a>) -> Self {
		Url::PathAndQuery(path_and_query)
	}
}

impl From<http::Uri> for Url<'_> {
	fn from(uri: http::Uri) -> Self {
		Url::Uri(uri)
	}
}

fn request<'client, 'url, TClient, TUrl, TBody, TResponse>(
	client: &'client TClient,
	method: http::Method,
	url: TUrl,
	body: Option<TBody>,
) -> impl std::future::Future<Output = anyhow::Result<TResponse>> + 'client
where
	TClient: Client,
	TUrl: Into<Url<'url>>,
	TBody: serde::Serialize + 'client,
	TResponse: http_common::FromResponse,
{
	let url = match url.into() {
		Url::PathAndQuery(path_and_query) =>
			client.make_url(path_and_query)
			.and_then(|uri| uri.try_into().context("could not parse request URL")),

		Url::Uri(uri) => Ok(uri),
	};

	async move {
		let (auth, client, cached_authorization, logger) = client.request_parameters();
		let url = url?;

		let authorization =
			cached_authorization.get_or_try_init(|| async {
				let authorization =
					auth.get_authorization(client, TClient::AUTH_RESOURCE, logger).await
					.context("could not get API authorization")?;
				Ok::<_, anyhow::Error>(authorization)
			}).await?.clone();

		let mut req =
			if let Some(body) = body {
				let mut req = hyper::Request::new(serde_json::to_vec(&body).context("could not serialize request body")?.into());
				req.headers_mut().insert(http::header::CONTENT_TYPE, APPLICATION_JSON);
				req
			}
			else if method == http::Method::GET {
				hyper::Request::new(Default::default())
			}
			else {
				let mut req = hyper::Request::new(serde_json::to_vec(&body).context("could not serialize request body")?.into());
				req.headers_mut().insert(http::header::CONTENT_LENGTH, 0.into());
				req
			};
		*req.method_mut() = method;
		*req.uri_mut() = url;
		req.headers_mut().insert(http::header::AUTHORIZATION, authorization);

		let value = client.request(req).await?;
		Ok(value)
	}
}
