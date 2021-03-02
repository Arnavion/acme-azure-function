#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_and_return,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::must_use_candidate,
	clippy::similar_names,
)]

use anyhow::Context;

pub static APPLICATION_JSON: once_cell2::race::LazyBox<hyper::header::HeaderValue> =
	once_cell2::race::LazyBox::new(|| hyper::header::HeaderValue::from_static("application/json"));

pub struct Client {
	inner: hyper::Client<hyper_rustls::HttpsConnector<hyper::client::connect::HttpConnector>, hyper::Body>,
	user_agent: hyper::header::HeaderValue,
}

impl Client {
	pub fn new(user_agent: &str) -> anyhow::Result<Self> {
		// Use this long form instead of just `hyper_rustls::HttpsConnector::with_webpki_roots()`
		// because otherwise it tries to initiate HTTP/2 connections with some hosts.
		//
		// Ref: https://github.com/ctz/hyper-rustls/issues/143
		let connector: hyper_rustls::HttpsConnector<_> = {
			let mut connector = hyper::client::connect::HttpConnector::new();
			connector.enforce_http(false);

			let mut config = rustls::ClientConfig::new();
			config.root_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
			config.alpn_protocols = vec![b"http/1.1".to_vec()];
			config.ct_logs = Some(&ct_logs::LOGS);

			(connector, config).into()
		};

		let inner = hyper::Client::builder().build(connector);

		let user_agent: hyper::header::HeaderValue = user_agent.parse().context("could not parse user_agent as HeaderValue")?;

		Ok(Client {
			inner,
			user_agent,
		})
	}

	pub async fn request<U, B, T>(
		&self,
		method: hyper::Method,
		url: U,
		authorization: hyper::header::HeaderValue,
		body: Option<B>,
	) -> anyhow::Result<T>
	where
		U: std::convert::TryInto<hyper::Uri>,
		Result<hyper::Uri, U::Error>: anyhow::Context<hyper::Uri, U::Error>,
		B: serde::Serialize,
		T: FromResponse,
	{
		let mut req =
			if let Some(body) = body {
				let mut req = hyper::Request::new(serde_json::to_vec(&body).context("could not serialize request body")?.into());
				req.headers_mut().insert(hyper::header::CONTENT_TYPE, APPLICATION_JSON.clone());
				req
			}
			else {
				hyper::Request::new(Default::default())
			};
		*req.method_mut() = method;
		*req.uri_mut() = url.try_into().context("could not parse request URL")?;
		req.headers_mut().insert(hyper::header::AUTHORIZATION, authorization);

		let value = self.request_inner(req).await?;
		Ok(value)
	}

	pub async fn request_inner<T>(&self, mut req: hyper::Request<hyper::Body>) -> anyhow::Result<T> where T: FromResponse {
		req.headers_mut().insert(hyper::header::USER_AGENT, self.user_agent.clone());

		let res = self.inner.request(req).await.context("could not execute request")?;

		let (http::response::Parts { status, mut headers, .. }, body) = res.into_parts();

		let mut body = match headers.remove(hyper::header::CONTENT_TYPE) {
			Some(content_type) => {
				let body = hyper::body::aggregate(body).await.context("could not read response body")?;
				let body = hyper::body::Buf::reader(body);
				Some((content_type, body))
			},
			None => None,
		};

		let err = match T::from_response(status, body.as_mut().map(|(content_type, body)| (&*content_type, body)), headers) {
			Ok(Some(value)) => return Ok(value),
			Ok(None) => None,
			Err(err) => Some(err),
		};

		let remaining = body.map(|(content_type, mut body)| {
			let mut remaining = vec![];
			let _ = std::io::Read::read_to_end(&mut body, &mut remaining).expect("cannot fail to read Buf to end");
			(content_type, hyper::body::Bytes::from(remaining))
		});

		match err {
			Some(err) => Err(err).context(format!("unexpected response {}: {:?}", status, remaining)),
			None => Err(anyhow::anyhow!("unexpected response {}: {:?}", status, remaining)),
		}
	}
}

pub trait FromResponse: Sized {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
		headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>>;
}

pub struct ResponseWithLocation<T> {
	pub body: T,
	pub location: hyper::Uri,
}

impl<T> FromResponse for ResponseWithLocation<T> where T: FromResponse {
	fn from_response(
		status: hyper::StatusCode,
		body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
		headers: hyper::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		let location = get_location(&headers)?;

		match T::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseWithLocation { body, location })),
			Ok(None) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

pub fn is_json(content_type: &hyper::header::HeaderValue) -> bool {
	let content_type = match content_type.to_str() {
		Ok(content_type) => content_type,
		Err(_) => return false,
	};
	content_type == "application/json" || content_type.starts_with("application/json;")
}

pub fn get_location(headers: &hyper::HeaderMap) -> anyhow::Result<hyper::Uri> {
	let location =
		headers
		.get(hyper::header::LOCATION).context("missing location header")?
		.to_str().context("could not parse location header")?
		.parse().context("could not parse location header")?;
	Ok(location)
}

pub fn get_retry_after(
	headers: &hyper::HeaderMap,
	min: std::time::Duration,
	max: std::time::Duration,
) -> anyhow::Result<std::time::Duration> {
	let retry_after =
		if let Some(retry_after) = headers.get(hyper::header::RETRY_AFTER) {
			retry_after
		}
		else {
			return Ok(min);
		};

	let retry_after = retry_after.to_str().context("could not parse retry-after header")?;

	// Ref:
	//
	// - https://tools.ietf.org/html/rfc7231#section-7.1.3
	// - https://tools.ietf.org/html/rfc7231#section-7.1.1.1

	let retry_after =
		if let Ok(secs) = retry_after.parse() {
			std::time::Duration::from_secs(secs)
		}
		else if let Ok(date) = chrono::NaiveDateTime::parse_from_str(retry_after, "%a, %d %b %Y %T GMT") {
			let date = chrono::DateTime::from_utc(date, chrono::Utc);
			let diff = date - chrono::Utc::now();
			let diff = diff.to_std().context("could not parse retry-after header as HTTP-date")?;
			diff
		}
		else {
			return Err(anyhow::anyhow!("could not parse retry-after header as delay-seconds or HTTP-date"));
		};

	Ok(retry_after.clamp(min, max))
}

pub fn deserialize_hyper_uri<'de, D>(deserializer: D) -> Result<hyper::Uri, D::Error> where D: serde::Deserializer<'de> {
	struct Visitor;

	impl serde::de::Visitor<'_> for Visitor {
		type Value = hyper::Uri;

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str("hyper::Uri")
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			std::convert::TryInto::try_into(s).map_err(serde::de::Error::custom)
		}

		fn visit_string<E>(self, s: String) -> Result<Self::Value, E> where E: serde::de::Error {
			std::convert::TryInto::try_into(s).map_err(serde::de::Error::custom)
		}
	}

	deserializer.deserialize_string(Visitor)
}

#[derive(Debug, serde::Deserialize)]
pub struct DeserializableUri(#[serde(deserialize_with = "deserialize_hyper_uri")] pub hyper::Uri);
