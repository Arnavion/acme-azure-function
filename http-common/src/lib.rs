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

pub static APPLICATION_JSON: once_cell2::race::LazyBox<http::HeaderValue> =
	once_cell2::race::LazyBox::new(|| http::HeaderValue::from_static("application/json"));

pub struct Client {
	inner: hyper::Client<hyper_rustls::HttpsConnector<hyper::client::connect::HttpConnector>, hyper::Body>,
	user_agent: http::HeaderValue,
}

impl Client {
	pub fn new(user_agent: http::HeaderValue) -> anyhow::Result<Self> {
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

		Ok(Client {
			inner,
			user_agent,
		})
	}

	pub async fn request<U, B, T>(
		&self,
		method: http::Method,
		url: U,
		authorization: http::HeaderValue,
		body: Option<B>,
	) -> anyhow::Result<T>
	where
		U: std::convert::TryInto<http::Uri>,
		Result<http::Uri, U::Error>: anyhow::Context<http::Uri, U::Error>,
		B: serde::Serialize,
		T: FromResponse,
	{
		let mut req =
			if let Some(body) = body {
				let mut req = hyper::Request::new(serde_json::to_vec(&body).context("could not serialize request body")?.into());
				req.headers_mut().insert(http::header::CONTENT_TYPE, APPLICATION_JSON.clone());
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
		*req.uri_mut() = url.try_into().context("could not parse request URL")?;
		req.headers_mut().insert(http::header::AUTHORIZATION, authorization);

		let value = self.request_inner(req).await?;
		Ok(value)
	}

	pub async fn request_inner<T>(&self, mut req: hyper::Request<hyper::Body>) -> anyhow::Result<T> where T: FromResponse {
		req.headers_mut().insert(http::header::USER_AGENT, self.user_agent.clone());

		let res = self.inner.request(req).await.context("could not execute request")?;

		let (http::response::Parts { status, mut headers, .. }, mut body) = res.into_parts();

		let mut body = match headers.remove(http::header::CONTENT_TYPE) {
			Some(content_type) => {
				let first = hyper::body::HttpBody::data(&mut body).await.transpose().context("could not read response body")?;
				let body =
					if let Some(first) = first {
						let second = hyper::body::HttpBody::data(&mut body).await.transpose().context("could not read response body")?;
						if let Some(second) = second {
							let rest = hyper::body::aggregate(body).await.context("could not read response body")?;
							let rest = hyper::body::Buf::reader(hyper::body::Buf::chain(second, rest));
							Body {
								first,
								rest: Some(rest),
							}
						}
						else {
							Body {
								first,
								rest: None,
							}
						}
					}
					else {
						Body {
							first: Default::default(),
							rest: None,
						}
					};
				Some((content_type, body))
			},
			None => None,
		};

		let err = match T::from_response(status, body.as_mut().map(|(content_type, body)| (&*content_type, body)), headers) {
			Ok(Some(value)) => return Ok(value),
			Ok(None) => None,
			Err(err) => Some(err),
		};

		let body = body.map(|(content_type, mut body)| {
			let mut body_vec = body.first.to_vec();
			if let Some(rest) = &mut body.rest {
				std::io::Read::read_to_end(rest, &mut body_vec).expect("cannot fail to read Buf to end");
			}
			(content_type, hyper::body::Bytes::from(body_vec))
		});

		match err {
			Some(err) => Err(err).context(format!("unexpected response {}: {:?}", status, body)),
			None => Err(anyhow::anyhow!("unexpected response {}: {:?}", status, body)),
		}
	}
}

pub trait FromResponse: Sized {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut Body<impl std::io::Read>)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>>;
}

pub struct Body<R> {
	first: hyper::body::Bytes,
	rest: Option<R>,
}

impl<R> Body<R> where R: std::io::Read {
	pub fn as_json<'de, T>(&'de mut self) -> serde_json::Result<T> where T: serde::Deserialize<'de> {
		let first = &self.first[..];
		match &mut self.rest {
			Some(rest) => serde::Deserialize::deserialize(&mut serde_json::Deserializer::from_reader(std::io::Read::chain(first, rest))),
			None => serde::Deserialize::deserialize(&mut serde_json::Deserializer::from_slice(first)),
		}
	}

	pub fn as_str(&mut self) -> anyhow::Result<std::borrow::Cow<'_, str>> {
		let first = &self.first[..];
		match &mut self.rest {
			Some(rest) => {
				let mut result = String::new();
				std::io::Read::read_to_string(&mut std::io::Read::chain(first, rest), &mut result)?;
				Ok(result.into())
			},
			None => Ok(std::str::from_utf8(first)?.into()),
		}
	}
}

pub struct ResponseWithLocation<T> {
	pub body: T,
	pub location: http::Uri,
}

impl<T> FromResponse for ResponseWithLocation<T> where T: FromResponse {
	fn from_response(
		status: http::StatusCode,
		body: Option<(&http::HeaderValue, &mut Body<impl std::io::Read>)>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
		let location = get_location(&headers)?;

		match T::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseWithLocation { body, location })),
			Ok(None) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

pub fn is_json(content_type: &http::HeaderValue) -> bool {
	content_type.to_str().map(|content_type| content_type == "application/json" || content_type.starts_with("application/json;")).unwrap_or_default()
}

pub fn get_location(headers: &http::HeaderMap) -> anyhow::Result<http::Uri> {
	let location =
		std::convert::TryInto::try_into(
			headers
			.get(http::header::LOCATION).context("missing location header")?
			.as_bytes(),
		).context("could not parse location header")?;
	Ok(location)
}

pub fn get_retry_after(
	headers: &http::HeaderMap,
	min: std::time::Duration,
	max: std::time::Duration,
) -> anyhow::Result<std::time::Duration> {
	let retry_after =
		if let Some(retry_after) = headers.get(http::header::RETRY_AFTER) {
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

pub fn deserialize_http_uri<'de, D>(deserializer: D) -> Result<http::Uri, D::Error> where D: serde::Deserializer<'de> {
	struct Visitor;

	impl serde::de::Visitor<'_> for Visitor {
		type Value = http::Uri;

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str("http::Uri")
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
pub struct DeserializableUri(#[serde(deserialize_with = "deserialize_http_uri")] pub http::Uri);
