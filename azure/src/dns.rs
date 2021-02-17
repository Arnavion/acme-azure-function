impl<'a> crate::Account<'a> {
	pub async fn dns_txt_record_create(
		&mut self,
		dns_zone_name: &str,
		name: &str,
		content: &str,
	) -> anyhow::Result<()> {
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
				status: hyper::StatusCode,
				_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		eprintln!("Creating DNS TXT record {}/{} ...", dns_zone_name, name);

		let (url, authorization) =
			self.management_request_parameters(
				&format!(
					"/providers/Microsoft.Network/dnsZones/{}/TXT/{}?api-version=2018-05-01",
					dns_zone_name,
					name,
				),
			).await?;

		let _: Response =
			self.client.request(
				hyper::Method::PUT,
				&url,
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

		eprintln!("Created DNS TXT record {}/{}", dns_zone_name, name);

		Ok(())
	}

	pub async fn dns_txt_record_delete(
		&mut self,
		dns_zone_name: &str,
		name: &str,
	) -> anyhow::Result<()> {
		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::ACCEPTED |
					hyper::StatusCode::NOT_FOUND |
					hyper::StatusCode::OK => Some(Response),
					_ => None,
				})
			}
		}

		eprintln!("Deleting DNS TXT record {}/{} ...", dns_zone_name, name);

		let (url, authorization) =
			self.management_request_parameters(
				&format!(
					"/providers/Microsoft.Network/dnsZones/{}/TXT/{}?api-version=2018-05-01",
					dns_zone_name,
					name,
				),
			).await?;

		let _: Response =
			self.client.request(
				hyper::Method::DELETE,
				&url,
				authorization,
				None::<&()>,
			).await?;

		eprintln!("Deleted DNS TXT record {}/{}", dns_zone_name, name);

		Ok(())
	}
}
