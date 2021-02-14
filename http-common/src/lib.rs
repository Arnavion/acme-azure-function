#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::missing_errors_doc,
	clippy::must_use_candidate,
	clippy::similar_names,
)]

use anyhow::Context;

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

	pub async fn request<T, B>(
		&self,
		method: hyper::Method,
		url: &str,
		authorization: hyper::header::HeaderValue,
		body: Option<&B>,
	) -> anyhow::Result<(T, hyper::header::HeaderMap)>
	where
		T: FromResponse,
		B: serde::Serialize,
	{
		let mut req =
			if let Some(body) = body {
				let mut req = hyper::Request::new(serde_json::to_vec(body).context("could not serialize request body")?.into());
				req.headers_mut().insert(hyper::header::CONTENT_TYPE, APPLICATION_JSON.clone());
				req
			}
			else {
				hyper::Request::new(Default::default())
			};
		*req.method_mut() = method;
		*req.uri_mut() = url.parse().with_context(|| format!("could not parse request URL {:?}", url))?;
		req.headers_mut().insert(hyper::header::AUTHORIZATION, authorization);

		let value = self.request_inner(req).await?;
		Ok(value)
	}

	pub async fn request_inner<T>(&self, mut req: hyper::Request<hyper::Body>) -> anyhow::Result<(T, hyper::header::HeaderMap)> where T: FromResponse {
		req.headers_mut().insert(hyper::header::USER_AGENT, self.user_agent.clone());

		let res = self.inner.request(req).await.context("could not execute request")?;

		let (http::response::Parts { status, headers, .. }, body) = res.into_parts();

		let mut body = match headers.get(hyper::header::CONTENT_TYPE) {
			Some(content_type) => {
				let body = hyper::body::aggregate(body).await.context("could not read response body")?;
				let body = hyper::body::Buf::reader(body);
				Some((content_type, body))
			},
			None => None,
		};

		let err = match T::from_response(status, body.as_mut().map(|(content_type, body)| (*content_type, body))) {
			Ok(Some(value)) => return Ok((value, headers)),
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
	) -> anyhow::Result<Option<Self>>;
}

impl FromResponse for () {
	fn from_response(
		status: hyper::StatusCode,
		_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
	) -> anyhow::Result<Option<Self>> {
		Ok(match status {
			hyper::StatusCode::OK => Some(()),
			_ => None,
		})
	}
}

pub fn is_json(content_type: &hyper::header::HeaderValue) -> bool {
	let content_type = match content_type.to_str() {
		Ok(content_type) => content_type,
		Err(_) => return false,
	};
	content_type == "application/json" || content_type.starts_with("application/json;")
}

pub static APPLICATION_JSON: once_cell::sync::Lazy<hyper::header::HeaderValue> =
	once_cell::sync::Lazy::new(|| hyper::header::HeaderValue::from_static("application/json"));

pub fn jws_base64_encode(s: &[u8]) -> String {
	let config = base64::Config::new(base64::CharacterSet::UrlSafe, false);
	base64::encode_config(s, config)
}
