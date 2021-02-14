use anyhow::Context;

impl<'a> crate::Account<'a> {
	pub async fn key_vault_csr_create(
		&mut self,
		key_vault_name: &str,
		certificate_name: &str,
		common_name: &str,
	) -> anyhow::Result<Vec<u8>> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			policy: RequestPolicy<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicy<'a> {
			issuer: RequestPolicyIssuer<'a>,
			key_props: RequestPolicyKeyProps<'a>,
			x509_props: RequestPolicyX509Props<'a>,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyIssuer<'a> {
			cert_transparency: bool,
			name: &'a str,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyKeyProps<'a> {
			exportable: bool,
			key_size: u32,
			kty: &'a str,
			reuse_key: bool,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyX509Props<'a> {
			sans: RequestPolicyX509PropsSans<'a>,
			subject: &'a str,
		}

		#[derive(serde::Serialize)]
		struct RequestPolicyX509PropsSans<'a> {
			dns_names: &'a [&'a str],
		}

		#[derive(serde::Deserialize)]
		struct Response {
			csr: String,
		}

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::ACCEPTED, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		eprintln!("Creating CSR {}/{} ...", key_vault_name, certificate_name);

		let (url, authorization) =
			self.key_vault_request_parameters(
				key_vault_name,
				&format!("/certificates/{}/create?api-version=7.1", certificate_name),
			).await?;

		let (Response { csr }, _) =
			self.client.request(
				hyper::Method::POST,
				&url,
				authorization,
				Some(&Request {
					policy: RequestPolicy {
						issuer: RequestPolicyIssuer {
							cert_transparency: false,
							name: "Unknown",
						},
						key_props: RequestPolicyKeyProps {
							exportable: true,
							key_size: 4096,
							kty: "RSA",
							reuse_key: false,
						},
						x509_props: RequestPolicyX509Props {
							sans: RequestPolicyX509PropsSans {
								dns_names: &[&common_name],
							},
							subject: &format!("CN={}", common_name),
						},
					},
				}),
			).await?;
		let csr = base64::decode(&csr).context("could not parse CSR from base64")?;
		eprintln!("Created CSR {}/{}", key_vault_name, certificate_name);
		Ok(csr)
	}

	pub async fn key_vault_certificate_get(
		&mut self,
		key_vault_name: &str,
		certificate_name: &str,
	) -> anyhow::Result<Option<Certificate>> {
		struct Response(Option<Certificate>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
			) -> anyhow::Result<Option<Self>> {
				#[derive(serde::Deserialize)]
				struct ResponseInner {
					attributes: ResponseAttributes,
					id: String,
				}

				#[derive(serde::Deserialize)]
				struct ResponseAttributes {
					exp: i64,
				}

				Ok(match (status, body) {
					(hyper::StatusCode::OK, Some((content_type, body))) if http_common::is_json(content_type) => {
						let ResponseInner { attributes: ResponseAttributes { exp }, id } = serde_json::from_reader(body)?;

						let not_after =
							chrono::TimeZone::ymd(&chrono::Utc, 1970, 1, 1).and_hms(0, 0, 0) +
							chrono::Duration::seconds(exp);

						let version = id.split('/').last().expect("str::split yields at least one part").to_owned();

						Some(Response(Some(Certificate {
							not_after,
							version,
						})))
					},

					(hyper::StatusCode::NOT_FOUND, _) => Some(Response(None)),

					_ => None,
				})
			}
		}

		eprintln!("Getting certificate {}/{} ...", key_vault_name, certificate_name);

		let (url, authorization) =
			self.key_vault_request_parameters(
				key_vault_name,
				&format!("/certificates/{}?api-version=7.1", certificate_name),
			).await?;

		let (response, _) =
			self.client.request(
				hyper::Method::GET,
				&url,
				authorization,
				None::<&()>,
			).await?;
		Ok(match response {
			Response(Some(certificate)) => {
				eprintln!("Got certificate {}/{}: {:?}", key_vault_name, certificate_name, certificate);
				Some(certificate)
			},
			Response(None) => {
				eprintln!("Certificate {}/{} does not exist", key_vault_name, certificate_name);
				None
			},
		})
	}

	pub async fn key_vault_certificate_merge(
		&mut self,
		key_vault_name: &str,
		certificate_name: &str,
		certificates: &[String],
	) -> anyhow::Result<()> {
		#[derive(serde::Serialize)]
		struct Request<'a> {
			x5c: &'a [String],
		}

		struct Response;

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				_body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		eprintln!("Merging certificate {}/{} ...", key_vault_name, certificate_name);

		let (url, authorization) =
			self.key_vault_request_parameters(
				key_vault_name,
				&format!("/certificates/{}/pending/merge?api-version=7.1", certificate_name),
			).await?;

		let _: (Response, _) =
			self.client.request(
				hyper::Method::POST,
				&url,
				authorization,
				Some(&Request {
					x5c: certificates,
				}),
			).await?;

		eprintln!("Merged certificate {}/{}", key_vault_name, certificate_name);

		Ok(())
	}
}

#[derive(Debug)]
pub struct Certificate {
	pub version: String,
	pub not_after: chrono::DateTime<chrono::Utc>,
}
