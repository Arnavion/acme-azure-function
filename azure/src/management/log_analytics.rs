use anyhow::Context;

impl<'a> super::Client<'a> {
	pub async fn log_analytics_log_sender(self, workspace_name: &str) -> anyhow::Result<LogSender<'a>> {
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
				status: http::StatusCode,
				body: Option<&mut http_common::Body<impl std::io::Read>>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some(body)) => Some(body.as_json()?),
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
				status: http::StatusCode,
				body: Option<&mut http_common::Body<impl std::io::Read>>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(http::StatusCode::OK, Some(body)) => Some(body.as_json()?),
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
					let key = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s).map_err(serde::de::Error::custom)?;
					let signer = hmac::Mac::new_from_slice(&key).expect("cannot fail to create hmac::Hmac<sha2::Sha256>");
					Ok(signer)
				}
			}

			deserializer.deserialize_str(Visitor)
		}

		let customer_id =
			self.logger.report_operation(
				"azure/log_analytics/workspace",
				workspace_name,
				<log2::ScopedObjectOperation>::Get,
				async {
					let GetResponse { properties: Properties { customer_id } } =
						crate::request(
							&self,
							http::Method::GET,
							format_args!("/providers/Microsoft.OperationalInsights/workspaces/{workspace_name}?api-version=2022-10-01"),
							None::<&()>,
						).await?;
					Ok(customer_id)
				},
			);

		let signer =
			self.logger.report_operation(
				"azure/log_analytics/workspace/shared_access_keys",
				workspace_name,
				<log2::ScopedObjectOperation>::Get,
				async {
					let SharedKeysResponse { primary_shared_key } =
						crate::request(
							&self,
							http::Method::POST,
							format_args!("/providers/Microsoft.OperationalInsights/workspaces/{workspace_name}/sharedKeys?api-version=2022-10-01"),
							None::<&()>,
						).await?;
					Ok(log2::Secret(primary_shared_key))
				},
			);

		let (customer_id, log2::Secret(signer)) = futures_util::future::try_join(customer_id, signer).await?;

		let uri =
			format!("https://{customer_id}.ods.opinsights.azure.com/api/logs?api-version=2016-04-01")
			.try_into().context("could not construct Log Analytics Data Collector API URI")?;
		let authorization_prefix = format!("SharedKey {customer_id}:");

		Ok(LogSender {
			customer_id,
			signer,
			client: self.client,
			uri,
			authorization_prefix,
			logger: self.logger,
		})
	}
}

pub struct LogSender<'a> {
	customer_id: String,
	uri: http::Uri,
	authorization_prefix: String,
	signer: hmac::Hmac<sha2::Sha256>,
	client: http_common::Client,
	logger: &'a log2::Logger,
}

impl LogSender<'_> {
	pub async fn send_logs(&self, log_type: http::HeaderValue, logs: Vec<u8>) -> anyhow::Result<()> {
		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
		const BODY_PREFIX: hyper::body::Bytes = hyper::body::Bytes::from_static(b"[");
		#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const hyper::body::Bytes
		const BODY_SUFFIX: hyper::body::Bytes = hyper::body::Bytes::from_static(b"]");

		let content_length: http::HeaderValue = (1 + logs.len() + 1).into();
		let content_length_s = content_length.to_str().expect("usize HeaderValue should be convertible to str").to_owned();

		let body =
			futures_util::stream::iter([
				Ok::<_, std::convert::Infallible>(BODY_PREFIX),
				Ok(logs.into()),
				Ok(BODY_SUFFIX),
			]);

		self.logger.report_operation(
			"azure/log_analytics/logs",
			&self.customer_id,
			log2::ScopedObjectOperation::Create { value: format_args!("{content_length_s}B") },
			async {
				struct Response;

				impl http_common::FromResponse for Response {
					fn from_response(
						status: http::StatusCode,
						_body: Option<&mut http_common::Body<impl std::io::Read>>,
						_headers: http::HeaderMap,
					) -> anyhow::Result<Option<Self>> {
						Ok(match status {
							http::StatusCode::OK => Some(Response),
							_ => None,
						})
					}
				}

				// Ref: https://tools.ietf.org/html/rfc822#section-5.1
				//
				// time's `time::format_description::well_known::Rfc2822` comes close, but it uses `+00:00` at the end
				// instead of `GMT` which Azure doesn't like.
				const RFC2822: &[time::format_description::FormatItem<'_>] = &[
					time::format_description::FormatItem::Component(time::format_description::Component::Weekday({
						let mut weekday = time::format_description::modifier::Weekday::default();
						weekday.repr = time::format_description::modifier::WeekdayRepr::Short;
						weekday
					})),
					time::format_description::FormatItem::Literal(b", "),
					time::format_description::FormatItem::Component(time::format_description::Component::Day(time::format_description::modifier::Day::default())),
					time::format_description::FormatItem::Literal(b" "),
					time::format_description::FormatItem::Component(time::format_description::Component::Month({
						let mut month = time::format_description::modifier::Month::default();
						month.repr = time::format_description::modifier::MonthRepr::Short;
						month
					})),
					time::format_description::FormatItem::Literal(b" "),
					time::format_description::FormatItem::Component(time::format_description::Component::Year(time::format_description::modifier::Year::default())),
					time::format_description::FormatItem::Literal(b" "),
					time::format_description::FormatItem::Component(time::format_description::Component::Hour(time::format_description::modifier::Hour::default())),
					time::format_description::FormatItem::Literal(b":"),
					time::format_description::FormatItem::Component(time::format_description::Component::Minute(time::format_description::modifier::Minute::default())),
					time::format_description::FormatItem::Literal(b":"),
					time::format_description::FormatItem::Component(time::format_description::Component::Second(time::format_description::modifier::Second::default())),
					time::format_description::FormatItem::Literal(b" GMT"),
				];

				let x_ms_date: http::HeaderValue =
					time::OffsetDateTime::now_utc()
					.format(RFC2822)
					.expect("could not format date")
					.try_into()
					.context("could not create authorization header")?;

				let signature = {
					let mut signer = self.signer.clone();
					hmac::Mac::update(&mut signer, b"POST\n");
					hmac::Mac::update(&mut signer, content_length.as_bytes());
					hmac::Mac::update(&mut signer, b"\napplication/json\nx-ms-date:");
					hmac::Mac::update(&mut signer, x_ms_date.as_bytes());
					hmac::Mac::update(&mut signer, b"\n/api/logs");
					let signature = hmac::Mac::finalize(signer).into_bytes();
					base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature)
				};
				let authorization =
					format!("{}{signature}", self.authorization_prefix)
					.try_into().context("could not create authorization header")?;

				let mut req = http::Request::new(hyper::Body::wrap_stream(body));
				*req.uri_mut() = self.uri.clone();
				*req.method_mut() = http::Method::POST;
				{
					#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderName
					const LOG_TYPE: http::header::HeaderName = http::header::HeaderName::from_static("log-type");
					#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderName
					const TIME_GENERATED_FIELD: http::header::HeaderName = http::header::HeaderName::from_static("time-generated-field");
					#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderName
					const X_MS_DATE: http::header::HeaderName = http::header::HeaderName::from_static("x-ms-date");

					let headers = req.headers_mut();
					headers.insert(http::header::AUTHORIZATION, authorization);
					headers.insert(http::header::CONTENT_LENGTH, content_length);
					headers.insert(http::header::CONTENT_TYPE, crate::APPLICATION_JSON);
					headers.insert(LOG_TYPE, log_type);
					headers.insert(TIME_GENERATED_FIELD, log2::TIME_GENERATED_FIELD);
					headers.insert(X_MS_DATE, x_ms_date);
				}

				let _: Response = self.client.request(req).await?;
				Ok(())
			},
		).await?;

		Ok(())
	}
}
