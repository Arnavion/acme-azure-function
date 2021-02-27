use anyhow::Context;

pub struct LogSender {
	customer_id: String,
	primary_shared_key: Vec<u8>,

	client: http_common::Client,
	uri: hyper::Uri,
}

impl LogSender {
	pub fn new(customer_id: String, primary_shared_key: Vec<u8>, user_agent: &str) -> anyhow::Result<Self> {
		let uri =
			format!("https://{}.ods.opinsights.azure.com/api/logs?api-version=2016-04-01", customer_id)
			.parse().context("could not construct Log Analytics Data Collector API URI")?;

		Ok(LogSender {
			customer_id,
			primary_shared_key,

			client: http_common::Client::new(user_agent).context("could not create HTTP client")?,
			uri,
		})
	}

	pub async fn send_logs(&self, log_type: hyper::header::HeaderValue, logs: &[u8]) -> anyhow::Result<()> {
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

		let (body_len, body) = {
			let mut body = Vec::with_capacity(logs.len() + 2);
			body.push(b'[');
			body.extend_from_slice(logs);
			body.push(b']');
			(body.len(), body)
		};

		log::info!("Sending {}B logs to LogAnalytics workspace {} ...", body_len, self.customer_id);

		let x_ms_date = {
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
			x_ms_date.to_string()
		};

		let signature_input = format!(
			"POST\n{}\napplication/json\nx-ms-date:{}\n/api/logs",
			body.len(),
			x_ms_date,
		);

		let mut signer: hmac::Hmac<sha2::Sha256> = hmac::NewMac::new_varkey(&self.primary_shared_key).context("could not create signer")?;
		hmac::Mac::update(&mut signer, signature_input.as_bytes());
		let signature = hmac::Mac::finalize(signer).into_bytes();
		let signature = base64::encode(&signature);
		let authorization = format!("SharedKey {}:{}", self.customer_id, signature);
		let authorization = authorization.parse().context("could not create authorization header")?;

		let x_ms_date = x_ms_date.parse().context("could not create x-ms-date header")?;

		let mut req = hyper::Request::new(body.into());
		*req.uri_mut() = self.uri.clone();
		*req.method_mut() = hyper::Method::POST;
		{
			let headers = req.headers_mut();
			headers.insert(hyper::header::AUTHORIZATION, authorization);
			headers.insert(hyper::header::CONTENT_TYPE, http_common::APPLICATION_JSON.clone());
			headers.insert("log-type", log_type);
			headers.insert("time-generated-field", log2::TIME_GENERATED_FIELD.clone());
			headers.insert("x-ms-date", x_ms_date);
		}

		let _: Response = self.client.request_inner(req).await?;

		log::info!("Sent {}B logs to LogAnalytics workspace {}", body_len, self.customer_id);

		Ok(())
	}
}

#[derive(serde::Serialize)]
pub struct LogRecord {
	#[serde(rename = "TimeCollected", serialize_with = "serialize_timestamp")]
	pub timestamp: chrono::DateTime<chrono::Utc>,

	#[serde(rename = "FunctionInvocationId", skip_serializing_if = "Option::is_none")]
	pub function_invocation_id: Option<std::sync::Arc<str>>,

	#[serde(rename = "SequenceNumber")]
	pub sequence_number: usize,

	#[serde(rename = "Level", serialize_with = "serialize_log_level")]
	pub level: log::Level,

	#[serde(rename = "Message")]
	pub message: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // clippy wants `log::Level` to be passed by value, but serde requires it to be passed as a borrow.
fn serialize_log_level<S>(level: &log::Level, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
	match level {
		log::Level::Debug => serializer.serialize_str("Debug"),
		log::Level::Error => serializer.serialize_str("Error"),
		log::Level::Info => serializer.serialize_str("Information"),
		log::Level::Trace => serializer.serialize_str("Trace"),
		log::Level::Warn => serializer.serialize_str("Warning"),
	}
}

fn serialize_timestamp<S>(timestamp: &chrono::DateTime<chrono::Utc>, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
	serializer.serialize_str(&timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
