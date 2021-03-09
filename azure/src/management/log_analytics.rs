use anyhow::Context;

impl<'a> super::Client<'a> {
	pub async fn log_analytics_log_sender(self, workspace_name: &str) -> anyhow::Result<LogSender> {
		#[derive(serde::Deserialize)]
		struct GetResponse {
			properties: Properties,
		}

		#[derive(serde::Deserialize)]
		struct Properties {
			#[serde(rename = "customerId")]
			customer_id: String,
		}

		impl http_common::FromResponse for GetResponse {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		#[derive(serde::Deserialize)]
		struct SharedKeysResponse {
			#[serde(deserialize_with = "deserialize_signer")]
			#[serde(rename = "primarySharedKey")]
			primary_shared_key: hmac::Hmac<sha2::Sha256>,
		}

		impl http_common::FromResponse for SharedKeysResponse {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		fn deserialize_signer<'de, D>(deserializer: D) -> Result<hmac::Hmac<sha2::Sha256>, D::Error> where D: serde::Deserializer<'de> {
			struct Visitor;

			impl serde::de::Visitor<'_> for Visitor {
				type Value = hmac::Hmac<sha2::Sha256>;

				fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
					f.write_str("base64-encoded string")
				}

				fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
					let key = base64::decode(s).map_err(serde::de::Error::custom)?;
					let signer = hmac::NewMac::new_varkey(&key).expect("cannot fail to create hmac::Hmac<sha2::Sha256>");
					Ok(signer)
				}
			}

			deserializer.deserialize_str(Visitor)
		}

		let customer_id =
			log2::report_operation(
				"azure/log_analytics/workspace",
				workspace_name,
				<log2::ScopedObjectOperation>::Get,
				async {
					let (url, authorization) =
						self.request_parameters(format_args!(
							"/providers/Microsoft.OperationalInsights/workspaces/{}?api-version=2020-08-01",
							workspace_name,
						)).await?;

					let GetResponse { properties: Properties { customer_id } } =
						self.client.request(
							hyper::Method::GET,
							url,
							authorization,
							None::<&()>,
						).await?;
					Ok::<_, anyhow::Error>(customer_id)
				},
			);

		let signer =
			log2::report_operation(
				"azure/log_analytics/workspace/shared_access_keys",
				workspace_name,
				<log2::ScopedObjectOperation>::Get,
				async {
					let (url, authorization) =
						self.request_parameters(format_args!(
							"/providers/Microsoft.OperationalInsights/workspaces/{}/sharedKeys?api-version=2020-08-01",
							workspace_name,
						)).await?;

					let SharedKeysResponse { primary_shared_key } =
						self.client.request(
							hyper::Method::POST,
							url,
							authorization,
							None::<&()>,
						).await?;
					Ok::<_, anyhow::Error>(log2::Secret(primary_shared_key))
				},
			);

		let (customer_id, log2::Secret(signer)) = futures_util::future::try_join(customer_id, signer).await?;

		let uri =
			std::convert::TryInto::try_into(format!("https://{}.ods.opinsights.azure.com/api/logs?api-version=2016-04-01", customer_id))
			.context("could not construct Log Analytics Data Collector API URI")?;
		let authorization_prefix = format!("SharedKey {}:", customer_id);

		Ok(LogSender {
			customer_id,
			signer,
			client: self.client,
			uri,
			authorization_prefix,
		})
	}
}

pub struct LogSender {
	customer_id: String,
	uri: hyper::Uri,
	authorization_prefix: String,
	signer: hmac::Hmac<sha2::Sha256>,
	client: http_common::Client,
}

impl LogSender {
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
