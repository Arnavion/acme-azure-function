use anyhow::Context;

pub struct LogSender {
	customer_id: String,
	signer: hmac::Hmac<sha2::Sha256>,

	client: http_common::Client,
	uri: hyper::Uri,
	authorization_prefix: String,
}

impl LogSender {
	pub fn new(customer_id: String, signer: hmac::Hmac<sha2::Sha256>, user_agent: hyper::header::HeaderValue) -> anyhow::Result<Self> {
		let uri =
			std::convert::TryInto::try_into(format!("https://{}.ods.opinsights.azure.com/api/logs?api-version=2016-04-01", customer_id))
			.context("could not construct Log Analytics Data Collector API URI")?;

		let authorization_prefix = format!("SharedKey {}:", customer_id);

		Ok(LogSender {
			customer_id,
			signer,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			uri,
			authorization_prefix,
		})
	}

	pub async fn send_logs(&self, log_type: hyper::header::HeaderValue, logs: Vec<u8>) -> anyhow::Result<()> {
		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::OK => Some(Response),
					_ => None,
				})
			}
		}

		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
		const BODY_PREFIX: hyper::body::Bytes = hyper::body::Bytes::from_static(b"[");
		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
		const BODY_SUFFIX: hyper::body::Bytes = hyper::body::Bytes::from_static(b"]");

		let content_length: hyper::header::HeaderValue = (1 + logs.len() + 1).into();
		let content_length_s = content_length.to_str().expect("usize HeaderValue should be convertible to str").to_owned();

		#[allow(clippy::borrow_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
		let body =
			futures_util::StreamExt::chain(
				futures_util::StreamExt::chain(
					futures_util::stream::iter(std::iter::once(Ok::<_, std::convert::Infallible>(BODY_PREFIX))),
					futures_util::stream::iter(std::iter::once(Ok(logs.into()))),
				),
				futures_util::stream::iter(std::iter::once(Ok(BODY_SUFFIX))),
			);

		log::info!("Sending {}B logs to LogAnalytics workspace {} ...", content_length_s, self.customer_id);

		let x_ms_date: hyper::header::HeaderValue = {
			// Ref: https://tools.ietf.org/html/rfc822#section-5.1
			//
			// chrono's `to_rfc2822()` comes close, but it uses `+00:00` at the end instead of `GMT` which Azure doesn't like.
			let x_ms_date = chrono::Utc::now();
			let x_ms_date = x_ms_date.format_with_items([
				chrono::format::Item::Fixed(chrono::format::Fixed::ShortWeekdayName),
				chrono::format::Item::Literal(", "),
				chrono::format::Item::Numeric(chrono::format::Numeric::Day, chrono::format::Pad::Zero),
				chrono::format::Item::Literal(" "),
				chrono::format::Item::Fixed(chrono::format::Fixed::ShortMonthName),
				chrono::format::Item::Literal(" "),
				chrono::format::Item::Numeric(chrono::format::Numeric::Year, chrono::format::Pad::Zero),
				chrono::format::Item::Literal(" "),
				chrono::format::Item::Numeric(chrono::format::Numeric::Hour, chrono::format::Pad::Zero),
				chrono::format::Item::Literal(":"),
				chrono::format::Item::Numeric(chrono::format::Numeric::Minute, chrono::format::Pad::Zero),
				chrono::format::Item::Literal(":"),
				chrono::format::Item::Numeric(chrono::format::Numeric::Second, chrono::format::Pad::Zero),
				chrono::format::Item::Literal(" GMT"),
			].iter());
			std::convert::TryInto::try_into(x_ms_date.to_string()).context("could not create authorization header")?
		};

		let signature = {
			let mut signer = self.signer.clone();
			hmac::Mac::update(&mut signer, b"POST\n");
			hmac::Mac::update(&mut signer, content_length.as_bytes());
			hmac::Mac::update(&mut signer, b"\napplication/json\nx-ms-date:");
			hmac::Mac::update(&mut signer, x_ms_date.as_bytes());
			hmac::Mac::update(&mut signer, b"\n/api/logs");
			let signature = hmac::Mac::finalize(signer).into_bytes();
			let signature = base64::encode(&signature);
			signature
		};
		let authorization =
			std::convert::TryInto::try_into(format!("{}{}", self.authorization_prefix, signature))
			.context("could not create authorization header")?;

		let mut req = hyper::Request::new(hyper::Body::wrap_stream(body));
		*req.uri_mut() = self.uri.clone();
		*req.method_mut() = hyper::Method::POST;
		{
			static LOG_TYPE: once_cell2::race::LazyBox<hyper::header::HeaderName> =
				once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("log-type"));
			static TIME_GENERATED_FIELD: once_cell2::race::LazyBox<hyper::header::HeaderName> =
				once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("time-generated-field"));
			static X_MS_DATE: once_cell2::race::LazyBox<hyper::header::HeaderName> =
				once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("x-ms-date"));

			let headers = req.headers_mut();
			headers.insert(hyper::header::AUTHORIZATION, authorization);
			headers.insert(hyper::header::CONTENT_LENGTH, content_length);
			headers.insert(hyper::header::CONTENT_TYPE, http_common::APPLICATION_JSON.clone());
			headers.insert(LOG_TYPE.clone(), log_type);
			headers.insert(TIME_GENERATED_FIELD.clone(), log2::TIME_GENERATED_FIELD.clone());
			headers.insert(X_MS_DATE.clone(), x_ms_date);
		}

		let _: Response = self.client.request_inner(req).await?;

		log::info!("Sent {}B logs to LogAnalytics workspace {}", content_length_s, self.customer_id);

		Ok(())
	}
}
