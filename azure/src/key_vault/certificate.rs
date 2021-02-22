use anyhow::Context;

impl<'a> crate::Account<'a> {
	pub async fn key_vault_csr_create(
		&self,
		key_vault_name: &str,
		certificate_name: &str,
		common_name: &str,
		key_type: CreateCsrKeyType,
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
			#[serde(skip_serializing_if = "Option::is_none")]
			crv: Option<super::EcCurve>,
			exportable: bool,
			#[serde(skip_serializing_if = "Option::is_none")]
			key_size: Option<u16>,
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
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match (status, body) {
					(hyper::StatusCode::ACCEPTED, Some((content_type, body))) if http_common::is_json(content_type) =>
						Some(serde_json::from_reader(body)?),
					_ => None,
				})
			}
		}

		eprintln!("Creating CSR {}/{} ...", key_vault_name, certificate_name);

		let key_vault_request_parameters =
			self.key_vault_request_parameters(
				key_vault_name,
				format_args!("/certificates/{}/create?api-version=7.1", certificate_name),
			);
		let (url, authorization) = key_vault_request_parameters.await?;

		let Response { csr } =
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
						key_props: {
							let (kty, crv, key_size, exportable) = match key_type {
								CreateCsrKeyType::Ec { curve, exportable } => ("EC", Some(curve), None, exportable),
								CreateCsrKeyType::EcHsm { curve } => ("EC-HSM", Some(curve), None, false),
								CreateCsrKeyType::Rsa { num_bits, exportable } => ("RSA", None, Some(num_bits), exportable),
								CreateCsrKeyType::RsaHsm { num_bits } => ("RSA-HSM", None, Some(num_bits), false),
							};

							RequestPolicyKeyProps {
								crv,
								exportable,
								key_size,
								kty,
								reuse_key: false,
							}
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
		&self,
		key_vault_name: &str,
		certificate_name: &str,
	) -> anyhow::Result<Option<Certificate>> {
		struct Response(Option<Certificate>);

		impl http_common::FromResponse for Response {
			fn from_response(
				status: hyper::StatusCode,
				body: Option<(&hyper::header::HeaderValue, &mut impl std::io::Read)>,
				_headers: hyper::HeaderMap,
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

		let key_vault_request_parameters =
			self.key_vault_request_parameters(
				key_vault_name,
				format_args!("/certificates/{}?api-version=7.1", certificate_name),
			);
		let (url, authorization) = key_vault_request_parameters.await?;

		let response =
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
		&self,
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
				_headers: hyper::HeaderMap,
			) -> anyhow::Result<Option<Self>> {
				Ok(match status {
					hyper::StatusCode::CREATED => Some(Response),
					_ => None,
				})
			}
		}

		eprintln!("Merging certificate {}/{} ...", key_vault_name, certificate_name);

		let key_vault_request_parameters =
			self.key_vault_request_parameters(
				key_vault_name,
				format_args!("/certificates/{}/pending/merge?api-version=7.1", certificate_name),
			);
		let (url, authorization) = key_vault_request_parameters.await?;

		let _: Response =
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

#[derive(Clone, Copy, Debug)]
pub enum CreateCsrKeyType {
	Ec {
		curve: super::EcCurve,
		exportable: bool,
	},

	EcHsm {
		curve: super::EcCurve,
	},

	Rsa {
		num_bits: u16,
		exportable: bool,
	},

	RsaHsm {
		num_bits: u16,
	},
}

#[derive(Debug)]
pub struct Certificate {
	pub version: String,
	pub not_after: chrono::DateTime<chrono::Utc>,
}
