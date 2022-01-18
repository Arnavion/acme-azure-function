impl<'a> super::Client<'a> {
	pub async fn dns_txt_record_create(&self, dns_zone_name: &str, name: &str, content: &str) -> anyhow::Result<()> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			properties: RequestProperties<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestProperties<'a> {
			#[serde(rename = "TTL")]
			ttl: u64,

			#[serde(rename = "TXTRecords")]
			txt_records: &'a [RequestPropertiesTxtRecord<'a>]
		}

		#[derive(serde::Serialize)]
		struct RequestPropertiesTxtRecord<'a> {
			value: &'a [&'a str],
		}

		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				_body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					http::StatusCode::OK |
					http::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		let () = self.logger.report_operation("azure/dns/txtrecord", (dns_zone_name, name), log2::ScopedObjectOperation::Create { value: "******" }, async {
			let _: Response =
				crate::request(
					self,
					http::Method::PUT,
					format_args!("/providers/Microsoft.Network/dnsZones/{dns_zone_name}/TXT/{name}?api-version=2018-05-01"),
					Some(&Request {
						properties: RequestProperties {
							ttl: 1,
							txt_records: &[
								RequestPropertiesTxtRecord {
									value: &[content],
								},
							],
						},
					}),
				).await?;

			Ok(())
		}).await?;

		Ok(())
	}

	pub async fn dns_txt_record_delete(&self, dns_zone_name: &str, name: &str) -> anyhow::Result<()> {
		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				_body: Option<(&http::HeaderValue, &mut http_common::Body<impl std::io::Read>)>,
				_headers: http::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					http::StatusCode::ACCEPTED |
					http::StatusCode::NOT_FOUND |
					http::StatusCode::OK => Some(Response),
					_ => None,
				})
			}
		}

		let () = self.logger.report_operation("azure/dns/txtrecord", (dns_zone_name, name), <log2::ScopedObjectOperation>::Delete, async {
			let _: Response =
				crate::request(
					self,
					http::Method::DELETE,
					format_args!("/providers/Microsoft.Network/dnsZones/{dns_zone_name}/TXT/{name}?api-version=2018-05-01"),
					None::<&()>,
				).await?;
			Ok(())
		}).await?;

		Ok(())
	}
}
