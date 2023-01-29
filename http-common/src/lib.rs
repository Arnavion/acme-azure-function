#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_and_return,
	clippy::missing_errors_doc,
	clippy::similar_names,
)]

use anyhow::Context;

pub struct Client {
	inner: hyper::Client<hyper_rustls::HttpsConnector<hyper::client::connect::HttpConnector>, hyper::Body>,
	user_agent: http::HeaderValue,
}

impl Client {
	pub fn new(user_agent: http::HeaderValue) -> anyhow::Result<Self> {
		let connector =
			hyper_rustls::HttpsConnectorBuilder::new()
			.with_webpki_roots()
			.https_or_http()
			.enable_http1()
			.build();

		let inner = hyper::Client::builder().build(connector);

		Ok(Client {
			inner,
			user_agent,
		})
	}

	pub async fn request<T>(&self, req: hyper::Request<hyper::Body>) -> anyhow::Result<T> where T: FromResponse {
		// This fn encapsulates the non-generic parts of `request` to reduce code size from monomorphization.
		async fn request_inner(client: &Client, mut req: hyper::Request<hyper::Body>) ->
			anyhow::Result<(
				http::StatusCode,
				http::HeaderMap,
				Option<Body<impl std::io::Read>>,
			)>
		{
			req.headers_mut().insert(http::header::USER_AGENT, client.user_agent.clone());

			let res = client.inner.request(req).await.context("could not execute request")?;

			let (http::response::Parts { status, mut headers, .. }, mut body) = res.into_parts();

			let body = match headers.remove(http::header::CONTENT_TYPE) {
				Some(content_type) => {
					let first = hyper::body::HttpBody::data(&mut body).await.transpose().context("could not read response body")?;
					let body =
						if let Some(first) = first {
							let second = hyper::body::HttpBody::data(&mut body).await.transpose().context("could not read response body")?;
							if let Some(second) = second {
								let rest = hyper::body::aggregate(body).await.context("could not read response body")?;
								let rest = hyper::body::Buf::reader(hyper::body::Buf::chain(second, rest));
								Body {
									content_type,
									first,
									rest: Some(rest),
								}
							}
							else {
								Body {
									content_type,
									first,
									rest: None,
								}
							}
						}
						else {
							Body {
								content_type,
								first: Default::default(),
								rest: None,
							}
						};
					Some(body)
				},

				None => None,
			};

			Ok((status, headers, body))
		}

		let (status, headers, mut body) = request_inner(self, req).await?;

		let err = match T::from_response(status, body.as_mut(), headers) {
			Ok(Some(value)) => return Ok(value),
			Ok(None) => None,
			Err(err) => Some(err),
		};

		let body = body.map(|mut body| {
			let mut body_vec = body.first.to_vec();
			if let Some(rest) = &mut body.rest {
				std::io::Read::read_to_end(rest, &mut body_vec).expect("cannot fail to read Buf to end");
			}
			(body.content_type, hyper::body::Bytes::from(body_vec))
		});

		match err {
			Some(err) => Err(err).context(format!("unexpected response {status}: {body:?}")),
			None => Err(anyhow::anyhow!("unexpected response {status}: {body:?}")),
		}
	}
}

pub trait FromResponse: Sized {
	fn from_response(
		status: http::StatusCode,
		body: Option<&mut Body<impl std::io::Read>>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>>;
}

pub struct Body<R> {
	content_type: http::HeaderValue,
	first: hyper::body::Bytes,
	rest: Option<R>,
}

impl<R> Body<R> where R: std::io::Read {
	pub fn as_json<'de, T>(&'de mut self) -> anyhow::Result<T> where T: serde::Deserialize<'de> {
		let is_json =
			self.content_type.to_str()
			.map(|content_type| content_type == "application/json" || content_type.starts_with("application/json;"))
			.unwrap_or_default();
		if !is_json {
			return Err(anyhow::anyhow!("response body does not have content-type:application/json"));
		}

		let first = &self.first[..];
		Ok(match &mut self.rest {
			Some(rest) => serde::Deserialize::deserialize(&mut serde_json::Deserializer::from_reader(std::io::Read::chain(first, rest)))?,
			None => serde::Deserialize::deserialize(&mut serde_json::Deserializer::from_slice(first))?,
		})
	}

	pub fn as_str(&mut self, expected_content_type: &str) -> anyhow::Result<std::borrow::Cow<'_, str>> {
		let content_type_matches =
			self.content_type.to_str()
			.map(|content_type| content_type == expected_content_type)
			.unwrap_or_default();
		if !content_type_matches {
			return Err(anyhow::anyhow!("response body does not have content-type:{expected_content_type}"));
		}

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
		body: Option<&mut Body<impl std::io::Read>>,
		headers: http::HeaderMap,
	) -> anyhow::Result<Option<Self>> {
	let location =
		headers
		.get(http::header::LOCATION).context("missing location header")?
		.as_bytes()
		.try_into().context("could not parse location header")?;

		match T::from_response(status, body, headers) {
			Ok(Some(body)) => Ok(Some(ResponseWithLocation { body, location })),
			Ok(None) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

pub fn get_retry_after(
	headers: &http::HeaderMap,
	min: std::time::Duration,
	max: std::time::Duration,
) -> anyhow::Result<std::time::Duration> {
	let Some(retry_after) = headers.get(http::header::RETRY_AFTER) else { return Ok(min); };

	let retry_after = retry_after.to_str().context("could not parse retry-after header")?;

	// Ref:
	//
	// - https://tools.ietf.org/html/rfc7231#section-7.1.3
	// - https://tools.ietf.org/html/rfc7231#section-7.1.1.1

	let retry_after =
		if let Ok(secs) = retry_after.parse() {
			std::time::Duration::from_secs(secs)
		}
		else if let Ok(date) = httpdate::parse_http_date(retry_after) {
			let date: chrono::DateTime<chrono::Utc> = date.into();
			let diff = date - chrono::Utc::now();
			let diff = diff.to_std().context("could not parse retry-after header as HTTP-date")?;
			diff
		}
		else {
			return Err(anyhow::anyhow!("could not parse retry-after header as delay-seconds or HTTP-date"));
		};

	Ok(retry_after.clamp(min, max))
}

pub struct DeserializableUri(pub http::Uri);

impl std::fmt::Debug for DeserializableUri {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

impl<'de> serde::Deserialize<'de> for DeserializableUri {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
		struct Visitor;

		impl serde::de::Visitor<'_> for Visitor {
			type Value = http::Uri;

			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("http::Uri")
			}

			fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
				s.try_into().map_err(serde::de::Error::custom)
			}

			fn visit_string<E>(self, s: String) -> Result<Self::Value, E> where E: serde::de::Error {
				s.try_into().map_err(serde::de::Error::custom)
			}
		}

		Ok(DeserializableUri(deserializer.deserialize_string(Visitor)?))
	}
}
