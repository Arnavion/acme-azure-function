use anyhow::Context;

pub use http::uri::{
	Authority as UriAuthority,
	Parts as UriParts,
	Scheme as UriScheme,
};

pub use hyper::{
	Method,
	Request,
	StatusCode,
	Uri,
	body::Bytes,
	header::{
		AUTHORIZATION,
		CONTENT_LENGTH,
		CONTENT_TYPE,
		HeaderMap,
		HeaderName,
		HeaderValue,
	},
};

pub struct Client {
	inner: hyper_util::client::legacy::Client<
		hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
		RequestBody,
	>,
	user_agent: HeaderValue,
}

impl Client {
	pub fn new(user_agent: HeaderValue) -> anyhow::Result<Self> {
		let connector =
			hyper_rustls::HttpsConnectorBuilder::new()
			.with_webpki_roots()
			.https_or_http()
			.enable_http1()
			.build();

		let inner = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector);

		Ok(Client {
			inner,
			user_agent,
		})
	}

	pub async fn request<T>(&self, req: Request<RequestBody>) -> anyhow::Result<T> where T: FromResponse {
		// This fn encapsulates the non-generic parts of `request` to reduce code size from monomorphization.
		async fn request_inner(client: &Client, mut req: Request<RequestBody>) ->
			anyhow::Result<(
				StatusCode,
				HeaderMap,
				Option<ResponseBody<impl std::io::Read>>,
			)>
		{
			req.headers_mut().insert(hyper::header::USER_AGENT, client.user_agent.clone());

			let res = client.inner.request(req).await.context("could not execute request")?;

			let (http::response::Parts { status, mut headers, .. }, mut body) = res.into_parts();

			let body = match headers.remove(CONTENT_TYPE) {
				Some(content_type) => {
					let first = http_body_util::BodyExt::frame(&mut body).await.transpose().context("could not read response body")?;
					let body =
						if let Some(first) = first {
							let first = first.into_data().map_err(|_| anyhow::anyhow!("could not read response body: not a data frame"))?;

							let second = http_body_util::BodyExt::frame(&mut body).await.transpose().context("could not read response body")?;
							if let Some(second) = second {
								let second = second.into_data().map_err(|_| anyhow::anyhow!("could not read response body: not a data frame"))?;

								let rest = http_body_util::BodyExt::collect(body).await.context("could not read response body")?.aggregate();
								let rest = hyper::body::Buf::reader(hyper::body::Buf::chain(second, rest));

								ResponseBody {
									content_type,
									first,
									rest: Some(rest),
								}
							}
							else {
								ResponseBody {
									content_type,
									first,
									rest: None,
								}
							}
						}
						else {
							ResponseBody {
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
			(body.content_type, Bytes::from(body_vec))
		});

		match err {
			Some(err) => Err(err).context(format!("unexpected response {status}: {body:?}")),
			None => Err(anyhow::anyhow!("unexpected response {status}: {body:?}")),
		}
	}
}

pub struct RequestBody(RequestBodyInner);

enum RequestBodyInner {
	Single(http_body_util::Full<Bytes>),
	Boxed(http_body_util::combinators::UnsyncBoxBody<Bytes, std::convert::Infallible>),
}

impl RequestBody {
	// clippy wants this to impl FromIterator but this needs an extra `T::IntoIter: Send + 'static` bound.
	#[allow(clippy::should_implement_trait)]
	pub fn from_iter<T>(iter: T) -> Self
	where
		T: IntoIterator<Item = Bytes>,
		T::IntoIter: Send + 'static,
	{
		let iter = iter.into_iter().map(|buf| Ok(hyper::body::Frame::data(buf)));
		let stream = futures_util::stream::iter(iter);
		let body = http_body_util::StreamBody::new(stream);
		let body = http_body_util::combinators::UnsyncBoxBody::new(body);
		Self(RequestBodyInner::Boxed(body))
	}
}

impl Default for RequestBody {
	fn default() -> Self {
		Self(RequestBodyInner::Single(Default::default()))
	}
}

impl<T> From<T> for RequestBody where T: Into<Bytes> {
	fn from(buf: T) -> Self {
		let buf: Bytes = buf.into();
		Self(RequestBodyInner::Single(buf.into()))
	}
}

impl hyper::body::Body for RequestBody {
	type Data = Bytes;
	type Error = std::convert::Infallible;

	fn poll_frame(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
		match &mut self.0 {
			RequestBodyInner::Single(inner) => std::pin::Pin::new(inner).poll_frame(cx),
			RequestBodyInner::Boxed(inner) => std::pin::Pin::new(inner).poll_frame(cx),
		}
	}

	fn is_end_stream(&self) -> bool {
		match &self.0 {
			RequestBodyInner::Single(inner) => inner.is_end_stream(),
			RequestBodyInner::Boxed(inner) => inner.is_end_stream(),
		}
	}

	fn size_hint(&self) -> hyper::body::SizeHint {
		match &self.0 {
			RequestBodyInner::Single(inner) => inner.size_hint(),
			RequestBodyInner::Boxed(inner) => inner.size_hint(),
		}
	}
}

pub trait FromResponse: Sized {
	fn from_response(
		status: StatusCode,
		body: Option<&mut ResponseBody<impl std::io::Read>>,
		headers: HeaderMap,
	) -> anyhow::Result<Option<Self>>;
}

pub struct ResponseBody<R> {
	content_type: HeaderValue,
	first: Bytes,
	rest: Option<R>,
}

impl<R> ResponseBody<R> where R: std::io::Read {
	pub fn as_json<'de, T>(&'de mut self) -> anyhow::Result<T> where T: serde::Deserialize<'de> {
		let is_json =
			self.content_type.to_str()
			.is_ok_and(|content_type| content_type == "application/json" || content_type.starts_with("application/json;"));
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
			.is_ok_and(|content_type| content_type == expected_content_type);
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
	pub location: Uri,
}

impl<T> FromResponse for ResponseWithLocation<T> where T: FromResponse {
	fn from_response(
		status: StatusCode,
		body: Option<&mut ResponseBody<impl std::io::Read>>,
		headers: HeaderMap,
	) -> anyhow::Result<Option<Self>> {
	let location =
		headers
		.get(hyper::header::LOCATION).context("missing location header")?
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
	headers: &HeaderMap,
	min: std::time::Duration,
	max: std::time::Duration,
) -> anyhow::Result<std::time::Duration> {
	let Some(retry_after) = headers.get(hyper::header::RETRY_AFTER) else { return Ok(min); };

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
			let diff = date - time::OffsetDateTime::now_utc();
			diff.try_into().context("could not parse retry-after header as HTTP-date")?
		}
		else {
			return Err(anyhow::anyhow!("could not parse retry-after header as delay-seconds or HTTP-date"));
		};

	Ok(retry_after.clamp(min, max))
}

pub struct DeserializableUri(pub Uri);

impl std::fmt::Debug for DeserializableUri {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

impl<'de> serde::Deserialize<'de> for DeserializableUri {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
		struct Visitor;

		impl serde::de::Visitor<'_> for Visitor {
			type Value = Uri;

			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("Uri")
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
