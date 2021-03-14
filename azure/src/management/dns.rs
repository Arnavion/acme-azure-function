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
				_body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
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
			let (url, authorization) =
				self.request_parameters(format_args!(
					"/providers/Microsoft.Network/dnsZones/{}/TXT/{}?api-version=2018-05-01",
					dns_zone_name,
					name,
				)).await?;

			let _: Response =
				self.client.request(
					http::Method::PUT,
					url,
					authorization,
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

			Ok::<_, anyhow::Error>(())
		}).await?;

		Ok(())
	}

	pub async fn dns_txt_record_delete(&self, dns_zone_name: &str, name: &str) -> anyhow::Result<()> {
		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: http::StatusCode,
				_body: Option<(&http::HeaderValue, &mut impl std::io::Read)>,
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
			let (url, authorization) =
				self.request_parameters(format_args!(
					"/providers/Microsoft.Network/dnsZones/{}/TXT/{}?api-version=2018-05-01",
					dns_zone_name,
					name,
				)).await?;

			let _: Response =
				self.client.request(
					http::Method::DELETE,
					url,
					authorization,
					None::<&()>,
				).await?;
			Ok::<_, anyhow::Error>(())
		}).await?;

		Ok(())
	}
}
